//! Child process execution with streaming capture.
//!
//! We read stdout and stderr on separate threads so a verbose process can't
//! dead-lock us: OS pipe buffers are ~64 KB, and `std::process::Child::
//! output()` reads them sequentially, which stalls the child when its
//! stderr is loud and stdout is idle (or vice versa).
//!
//! Capture caps:
//!   - `WARN_THRESHOLD_BYTES` (10 MB) per stream — we keep reading but set
//!     a flag so the pipeline can surface an adaptive "output > 10MB"
//!     warning in the trim report.
//!   - `OUTPUT_CAP_BYTES` (100 MB) per stream — hard cap. Past this, we
//!     stop appending but keep draining the pipe so the child doesn't block
//!     on write. The last bytes of output past the cap are LOST — we'd
//!     rather keep head (which is usually the error context) than tail.
//!
//! Exit-code semantics:
//!   - `status.code()` if present → passthrough 1:1 (grep's 1 stays 1,
//!     cargo test's 101 stays 101).
//!   - Killed by signal on Unix → `128 + signum` (POSIX shell convention).
//!   - Otherwise → 1.
//!
//! This layer never interprets the exit code, never turns a non-zero into
//! an "hsp error".
//!
//! Signal propagation: Unix installs a SIGINT/SIGTERM/SIGHUP handler that
//! forwards the signal to the active child. Without this, Ctrl+C on a long-
//! running proxied command (`hsp cargo test`) would kill `hsp` and orphan
//! the child — which then keeps consuming CPU until it decides to exit.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

/// PID of the currently running child, or 0 when no child is alive.
/// Signal handlers read this atomically to decide whether (and whom) to
/// kill. Using `i32` to match `libc::pid_t`.
#[cfg(unix)]
static ACTIVE_CHILD_PID: AtomicI32 = AtomicI32::new(0);

/// Hard cap per stream. Output past this is dropped on the floor but the
/// pipe is still drained so the child doesn't block.
pub const OUTPUT_CAP_BYTES: usize = 100 * 1024 * 1024;

/// Soft threshold. Hitting this flips `stream_exceeded_warn_threshold`
/// without changing behaviour — phase 2d renders an adaptive message when
/// set.
pub const WARN_THRESHOLD_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Captured {
    pub stdout: String,
    pub stderr: String,
    pub exit: i32,
    /// True iff stdout was truncated at `OUTPUT_CAP_BYTES`.
    pub stdout_truncated: bool,
    /// True iff stderr was truncated at `OUTPUT_CAP_BYTES`.
    pub stderr_truncated: bool,
    /// True iff stdout crossed `WARN_THRESHOLD_BYTES` (may or may not be
    /// truncated).
    pub stdout_warn: bool,
    /// True iff stderr crossed `WARN_THRESHOLD_BYTES`.
    pub stderr_warn: bool,
    /// Total raw bytes received on stdout before any capping. Surfaces in
    /// the adaptive report so the user sees "we saw 42MB, kept 42MB".
    pub stdout_total_bytes: usize,
    /// Same, for stderr.
    pub stderr_total_bytes: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("failed to spawn child: {0}")]
    Spawn(#[from] std::io::Error),
}

/// Run with the default per-stream cap ([`OUTPUT_CAP_BYTES`]). Thin wrapper
/// around [`run_with_cap`] kept for callers that don't need the override.
pub fn run(
    cmd: &str,
    args: &[String],
    cwd: Option<&std::path::Path>,
) -> Result<Captured, ExecError> {
    run_with_cap(cmd, args, cwd, OUTPUT_CAP_BYTES)
}

