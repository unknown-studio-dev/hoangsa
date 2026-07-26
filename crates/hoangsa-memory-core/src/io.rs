//! Small I/O helpers shared across crates.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Write `content` to `path` atomically: write to `<path>.tmp` first, then
/// rename over the target. Creates parent directories as needed.
///
/// Crash-mid-write leaves `<path>.tmp` orphaned (the original target stays
/// intact). `<path>.tmp` is per-target, so concurrent writers to *different*
/// targets are safe; concurrent writers to the *same* target race on the
/// temp file — callers needing inter-process exclusion must layer a lock on
/// top.
pub fn atomic_write(path: &Path, content: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // The temp name must be unique PER WRITER, not just per target. With a
    // shared `<path>.tmp`, two processes writing the same file interleave
    // inside the temp — the short writer truncates, the long writer's tail
    // survives, and the result is renamed into place as a torn file. That was
    // reproduced corrupting `projects.json`, which then killed the service
    // daemon for every project and never self-healed.
    let mut tmp_name: OsString = path.as_os_str().to_owned();
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp_name);
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Run `f` while holding an exclusive advisory lock on `<path>.lock`.
///
/// `atomic_write` makes a *publish* atomic, but a load-modify-save sequence
/// still needs mutual exclusion: two writers that both read the old registry
/// and both save produce a silent lost update. Reproduced dropping 20 of 40
/// concurrent project registrations.
///
/// The lock is advisory and released by the OS when the process exits, so a
/// crash mid-critical-section cannot wedge it. `wait` bounds how long we
/// block; on timeout `f` runs anyway — a lost update is strictly better than
/// a hung CLI on a machine where some other process is holding the lock for
/// too long.
pub fn with_file_lock<T>(path: &Path, wait: std::time::Duration, f: impl FnOnce() -> T) -> T {
    let lock_path = {
        let mut p: OsString = path.as_os_str().to_owned();
        p.push(".lock");
        PathBuf::from(p)
    };
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let handle = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .ok();

    let mut held = false;
    if let Some(file) = handle.as_ref() {
        let deadline = std::time::Instant::now() + wait;
        loop {
            if file.try_lock().is_ok() {
                held = true;
                break;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    let result = f();
    if held && let Some(file) = handle.as_ref() {
        let _ = file.unlock();
    }
    result
}
