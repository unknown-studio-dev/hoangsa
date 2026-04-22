//! Post-install auto-bootstrap for hoangsa-memory.
//!
//! Runs once per project, lazy — kicked off by the SessionStart hook on
//! the first Claude Code open of a project. Handles the steps a user
//! would otherwise have to type by hand:
//!
//!   1. `hoangsa-memory index <project>` — parse + index source
//!   2. `hoangsa-memory archive ingest --project <slug>` — backfill
//!      past Claude transcripts for this project
//!   3. `hoangsa-memory init` — seed MEMORY.md / LESSONS.md / USER.md
//!      skeleton if missing
//!
//! Progress is surfaced via `bootstrap.state` (read by the statusline);
//! a `.bootstrap-done` sentinel makes subsequent SessionStart fires
//! short-circuit in <100 ms. See
//! `.hoangsa/sessions/brainstorm/post-install-onboarding/BRAINSTORM.md`
//! for the design rationale.

use crate::helpers::out;
use serde_json::{Value, json};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PHASE_INDEXING: &str = "indexing";
pub const PHASE_INGESTING: &str = "ingesting";
pub const PHASE_SEEDING: &str = "seeding";
pub const PHASE_DONE: &str = "done";
pub const PHASE_ERROR: &str = "error";

/// Worker is considered stale (presumed crashed) when its state file's
/// `updated_at_epoch` hasn't advanced in this many seconds and the
/// phase is not terminal. Indexing large repos can take minutes between
/// phase transitions — keep the threshold generous to avoid false
/// restarts during legitimate long runs.
const STALE_THRESHOLD_SECS: u64 = 30 * 60;

// ── path helpers ────────────────────────────────────────────────────────────

