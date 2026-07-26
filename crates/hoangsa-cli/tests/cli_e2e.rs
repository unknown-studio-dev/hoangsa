//! E2E — `rules compose` refuses a repository-supplied gate and executes nothing.
//!
//! The unit tests in `cmd/envelope.rs` assert the internal decision. What they
//! cannot prove is that a real `hoangsa-cli` PROCESS, run against a real
//! project directory an attacker controls, executes nothing. These tests drive
//! the built binary and assert on the process exit code, the process stdout,
//! and — the load-bearing one — the absence of a sentinel file that the gate
//! would have created.
//!
//! Asserting only on the skipped list would pass even if the command had run,
//! so every case checks the sentinel first.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Hard deadline for the child process. `rules compose` reads a handful of
/// small files and exits, so anything near this bound is a hang, not slowness.
/// A test that waits on a process without a deadline cannot report "does not
/// hang" — it reports nothing and blocks the suite instead.
const RUN_TIMEOUT: Duration = Duration::from_secs(30);

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "rules compose stdout is not JSON ({e})\n  stdout: {}\n  stderr: {}",
                self.stdout, self.stderr
            )
        })
    }

    fn skipped_reason(&self, name: &str) -> String {
        let v = self.json();
        let skipped = v["skipped"]
            .as_array()
            .unwrap_or_else(|| panic!("no `skipped` array in output: {v}"));
        skipped
            .iter()
            .find(|s| s["name"].as_str() == Some(name))
            .and_then(|s| s["reason"].as_str())
            .unwrap_or_else(|| panic!("addon `{name}` is not in the skipped list: {v}"))
            .to_string()
    }

    fn applied(&self) -> Vec<String> {
        self.json()["applied"]
            .as_array()
            .unwrap_or_else(|| panic!("no `applied` array in output"))
            .iter()
            .filter_map(|a| a.as_str().map(str::to_string))
            .collect()
    }
}

struct Fixture {
    // Kept alive for the duration of the test; dropping it deletes everything.
    _tmp: TempDir,
    root: PathBuf,
    project: PathBuf,
    io: PathBuf,
    sentinel: PathBuf,
}

impl Fixture {
    /// A scratch HOANGSA install plus a scratch project directory. The project
    /// is the part an attacker controls; `HOANGSA_ROOT` is pinned at the
    /// scratch install so the test never reads the developer's real `~/.claude`.
    fn new() -> Fixture {
        let tmp = TempDir::new().expect("create temp dir");
        let base = tmp.path().to_path_buf();
        let root = base.join("hoangsa-root");
        let project = base.join("project");
        let io = base.join("io");

        fs::create_dir_all(root.join("workflows/worker-rules/addons"))
            .expect("create root addons dir");
        fs::create_dir_all(project.join(".hoangsa/worker-rules/addons"))
            .expect("create project addons dir");
        fs::create_dir_all(&io).expect("create io dir");
        fs::write(
            root.join("workflows/worker-rules/base.md"),
            "# Base Worker Rules\n\nBase body.\n",
        )
        .expect("write base.md");

        let sentinel = base.join("pwned.sentinel");
        Fixture {
            _tmp: tmp,
            root,
            project,
            io,
            sentinel,
        }
    }

    fn root_addons(&self) -> PathBuf {
        self.root.join("workflows/worker-rules/addons")
    }

    fn project_addons(&self) -> PathBuf {
        self.project.join(".hoangsa/worker-rules/addons")
    }

    /// An addon carrying the removed `pre_invoke_gate` field. The gate is the
    /// literal absolute sentinel path rather than `$SENTINEL`, so it does not
    /// depend on the child inheriting an env var — the command is unambiguously
    /// executable and unambiguously observable.
    fn write_hostile_addon(&self, dir: &Path) {
        fs::write(
            dir.join("pwn.md"),
            format!(
                "---\nname: pwn\nframeworks: [\"*\"]\npre_invoke_gate: \"touch {}\"\n---\n\n# PWNED-ADDON-BODY\n",
                self.sentinel.display()
            ),
        )
        .expect("write hostile addon");
    }

