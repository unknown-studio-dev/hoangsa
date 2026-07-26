//! E2E — `hoangsa-cli update` drives the real binary against a local installer
//! server.
//!
//! The unit tests in `cmd/update.rs` cover the pieces: `installer_argv` refuses
//! a hostile tag, and `fetch_to_file` deletes a partially written download.
//! What they cannot prove is what a real PROCESS does, and EC-10 is a claim
//! about the process: "nothing executed, exit 1, no partial install". Until the
//! two debug-only seams landed, `cmd_update` had no test driving it at all.
//!
//! Two things make these tests worth having rather than decorative:
//!
//! 1. The exit code alone pins nothing. Deleting the `std::process::exit(1)`
//!    that follows a failed download still exits 1 — control falls through to
//!    the spawn, `sh <file that was never downloaded>` fails, and the `Ok(s)`
//!    arm exits 1 too. Only the error TEXT distinguishes the two: a download
//!    failure says "installer download failed", the fall-through says
//!    "installer exited". `update_exits_1_and_runs_nothing_when_the_download_fails`
//!    asserts on that distinction.
//! 2. "Nothing was executed" is vacuous in a harness that could never execute
//!    anything, so `update_runs_the_installer_when_the_download_succeeds` is
//!    the control: the same harness serves a working installer and the sentinel
//!    it creates must EXIST. Same shape as `prove_gate_would_fire` in
//!    `cli_e2e.rs`.
//!
//! The seam that makes this reachable (`HOANGSA_UPDATE_LATEST_TAG` and
//! `HOANGSA_UPDATE_BASE_URL`) is `#[cfg(debug_assertions)]`, so it exists in the
//! binary these tests run and not in a released one.

use serde_json::Value;
use std::fs;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Hard deadline for the child. `curl --retry 2` against a 500 costs a few
/// seconds; anything near this bound is a hang, and a test that waits without a
/// deadline blocks the suite instead of reporting.
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

const TAG: &str = "v9.9.9";

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    fn error(&self) -> String {
        let v: Value = serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "update stdout is not a single JSON object ({e})\n  stdout: {}\n  stderr: {}",
                self.stdout, self.stderr
            )
        });
        v["error"]
            .as_str()
            .unwrap_or_else(|| panic!("no `error` string in output: {v}"))
            .to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "update stdout is not JSON ({e})\n  stdout: {}\n  stderr: {}",
                self.stdout, self.stderr
            )
        })
    }
}

/// A loopback HTTP server that answers every request with the same bytes.
/// `curl --retry 2` may connect more than once, so it serves in a loop until
/// dropped, and counts the requests it answered — a test against a server that
/// was never reached proves nothing.
struct Server {
    addr: SocketAddr,
    hits: Arc<AtomicUsize>,
    handle: Option<JoinHandle<()>>,
}

impl Server {
    fn spawn(response: Vec<u8>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let addr = listener.local_addr().expect("read the bound port");
        let hits = Arc::new(AtomicUsize::new(0));

        let counter = Arc::clone(&hits);
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(c) => c,
                    Err(_) => break,
                };
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                // The shutdown poke in `drop` connects without sending a
                // request; do not count it as a served download.
                if buf[0] == 0 {
                    break;
                }
                let _ = stream.write_all(&response);
                let _ = stream.flush();
                counter.fetch_add(1, Ordering::SeqCst);
            }
        });

        Server {
            addr,
            hits,
            handle: Some(handle),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Unblock the accept loop, then join, so no thread outlives the test.
        let _ = TcpStream::connect(self.addr);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn ok_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn error_response() -> Vec<u8> {
    b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 4\r\nConnection: close\r\n\r\nboom"
        .to_vec()
}

struct Fixture {
    // Kept alive for the test; dropping it deletes everything below.
    _tmp: TempDir,
    install: PathBuf,
    home: PathBuf,
    tmpdir: PathBuf,
    io: PathBuf,
    sentinel: PathBuf,
}

impl Fixture {
    /// A scratch install whose manifest claims an old version, so any tag the
    /// test supplies counts as an upgrade. `HOME`, `CLAUDE_CONFIG_DIR`, `TMPDIR`
    /// and the child's cwd all point inside the temp dir: the success path
    /// clears the update cache out of the config dirs, and nothing in a test may
    /// reach the developer's real `~/.claude`.
    fn new() -> Fixture {
        let tmp = TempDir::new().expect("create temp dir");
        let base = tmp.path().to_path_buf();
        let install = base.join("install");
        let home = base.join("home");
        let tmpdir = base.join("tmp");
        let io = base.join("io");

        for d in [&install, &home, &tmpdir, &io] {
            fs::create_dir_all(d).expect("create fixture dir");
        }
        fs::write(
            install.join("manifest.json"),
            r#"{"version":"0.0.1","files":{}}"#,
        )
        .expect("write manifest.json");

        let sentinel = base.join("installer-ran.sentinel");
        Fixture {
            _tmp: tmp,
            install,
            home,
            tmpdir,
            io,
            sentinel,
        }
    }

    /// An installer script that records that it ran, at a path only this test
    /// knows. `cmd_update` invokes it as `sh <path> --global`.
    fn installer_script(&self) -> String {
        format!("#!/bin/sh\ntouch '{}'\n", self.sentinel.display())
    }

    fn run(&self, tag: &str, base_url: &str) -> Run {
        let out_path = self.io.join("stdout.txt");
        let err_path = self.io.join("stderr.txt");
        let mut child = Command::new(env!("CARGO_BIN_EXE_hoangsa-cli"))
            .args(["update", "--yes"])
            .env("HOANGSA_INSTALL_DIR", &self.install)
            .env("HOANGSA_UPDATE_LATEST_TAG", tag)
            .env("HOANGSA_UPDATE_BASE_URL", base_url)
            .env("TMPDIR", &self.tmpdir)
            .env("HOME", &self.home)
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .current_dir(&self.home)
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                fs::File::create(&out_path).expect("create stdout file"),
            ))
            .stderr(Stdio::from(
                fs::File::create(&err_path).expect("create stderr file"),
            ))
            .spawn()
            .expect("spawn hoangsa-cli");

        let deadline = Instant::now() + RUN_TIMEOUT;
        let status = loop {
            match child.try_wait().expect("poll hoangsa-cli") {
                Some(status) => break status,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("`hoangsa-cli update --yes` did not exit within {RUN_TIMEOUT:?} — hung");
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        };

        Run {
            code: status.code(),
            stdout: fs::read_to_string(&out_path).expect("read stdout file"),
            stderr: fs::read_to_string(&err_path).expect("read stderr file"),
        }
    }
}

