//! `hsp doctor` — self-check diagnostic.
//!
//! Emits one `[hsp check] item=<name> status=<ok|warn|fail> …` record per
//! check on stdout (doctor is an interactive debugging command, so stdout
//! is appropriate). Exits 0 when no check has `status=fail`, 1 otherwise.
//!
//! The record format is the same machine-parsable shape used by the trim
//! report, so an LLM reading the output can reuse one regex for both.

use crate::init::{self, HSP_MARKER, Scope};
use crate::prefs::Prefs;
use crate::registry;
use crate::rhai_engine::RhaiRuntime;
use crate::tty;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Check {
    pub item: String,
    pub status: Status,
    pub fields: Vec<(String, String)>,
}

impl Check {
    fn render(&self) -> String {
        let mut line = format!("[hsp check] item={} status={}", self.item, self.status.as_str());
        for (k, v) in &self.fields {
            line.push(' ');
            line.push_str(k);
            line.push('=');
            line.push_str(&quote_if_needed(v));
        }
        line
    }
}

fn quote_if_needed(s: &str) -> String {
    if s.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'') {
        format!("'{}'", s.replace('\'', "\\'"))
    } else if s.is_empty() {
        "''".to_string()
    } else {
        s.to_string()
    }
}

/// Run every check and return the collected results. Pure — no stdout side
/// effects so tests can inspect the vec directly.
pub fn run(cwd: &Path) -> Vec<Check> {
    let mut out = vec![
        check_version(),
        check_platform(),
        check_binary_path(),
        check_hook(Scope::Global, cwd),
        check_hook(Scope::Project, cwd),
        check_hook_rewrite_works(),
    ];
    out.extend(check_config(cwd));
    out.push(check_handlers(cwd));
    out
}

fn check_version() -> Check {
    Check {
        item: "version".into(),
        status: Status::Ok,
        fields: vec![("value".into(), env!("CARGO_PKG_VERSION").into())],
    }
}

fn check_platform() -> Check {
    Check {
        item: "platform".into(),
        status: Status::Ok,
        fields: vec![
            ("os".into(), std::env::consts::OS.into()),
            ("arch".into(), std::env::consts::ARCH.into()),
            ("tty_stdin".into(), tty::stdin_is_tty().to_string()),
            ("tty_stdout".into(), tty::stdout_is_tty().to_string()),
        ],
    }
}

fn check_binary_path() -> Check {
    match std::env::current_exe() {
        Ok(p) => Check {
            item: "binary_path".into(),
            status: Status::Ok,
            fields: vec![("path".into(), p.display().to_string())],
        },
        Err(e) => Check {
            item: "binary_path".into(),
            status: Status::Warn,
            fields: vec![("error".into(), e.to_string())],
        },
    }
}

fn check_hook(scope: Scope, cwd: &Path) -> Check {
    let name = match scope {
        Scope::Global => "hook_global",
        Scope::Project => "hook_project",
    };
    let Some(path) = init::settings_path(scope, cwd) else {
        return Check {
            item: name.into(),
            status: Status::Warn,
            fields: vec![("error".into(), "no_settings_dir".into())],
        };
    };
    if !path.exists() {
        return Check {
            item: name.into(),
            status: Status::Warn,
            fields: vec![
                ("installed".into(), "false".into()),
                ("reason".into(), "settings_missing".into()),
                ("path".into(), path.display().to_string()),
            ],
        };
    }
    let installed = is_hsp_hook_present(&path);
    Check {
        item: name.into(),
        status: if installed { Status::Ok } else { Status::Warn },
        fields: vec![
            ("installed".into(), installed.to_string()),
            ("path".into(), path.display().to_string()),
        ],
    }
}

fn is_hsp_hook_present(settings_path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(settings_path) else {
        return false;
    };
    raw.contains(HSP_MARKER)
}

/// Spawn our own binary with `hook rewrite`, feed a sample payload, and
/// verify we get back either `{}` or a JSON decision record. Round-trip
/// latency is included — useful for sanity-checking a daemon-less setup.
fn check_hook_rewrite_works() -> Check {
    let Ok(exe) = std::env::current_exe() else {
        return Check {
            item: "hook_rewrite".into(),
            status: Status::Fail,
            fields: vec![("error".into(), "current_exe_failed".into())],
        };
    };
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"git log -5"}}"#;
    let t = Instant::now();
    let mut child = match Command::new(&exe)
        .args(["hook", "rewrite"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return Check {
                item: "hook_rewrite".into(),
                status: Status::Fail,
                fields: vec![("error".into(), format!("spawn_failed:{e}"))],
            };
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.as_bytes());
    }
    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            return Check {
                item: "hook_rewrite".into(),
                status: Status::Fail,
                fields: vec![("error".into(), format!("wait_failed:{e}"))],
            };
        }
    };
    let elapsed = t.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // Must parse as JSON (either {} or the approve payload).
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
    let status = if output.status.success() && parsed.is_ok() {
        Status::Ok
    } else {
        Status::Fail
    };
    Check {
        item: "hook_rewrite".into(),
        status,
        fields: vec![
            ("latency_ms".into(), format!("{}", elapsed.as_millis())),
            ("exit".into(), format!("{}", output.status.code().unwrap_or(-1))),
            ("response_valid_json".into(), parsed.is_ok().to_string()),
        ],
    }
}