/// Run `cmd` with `args` in `cwd` (None = inherit). Captures both streams
/// concurrently with a per-stream cap. Returns the exit code untouched.
pub fn run_with_cap(
    cmd: &str,
    args: &[String],
    cwd: Option<&std::path::Path>,
    cap_bytes: usize,
) -> Result<Captured, ExecError> {
    #[cfg(unix)]
    install_signal_forwarder();

    let mut command = Command::new(cmd);
    command.args(args);
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }

    let mut child = command.spawn()?;

    #[cfg(unix)]
    {
        // `Child::id()` is `u32`; libc PIDs are `i32`. Cast is safe for any
        // real PID (kernel never allocates values past i32::MAX).
        let pid = child.id() as i32;
        ACTIVE_CHILD_PID.store(pid, Ordering::SeqCst);
    }

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let out_state = Arc::new(Mutex::new(StreamState::default()));
    let err_state = Arc::new(Mutex::new(StreamState::default()));

    let out_handle = spawn_reader(stdout, Arc::clone(&out_state), cap_bytes);
    let err_handle = spawn_reader(stderr, Arc::clone(&err_state), cap_bytes);

    let status = child.wait()?;

    #[cfg(unix)]
    ACTIVE_CHILD_PID.store(0, Ordering::SeqCst);

    // Reader threads exit naturally when the child closes the pipe, but
    // join anyway so we can unwrap the state after the child is reaped.
    let _ = out_handle.join();
    let _ = err_handle.join();

    let out = Arc::try_unwrap(out_state)
        .expect("reader holds last ref")
        .into_inner()
        .expect("stdout state lock");
    let err = Arc::try_unwrap(err_state)
        .expect("reader holds last ref")
        .into_inner()
        .expect("stderr state lock");

    let exit = exit_code(&status);

    Ok(Captured {
        stdout: String::from_utf8_lossy(&out.buf).into_owned(),
        stderr: String::from_utf8_lossy(&err.buf).into_owned(),
        exit,
        stdout_truncated: out.truncated,
        stderr_truncated: err.truncated,
        stdout_warn: out.total >= WARN_THRESHOLD_BYTES,
        stderr_warn: err.total >= WARN_THRESHOLD_BYTES,
        stdout_total_bytes: out.total,
        stderr_total_bytes: err.total,
    })
}

/// Install SIGINT/SIGTERM/SIGHUP handlers that forward the signal to our
/// active child, then re-raise so the parent itself exits with conventional
/// 128+signum semantics.
///
/// Idempotent: only runs once per process. Uses `sigaction` directly so we
/// don't pull in a signal-handling crate for three syscalls.
#[cfg(unix)]
fn install_signal_forwarder() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: sigaction is standard POSIX; handler is async-signal-
            // safe (only atomic load + kill() + _exit()). We intentionally
            // overwrite any previously-installed handler — hsp is meant to
            // be a leaf process, no parent has a legitimate handler to
            // preserve.
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = forward_signal as usize;
                libc::sigemptyset(&mut sa.sa_mask);
                sa.sa_flags = libc::SA_RESTART;
                libc::sigaction(sig, &sa, std::ptr::null_mut());
            }
        }
    });
}

/// Signal handler. Runs in signal context — anything it calls must be
/// async-signal-safe. Atomic load, `kill(2)`, and `_exit(2)` are all on
/// POSIX's async-signal-safe list.
#[cfg(unix)]
extern "C" fn forward_signal(sig: i32) {
    // SAFETY: AtomicI32::load is lock-free, signal-safe.
    let pid = ACTIVE_CHILD_PID.load(Ordering::SeqCst);
    if pid > 0 {
        // Best-effort. If the child is already gone, ESRCH — ignore.
        unsafe {
            libc::kill(pid, sig);
        }
    }
    // Re-raise as default disposition so our own exit code reflects the
    // signal (128+signum). SA_RESETHAND would achieve this automatically
    // but sigaction(SIG_DFL) + raise gives us control if we ever want to
    // delay exit until after the child reaps.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = libc::SIG_DFL;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(sig, &sa, std::ptr::null_mut());
        libc::raise(sig);
    }
}

#[derive(Debug, Default)]
struct StreamState {
    buf: Vec<u8>,
    total: usize,
    truncated: bool,
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    state: Arc<Mutex<StreamState>>,
    cap_bytes: usize,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            let n = match reader.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            };
            // Scope the lock: don't hold it across next read(). Release
            // quickly so the other stream isn't starved if both fire at
            // once.
            if let Ok(mut s) = state.lock() {
                s.total = s.total.saturating_add(n);
                let room = cap_bytes.saturating_sub(s.buf.len());
                if room == 0 {
                    // Cap reached — drop chunk but keep draining so the
                    // child doesn't block on write.
                    s.truncated = true;
                    continue;
                }
                let take = n.min(room);
                s.buf.extend_from_slice(&chunk[..take]);
                if take < n {
                    s.truncated = true;
                }
            }
        }
    })
}

fn exit_code(status: &std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1
}