/// Last-two-path-components slug (lowercased, non-alnum → '-'). Kept in
/// sync with `hook::project_slug` / `hoangsa-memory-mcp::main::project_slug`
/// so the per-project memory dir we write agrees with the one the MCP
/// daemon reads.
pub fn project_slug(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let components: Vec<&str> = canonical
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    let n = components.len();
    let parts = if n >= 2 {
        &components[n - 2..]
    } else {
        &components[..]
    };
    let raw = parts.join("-");
    let mut result = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for c in raw.chars().flat_map(|c| c.to_lowercase()) {
        if c.is_ascii_alphanumeric() {
            result.push(c);
            prev_dash = false;
        } else if !prev_dash {
            result.push('-');
            prev_dash = true;
        }
    }
    result.trim_matches('-').to_string()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn project_memory_dir(cwd: &Path) -> Option<PathBuf> {
    Some(
        home_dir()?
            .join(".hoangsa")
            .join("memory")
            .join("projects")
            .join(project_slug(cwd)),
    )
}

pub fn state_path(cwd: &Path) -> Option<PathBuf> {
    project_memory_dir(cwd).map(|d| d.join("bootstrap.state"))
}

pub fn sentinel_path(cwd: &Path) -> Option<PathBuf> {
    project_memory_dir(cwd).map(|d| d.join(".bootstrap-done"))
}

fn log_dir() -> Option<PathBuf> {
    Some(home_dir()?.join(".hoangsa").join("logs"))
}

// ── opt-out checks ──────────────────────────────────────────────────────────

pub fn project_opt_out(cwd: &Path) -> bool {
    cwd.join(".hoangsa").join("skip-bootstrap").exists()
}

pub fn env_opt_out() -> bool {
    std::env::var("HOANGSA_NO_BOOTSTRAP")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Default (missing file / missing key) leaves auto-bootstrap ON — only
/// an explicit `false` opts out.
pub fn config_opt_out() -> bool {
    let Some(cfg) = home_dir().map(|h| h.join(".hoangsa").join("config.json")) else {
        return false;
    };
    let Ok(raw) = fs::read_to_string(&cfg) else {
        return false;
    };
    let Ok(v): Result<Value, _> = serde_json::from_str(&raw) else {
        return false;
    };
    matches!(v.get("auto_bootstrap").and_then(|x| x.as_bool()), Some(false))
}

pub fn opt_out_reason(cwd: &Path) -> Option<&'static str> {
    if project_opt_out(cwd) {
        return Some("project_file");
    }
    if env_opt_out() {
        return Some("env");
    }
    if config_opt_out() {
        return Some("config");
    }
    None
}

// ── state file ──────────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_iso() -> String {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

fn iso_from_epoch(epoch: u64) -> String {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;
    OffsetDateTime::from_unix_timestamp(epoch as i64)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default()
}

pub fn read_state(cwd: &Path) -> Option<Value> {
    let p = state_path(cwd)?;
    let raw = fs::read_to_string(&p).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_state_atomic(cwd: &Path, state: &Value) -> io::Result<()> {
    let p = state_path(cwd)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no memory root (HOME unset?)"))?;
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = p.with_file_name("bootstrap.state.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(state).unwrap_or_default())?;
    fs::rename(&tmp, &p)?;
    Ok(())
}

/// True iff phase is non-terminal AND `updated_at_epoch` is within
/// `STALE_THRESHOLD_SECS`. A stale file implies the worker crashed.
pub fn state_is_active(state: &Value) -> bool {
    let phase = state.get("phase").and_then(|p| p.as_str()).unwrap_or("");
    if phase == PHASE_DONE || phase == PHASE_ERROR || phase.is_empty() {
        return false;
    }
    let updated = state
        .get("updated_at_epoch")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if updated == 0 {
        return false;
    }
    now_secs().saturating_sub(updated) < STALE_THRESHOLD_SECS
}

fn build_state(phase: &str, pct: u32, started_epoch: u64, error: Option<&str>) -> Value {
    let mut v = json!({
        "phase": phase,
        "pct": pct,
        "started_at": iso_from_epoch(started_epoch),
        "started_at_epoch": started_epoch,
        "updated_at": now_iso(),
        "updated_at_epoch": now_secs(),
        "pid": std::process::id(),
    });
    if let Some(e) = error {
        v["error"] = Value::String(e.into());
    }
    v
}

// ── sentinel + decision ─────────────────────────────────────────────────────

pub fn sentinel_exists(cwd: &Path) -> bool {
    sentinel_path(cwd).map(|p| p.exists()).unwrap_or(false)
}

/// Hook-side decision: should we spawn the worker right now?
///
/// Returns `Err("reason")` when NOT spawning (opt_out:<layer> / done /
/// running) so the caller can log what short-circuited.
pub fn should_bootstrap(cwd: &Path) -> Result<(), String> {
    if let Some(layer) = opt_out_reason(cwd) {
        return Err(format!("opt_out:{layer}"));
    }
    if sentinel_exists(cwd) {
        return Err("done".into());
    }
    if let Some(state) = read_state(cwd)
        && state_is_active(&state)
    {
        return Err("running".into());
    }
    Ok(())
}

// ── memory binary lookup (mirrors hook::find_memory_bin) ────────────────────

fn find_bin_in_path(stem: &str) -> Option<String> {
    let path_var = std::env::var("PATH").ok()?;
    let sep = if cfg!(windows) { ';' } else { ':' };
    let names: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ""]
    } else {
        &[""]
    };
    for dir in path_var.split(sep) {
        for suffix in names {
            let name = format!("{stem}{suffix}");
            let candidate = Path::new(dir).join(&name);
            if candidate.exists() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn find_memory_bin() -> Option<String> {
    if let Some(p) = find_bin_in_path("hoangsa-memory") {
        return Some(p);
    }
    let home = home_dir()?;
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let candidate = home
        .join(".hoangsa")
        .join("bin")
        .join(format!("hoangsa-memory{suffix}"));
    if candidate.exists() {
        return Some(candidate.to_string_lossy().to_string());
    }
    None
}

fn find_cli_bin() -> Option<String> {
    // Prefer the currently-running binary so the spawned worker is the
    // same version as the hook we ran from.
    if let Ok(p) = std::env::current_exe()
        && p.exists()
    {
        return Some(p.to_string_lossy().to_string());
    }
    if let Some(p) = find_bin_in_path("hoangsa-cli") {
        return Some(p);
    }
    let home = home_dir()?;
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let candidate = home
        .join(".hoangsa")
        .join("bin")
        .join(format!("hoangsa-cli{suffix}"));
    if candidate.exists() {
        return Some(candidate.to_string_lossy().to_string());
    }
    None
}

// ── detached spawn ──────────────────────────────────────────────────────────

/// Fire-and-forget launch of `hoangsa-cli bootstrap --project <cwd>`.
/// The child is detached (null stdio) — the hook parent exits almost
/// immediately and the worker survives via reparent-to-init.
pub fn spawn_detached_worker(cwd: &Path) -> bool {
    let Some(cli) = find_cli_bin() else {
        return false;
    };
    let cwd_str = cwd.to_string_lossy().to_string();
    Command::new(cli)
        .args(["bootstrap", "--project", &cwd_str])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

// ── logging ─────────────────────────────────────────────────────────────────

fn append_log(line: &str) {
    let Some(dir) = log_dir() else { return };
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    use time::OffsetDateTime;
    use time::format_description::well_known::Iso8601;
    let date = OffsetDateTime::now_utc()
        .format(&Iso8601::DATE)
        .unwrap_or_else(|_| "unknown".into());
    let file = dir.join(format!("bootstrap-{date}.log"));
    let stamp = now_iso();
    let line = format!("{stamp} {line}\n");
    use std::io::Write;
    if let Ok(mut f) = fs::OpenOptions::new().append(true).create(true).open(&file) {
        let _ = f.write_all(line.as_bytes());
    }
}

// ── CLI entry: `hoangsa-cli bootstrap` ──────────────────────────────────────

struct BootstrapArgs {
    project: PathBuf,
    force: bool,
    json: bool,
}

fn parse_args(rest: &[&str], default_cwd: &str) -> Result<BootstrapArgs, String> {
    let mut project: Option<PathBuf> = None;
    let mut force = false;
    let mut json = false;

    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        match a {
            "--force" => force = true,
            "--json" => json = true,
            "--project" => {
                let v = rest
                    .get(i + 1)
                    .ok_or_else(|| "--project requires a value".to_string())?;
                project = Some(PathBuf::from(v));
                i += 1;
            }
            other if other.starts_with("--project=") => {
                project = Some(PathBuf::from(other.trim_start_matches("--project=")));
            }
            other => return Err(format!("unknown bootstrap flag: {other}")),
        }
        i += 1;
    }

    Ok(BootstrapArgs {
        project: project.unwrap_or_else(|| PathBuf::from(default_cwd)),
        force,
        json,
    })
}

pub fn cmd_bootstrap(rest: &[&str], cwd: &str) {
    let args = match parse_args(rest, cwd) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("bootstrap: {e}");
            std::process::exit(2);
        }
    };

    if !args.force {
        if let Some(layer) = opt_out_reason(&args.project) {
            emit_result(
                args.json,
                "skipped",
                &format!("opt_out:{layer}"),
                &args.project,
                None,
            );
            return;
        }
        if sentinel_exists(&args.project) {
            emit_result(args.json, "skipped", "already-done", &args.project, None);
            return;
        }
    }

    match run_bootstrap(&args.project) {
        Ok(()) => emit_result(args.json, "ok", "done", &args.project, None),
        Err(e) => {
            eprintln!("bootstrap: {e}");
            emit_result(args.json, "error", "worker-failed", &args.project, Some(&e));
            std::process::exit(1);
        }
    }
}

fn emit_result(json_out: bool, status: &str, reason: &str, project: &Path, err: Option<&str>) {
    if json_out {
        let mut v = json!({
            "status": status,
            "reason": reason,
            "project": project.display().to_string(),
        });
        if let Some(e) = err {
            v["error"] = Value::String(e.into());
        }
        out(&v);
    } else {
        match status {
            "ok" => println!("hoangsa bootstrap: done ({})", project.display()),
            "skipped" => println!("hoangsa bootstrap: skipped — {reason}"),
            _ => eprintln!(
                "hoangsa bootstrap: {status} — {reason}{}",
                err.map(|e| format!(": {e}")).unwrap_or_default()
            ),
        }
    }
}

/// Real worker. Each phase transition writes the state file atomically
/// and appends a log line; on failure we mark the state as `error`.
fn run_bootstrap(project: &Path) -> Result<(), String> {
    let memory_bin = find_memory_bin()
        .ok_or_else(|| "hoangsa-memory binary not found on PATH or in ~/.hoangsa/bin".to_string())?;

    append_log(&format!(
        "start project={} pid={} memory_bin={memory_bin}",
        project.display(),
        std::process::id()
    ));

    let started = now_secs();

    // Phase 1: index source. Fatal on failure — nothing useful below
    // can land without the code graph.
    write_state(project, PHASE_INDEXING, 0, started, None);
    match Command::new(&memory_bin)
        .arg("index")
        .arg(project)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(s) if s.success() => {}
        Ok(s) => {
            let msg = format!("index exited with {s}");
            write_state(project, PHASE_ERROR, 0, started, Some(&msg));
            append_log(&format!("index failed: {msg}"));
            return Err(msg);
        }
        Err(e) => {
            let msg = format!("failed to spawn `hoangsa-memory index`: {e}");
            write_state(project, PHASE_ERROR, 0, started, Some(&msg));
            append_log(&format!("index spawn failed: {msg}"));
            return Err(msg);
        }
    }
    append_log("index done");

    // Phase 2: archive ingest for THIS project. A non-zero exit here
    // doesn't abort — index already landed, user can still get code
    // recall; just log and continue so seeding runs.
    write_state(project, PHASE_INGESTING, 50, started, None);
    let slug = project_slug(project);
    match Command::new(&memory_bin)
        .args(["archive", "ingest", "--project", &slug])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(s) if s.success() => {}
        Ok(s) => append_log(&format!("archive ingest exited with {s} (continuing)")),
        Err(e) => append_log(&format!("archive ingest spawn failed: {e} (continuing)")),
    }
    append_log("ingest done");

    // Phase 3: seed MEMORY.md / LESSONS.md / USER.md (idempotent).
    write_state(project, PHASE_SEEDING, 90, started, None);
    if let Err(e) = Command::new(&memory_bin)
        .arg("init")
        .current_dir(project)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        append_log(&format!("init spawn failed: {e} (continuing)"));
    }
    append_log("seed done");

    if let Some(sentinel) = sentinel_path(project) {
        if let Some(parent) = sentinel.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&sentinel, "");
    }
    write_state(project, PHASE_DONE, 100, started, None);
    append_log(&format!("complete project={}", project.display()));
    Ok(())
}