fn check_config(cwd: &Path) -> Vec<Check> {
    let prefs = Prefs::load(cwd, None);
    let mut checks = Vec::new();
    let status = if prefs.warnings.is_empty() {
        Status::Ok
    } else {
        Status::Warn
    };
    checks.push(Check {
        item: "config".into(),
        status,
        fields: vec![
            ("strict".into(), prefs.strict.to_string()),
            (
                "max_output_mb".into(),
                prefs
                    .max_output_mb
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "default".into()),
            ),
            ("disabled_handlers_count".into(), prefs.disabled_handlers.len().to_string()),
            ("warnings_count".into(), prefs.warnings.len().to_string()),
        ],
    });
    // Surface each warning as its own record so Claude can grep them.
    for w in prefs.warnings {
        checks.push(Check {
            item: "config_warning".into(),
            status: Status::Warn,
            fields: vec![("detail".into(), w)],
        });
    }
    checks
}

fn check_handlers(cwd: &Path) -> Check {
    let builtins = registry::builtins();
    let mut rt = RhaiRuntime::new();
    let project = crate::config::project_dir(cwd);
    let global = crate::config::global_dir();
    rt.load_dirs(&project, global.as_deref());
    let rhai_count = rt.handlers.lock().map(|h| h.len()).unwrap_or(0);
    Check {
        item: "handlers".into(),
        status: Status::Ok,
        fields: vec![
            ("builtin_count".into(), builtins.len().to_string()),
            ("rhai_count".into(), rhai_count.to_string()),
            ("project_rhai_dir".into(), project.display().to_string()),
            (
                "global_rhai_dir".into(),
                global
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "none".into()),
            ),
        ],
    }
}

/// Print every check to stdout and return an exit code.
pub fn print_and_exit_code(cwd: &Path) -> i32 {
    let checks = run(cwd);
    let mut has_fail = false;
    for c in &checks {
        if c.status == Status::Fail {
            has_fail = true;
        }
        println!("{}", c.render());
    }
    if has_fail { 1 } else { 0 }
}

// Keep unused-import lint happy when PathBuf isn't referenced above after
// refactors.
fn _use_pathbuf(_: PathBuf) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn version_check_always_ok() {
        let c = check_version();
        assert_eq!(c.status, Status::Ok);
        assert!(c.fields.iter().any(|(k, v)| k == "value" && !v.is_empty()));
    }

    #[test]
    fn platform_check_reports_fields() {
        let c = check_platform();
        assert_eq!(c.status, Status::Ok);
        let keys: Vec<&String> = c.fields.iter().map(|(k, _)| k).collect();
        assert!(keys.iter().any(|k| *k == "os"));
        assert!(keys.iter().any(|k| *k == "arch"));
    }

    #[test]
    fn hook_project_check_warn_when_absent() {
        let tmp = TempDir::new().unwrap();
        let c = check_hook(Scope::Project, tmp.path());
        assert_eq!(c.status, Status::Warn);
        assert!(c.fields.iter().any(|(k, v)| k == "installed" && v == "false"));
    }

    #[test]
    fn hook_project_check_ok_when_installed() {
        let tmp = TempDir::new().unwrap();
        // Fake an installed hook: write the marker string into settings.
        let settings = tmp.path().join(".claude/settings.local.json");
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::write(
            &settings,
            format!(
                r#"{{"hooks":{{"PreToolUse":[{{"hooks":[{{"command":"hsp hook rewrite {HSP_MARKER}"}}]}}]}}}}"#
            ),
        )
        .unwrap();
        let c = check_hook(Scope::Project, tmp.path());
        assert_eq!(c.status, Status::Ok);
    }

    #[test]
    fn config_warn_on_broken_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join(".hoangsa-proxy");
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("config.toml"), "not = valid = toml").unwrap();
        let checks = check_config(tmp.path());
        assert!(
            checks.iter().any(|c| c.item == "config_warning"),
            "expected config_warning record, got: {checks:?}"
        );
        let cfg = checks.iter().find(|c| c.item == "config").unwrap();
        assert_eq!(cfg.status, Status::Warn);
    }

    #[test]
    fn check_render_quotes_values_with_spaces() {
        let c = Check {
            item: "example".into(),
            status: Status::Ok,
            fields: vec![
                ("path".into(), "/a b/c".into()),
                ("plain".into(), "foo".into()),
            ],
        };
        let line = c.render();
        assert!(line.contains("path='/a b/c'"));
        assert!(line.contains("plain=foo"));
    }

    #[test]
    fn run_emits_core_checks() {
        let tmp = TempDir::new().unwrap();
        let checks = run(tmp.path());
        for expected in ["version", "platform", "binary_path", "hook_project", "config", "handlers"] {
            assert!(
                checks.iter().any(|c| c.item == expected),
                "missing {expected} in checks: {checks:?}"
            );
        }
    }
}