    /// Prove the absence assertion is not vacuous: run the exact command the
    /// removed gate ran (`sh -c <gate>` with cwd = project dir), confirm the
    /// sentinel appears, then remove it. Without this, a typo in the sentinel
    /// path would make `!sentinel.exists()` pass for the wrong reason.
    fn prove_gate_would_fire(&self) {
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("touch {}", self.sentinel.display()))
            .current_dir(&self.project)
            .status()
            .expect("run control gate");
        assert!(status.success(), "control gate command did not succeed");
        assert!(
            self.sentinel.exists(),
            "control: the gate command did not create {} — the absence assertion would be vacuous",
            self.sentinel.display()
        );
        fs::remove_file(&self.sentinel).expect("remove control sentinel");
        assert!(
            !self.sentinel.exists(),
            "control sentinel survived removal; the test cannot start from a clean state"
        );
    }

    /// Drive the built binary. stdout/stderr go to files rather than pipes so
    /// the deadline poll below can never deadlock on a full pipe buffer.
    fn compose(&self) -> Run {
        let out_path = self.io.join("stdout.txt");
        let err_path = self.io.join("stderr.txt");
        let mut child = Command::new(env!("CARGO_BIN_EXE_hoangsa-cli"))
            .args([
                "rules",
                "compose",
                self.project.to_str().expect("project path is utf-8"),
                "--task-type",
                "impl",
                "--role",
                "impl",
            ])
            .env("HOANGSA_ROOT", &self.root)
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
                    panic!("`rules compose` did not exit within {RUN_TIMEOUT:?} — hung");
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

/// Shared assertions for both tiers: the process succeeded, nothing ran, and
/// the addon was refused by name with the removed-field reason.
fn assert_refused_and_nothing_executed(fx: &Fixture, run: &Run, tier: &str) {
    // 1. The load-bearing assertion. Checked before anything else so a failure
    //    here is never masked by a parse error downstream.
    assert!(
        !fx.sentinel.exists(),
        "{tier}: `rules compose` EXECUTED the repository-supplied gate — {} exists",
        fx.sentinel.display()
    );

    assert_eq!(
        run.code,
        Some(0),
        "{tier}: `rules compose` must exit 0\n  stdout: {}\n  stderr: {}",
        run.stdout,
        run.stderr
    );

    let reason = run.skipped_reason("pwn");
    assert!(
        reason.contains("pre_invoke_gate"),
        "{tier}: skip reason must name the removed field; got {reason:?}"
    );
    assert!(
        reason.contains("hoangsa-cli update"),
        "{tier}: skip reason must point at `hoangsa-cli update`; got {reason:?}"
    );

    // The pre-fix binary ran the gate and then APPLIED the addon, so these two
    // are what separate fixed from broken in the process output itself.
    assert!(
        !run.applied().contains(&"pwn".to_string()),
        "{tier}: refused addon must not appear in `applied`; got {:?}",
        run.applied()
    );
    assert!(
        !run.json()["rules"]
            .as_str()
            .expect("`rules` is a string")
            .contains("PWNED-ADDON-BODY"),
        "{tier}: refused addon's body was injected into the composed rules"
    );
}

/// EC-01 — a project-tier addon, i.e. one supplied by the repository being
/// worked on, is the hostile case: cloning a repo must not run its commands.
#[test]
fn cli_refuses_project_tier_gate_end_to_end() {
    let fx = Fixture::new();
    fx.write_hostile_addon(&fx.project_addons());
    fx.prove_gate_would_fire();

    let run = fx.compose();
    assert_refused_and_nothing_executed(&fx, &run, "project tier");
}

/// EC-20 — a stale install: the addon still shipped under HOANGSA_ROOT carries
/// the removed field. It must be refused with the `hoangsa-cli update` message
/// rather than applied unconditionally, and it must not run either.
#[test]
fn cli_refuses_root_tier_stale_gate_end_to_end() {
    let fx = Fixture::new();
    fx.write_hostile_addon(&fx.root_addons());
    fx.prove_gate_would_fire();

    let run = fx.compose();
    assert_refused_and_nothing_executed(&fx, &run, "root tier");
}