fn write_state(project: &Path, phase: &str, pct: u32, started: u64, error: Option<&str>) {
    let state = build_state(phase, pct, started, error);
    if let Err(e) = write_state_atomic(project, &state) {
        append_log(&format!("write_state_atomic failed: {e}"));
    }
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Serialize every test in this module that mutates `HOME` or
    /// `HOANGSA_NO_BOOTSTRAP`. Cargo runs unit tests in parallel threads
    /// within a single process, so env-var writes would otherwise race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Scrub the env vars we read to a clean baseline, hold the lock
    /// for the caller's closure, then restore prior values on drop.
    fn with_clean_env<F: FnOnce()>(home: Option<&Path>, f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_home = std::env::var_os("HOME");
        let prev_optout = std::env::var_os("HOANGSA_NO_BOOTSTRAP");
        // SAFETY: guarded by ENV_LOCK — single-threaded within this section.
        unsafe {
            match home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
            std::env::remove_var("HOANGSA_NO_BOOTSTRAP");
        }
        f();
        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prev_optout {
                Some(v) => std::env::set_var("HOANGSA_NO_BOOTSTRAP", v),
                None => std::env::remove_var("HOANGSA_NO_BOOTSTRAP"),
            }
        }
    }

    fn with_home<F: FnOnce()>(home: &Path, f: F) {
        with_clean_env(Some(home), f);
    }

    #[test]
    fn opt_out_project_file_wins() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path();
        fs::create_dir_all(cwd.join(".hoangsa")).unwrap();
        fs::write(cwd.join(".hoangsa").join("skip-bootstrap"), "").unwrap();
        assert_eq!(opt_out_reason(cwd), Some("project_file"));
    }

    #[test]
    fn opt_out_env_triggers() {
        let tmp = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        with_clean_env(Some(home.path()), || {
            // SAFETY: guarded by ENV_LOCK via with_clean_env.
            unsafe {
                std::env::set_var("HOANGSA_NO_BOOTSTRAP", "1");
            }
            assert!(env_opt_out());
            assert_eq!(opt_out_reason(tmp.path()), Some("env"));
        });
    }

    #[test]
    fn opt_out_config_default_is_off() {
        let home = TempDir::new().unwrap();
        with_home(home.path(), || {
            assert!(!config_opt_out());
        });
    }

    #[test]
    fn opt_out_config_explicit_false() {
        let home = TempDir::new().unwrap();
        fs::create_dir_all(home.path().join(".hoangsa")).unwrap();
        fs::write(
            home.path().join(".hoangsa").join("config.json"),
            r#"{"auto_bootstrap": false}"#,
        )
        .unwrap();
        with_home(home.path(), || {
            assert!(config_opt_out());
        });
    }

    #[test]
    fn opt_out_config_true_is_not_opt_out() {
        let home = TempDir::new().unwrap();
        fs::create_dir_all(home.path().join(".hoangsa")).unwrap();
        fs::write(
            home.path().join(".hoangsa").join("config.json"),
            r#"{"auto_bootstrap": true}"#,
        )
        .unwrap();
        with_home(home.path(), || {
            assert!(!config_opt_out());
        });
    }

    #[test]
    fn state_is_active_detects_fresh_non_terminal() {
        let state = json!({
            "phase": "indexing",
            "updated_at_epoch": now_secs(),
        });
        assert!(state_is_active(&state));
    }

    #[test]
    fn state_is_active_rejects_terminal() {
        for phase in [PHASE_DONE, PHASE_ERROR] {
            let state = json!({
                "phase": phase,
                "updated_at_epoch": now_secs(),
            });
            assert!(!state_is_active(&state), "{phase} must not be active");
        }
    }

    #[test]
    fn state_is_active_rejects_stale() {
        let stale = now_secs().saturating_sub(STALE_THRESHOLD_SECS + 60);
        let state = json!({
            "phase": "indexing",
            "updated_at_epoch": stale,
        });
        assert!(!state_is_active(&state));
    }

    #[test]
    fn should_bootstrap_respects_sentinel() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        with_home(home.path(), || {
            let sentinel = sentinel_path(project.path()).unwrap();
            fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
            fs::write(&sentinel, "").unwrap();
            assert_eq!(should_bootstrap(project.path()), Err("done".into()));
        });
    }

    #[test]
    fn should_bootstrap_ok_on_clean_slate() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        with_home(home.path(), || {
            assert!(should_bootstrap(project.path()).is_ok());
        });
    }

    #[test]
    fn slug_uses_last_two_components() {
        let slug = project_slug(Path::new("/Users/alice/Projects/My Repo"));
        assert!(
            slug.contains("projects") && slug.contains("my-repo"),
            "got {slug}"
        );
    }

    #[test]
    fn parse_args_defaults() {
        let a = parse_args(&[], "/tmp/x").unwrap();
        assert_eq!(a.project, PathBuf::from("/tmp/x"));
        assert!(!a.force);
        assert!(!a.json);
    }

    #[test]
    fn parse_args_full() {
        let a = parse_args(&["--project", "/tmp/foo", "--force", "--json"], "/unused").unwrap();
        assert_eq!(a.project, PathBuf::from("/tmp/foo"));
        assert!(a.force);
        assert!(a.json);
    }

    #[test]
    fn parse_args_equals_form() {
        let a = parse_args(&["--project=/tmp/bar"], "/unused").unwrap();
        assert_eq!(a.project, PathBuf::from("/tmp/bar"));
    }

    #[test]
    fn parse_args_rejects_unknown() {
        assert!(parse_args(&["--weird"], "/x").is_err());
    }
}