/// Every `install-*.sh` left under the child's TMPDIR — i.e. every file the
/// next run could execute.
fn installer_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return found,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(installer_files(&path));
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("install-") && n.ends_with(".sh"))
        {
            found.push(path);
        }
    }
    found
}

/// The per-process directory `cmd_update` downloads into. Its existence is what
/// makes "no installer file under TMPDIR" a real assertion rather than a
/// statement about the wrong directory.
fn download_dirs(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("hoangsa-update-"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// EC-28 — the control. The same harness, the same binary, the same code path,
/// with a server that actually serves an installer: the sentinel MUST exist.
/// Without this, the absence assertion in the 500 case would pass in a harness
/// that could never have executed anything.
#[test]
fn update_runs_the_installer_when_the_download_succeeds() {
    let fx = Fixture::new();
    let server = Server::spawn(ok_response(&fx.installer_script()));

    let run = fx.run(TAG, &server.base_url());

    assert!(
        fx.sentinel.exists(),
        "control: the served installer did not run — {} is missing\n  code: {:?}\n  stdout: {}\n  stderr: {}",
        fx.sentinel.display(),
        run.code,
        run.stdout,
        run.stderr
    );
    assert_eq!(
        run.code,
        Some(0),
        "a successful update must exit 0\n  stdout: {}\n  stderr: {}",
        run.stdout,
        run.stderr
    );
    assert_eq!(run.json()["status"].as_str(), Some("ok"));
    assert_eq!(
        run.json()["updated_to"].as_str(),
        Some(TAG),
        "the update must report the tag it installed: {}",
        run.stdout
    );
    assert!(server.hits() >= 1, "the installer was never downloaded");
    // Control for the failure case's directory assertion: this is where the
    // child downloads to, so looking there for a leftover installer is looking
    // in the right place.
    assert!(
        !download_dirs(&fx.tmpdir).is_empty(),
        "the child did not download inside {} — the failure case would be checking the wrong directory",
        fx.tmpdir.display()
    );
}

/// EC-10 — the download returns HTTP 500. Nothing executed, exit 1, no partial
/// install. The error text assertion is the load-bearing one: without it,
/// removing the `exit(1)` that follows a failed download still leaves the
/// process exiting 1 by falling through to `sh <missing file>`.
#[test]
fn update_exits_1_and_runs_nothing_when_the_download_fails() {
    let fx = Fixture::new();
    let server = Server::spawn(error_response());

    let run = fx.run(TAG, &server.base_url());

    assert!(
        !fx.sentinel.exists(),
        "a failed download EXECUTED something — {} exists",
        fx.sentinel.display()
    );
    // The discriminating assertion. "installer exited" is what the fall-through
    // reports when the download guard is gone; the exit code is 1 either way.
    assert!(
        !run.stdout.contains("installer exited"),
        "the failed download fell through to the installer spawn\n  stdout: {}\n  stderr: {}",
        run.stdout,
        run.stderr
    );
    assert_eq!(
        run.code,
        Some(1),
        "a failed download must exit 1\n  stdout: {}\n  stderr: {}",
        run.stdout,
        run.stderr
    );
    let err = run.error();
    assert!(
        err.contains("installer download failed"),
        "the error must name the DOWNLOAD as what failed; got {err:?}"
    );
    assert!(
        server.hits() >= 1,
        "the failing server was never reached — the test proved nothing"
    );
    let left = installer_files(&fx.tmpdir);
    assert!(
        left.is_empty(),
        "a failed download left an installer for the next run to execute: {left:?}"
    );
}

/// EC-09 — a hostile release tag reaches `cmd_update` from the releases API.
/// `installer_argv` refuses it as a unit; this asserts the PROCESS refuses it
/// too, before any download or execution. The base URL points at a dead port so
/// a regression here fails loudly instead of quietly fetching from github.com.
#[test]
fn update_refuses_a_hostile_release_tag_end_to_end() {
    let fx = Fixture::new();

    let run = fx.run("v1.0\"; touch /tmp/PWNED #", "http://127.0.0.1:1");

    assert!(
        !Path::new("/tmp/PWNED").exists(),
        "the hostile tag was executed as a command"
    );
    assert!(
        !fx.sentinel.exists(),
        "a refused tag must not reach an installer"
    );
    assert_eq!(
        run.code,
        Some(1),
        "a refused tag must exit 1\n  stdout: {}\n  stderr: {}",
        run.stdout,
        run.stderr
    );
    let err = run.error();
    assert!(
        err.contains("refusing to run installer"),
        "the error must say the tag was refused; got {err:?}"
    );
    assert!(
        installer_files(&fx.tmpdir).is_empty(),
        "a refused tag must not have downloaded anything"
    );
}
