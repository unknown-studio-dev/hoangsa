//! Layered runtime preferences.
//!
//! Precedence (lowest → highest):
//!   1. Built-in defaults
//!   2. Global config `~/.hoangsa/proxy/config.toml`
//!   3. Project config `<cwd>/.hoangsa/proxy/config.toml`
//!   4. Env vars (`HSP_STRICT`)
//!   5. CLI flags (`--strict`)
//!
//! Parse failures never abort the proxy — we emit a `[hsp warn] event=
//! config_parse_error …` line and fall back to the lower-precedence layer.
//! A broken config must never prevent a command from running.
//!
//! Schema:
//! ```toml
//! [runtime]
//! strict = false                # default lossless-only mode
//! max_output_mb = 100           # per-stream cap (1..=1024)
//!
//! [handlers]
//! disabled = ["cargo", "find"]  # skip these built-ins; Rhai still runs
//! ```

use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Prefs {
    pub strict: bool,
    pub max_output_mb: Option<u64>,
    pub disabled_handlers: Vec<String>,
    /// Warnings encountered while loading. Surfaced in the trim report so
    /// the user knows their config was (partially) ignored.
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawConfig {
    #[serde(default)]
    runtime: RawRuntime,
    #[serde(default)]
    handlers: RawHandlers,
}

#[derive(Debug, Deserialize, Default)]
struct RawRuntime {
    strict: Option<bool>,
    max_output_mb: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
struct RawHandlers {
    #[serde(default)]
    disabled: Vec<String>,
}

impl Prefs {
    /// Load from `global` then overlay `project`. Either path may be absent;
    /// parse errors produce warnings, not failures.
    pub fn load(project_cwd: &Path, global_override: Option<&Path>) -> Self {
        let mut prefs = Self::default();
        if let Some(g) = global_config_path_with_override(global_override) {
            prefs.overlay_file(&g);
        }
        prefs.overlay_file(&project_cwd.join(".hoangsa/proxy/config.toml"));
        prefs
    }

    fn overlay_file(&mut self, path: &Path) {
        let text = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                self.warnings.push(format!(
                    "event=config_read_error path={} error={e}",
                    path.display()
                ));
                return;
            }
        };
        let raw: RawConfig = match toml::from_str(&text) {
            Ok(r) => r,
            Err(e) => {
                self.warnings.push(format!(
                    "event=config_parse_error path={} error={}",
                    path.display().to_string().replace(' ', "_"),
                    e.message().replace(' ', "_"),
                ));
                return;
            }
        };
        if let Some(s) = raw.runtime.strict {
            self.strict = s;
        }
        if let Some(m) = raw.runtime.max_output_mb {
            if !(1..=1024).contains(&m) {
                self.warnings.push(format!(
                    "event=config_value_out_of_range path={} field=runtime.max_output_mb value={m}",
                    path.display().to_string().replace(' ', "_"),
                ));
            } else {
                self.max_output_mb = Some(m);
            }
        }
        if !raw.handlers.disabled.is_empty() {
            self.disabled_handlers = raw.handlers.disabled;
        }
    }

    pub fn is_handler_disabled(&self, cmd: &str) -> bool {
        self.disabled_handlers.iter().any(|d| d == cmd)
    }
}

fn global_config_path_with_override(override_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = override_path {
        return Some(p.to_path_buf());
    }
    dirs::home_dir().map(|d| d.join(".hoangsa").join("proxy").join("config.toml"))
}

/// Path where `hsp doctor` and `hsp init` resolve the project config. Kept
/// here so every caller agrees on the filename.
pub fn project_config_path(cwd: &Path) -> PathBuf {
    cwd.join(".hoangsa/proxy/config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn missing_config_gives_defaults() {
        let tmp = TempDir::new().unwrap();
        let p = Prefs::load(tmp.path(), Some(&tmp.path().join("nowhere.toml")));
        assert!(!p.strict);
        assert_eq!(p.max_output_mb, None);
        assert!(p.disabled_handlers.is_empty());
        assert!(p.warnings.is_empty());
    }

    #[test]
    fn project_config_loads() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            ".hoangsa/proxy/config.toml",
            "[runtime]\nstrict = true\nmax_output_mb = 50\n\n[handlers]\ndisabled = [\"cargo\"]\n",
        );
        let p = Prefs::load(tmp.path(), Some(&tmp.path().join("nowhere.toml")));
        assert!(p.strict);
        assert_eq!(p.max_output_mb, Some(50));
        assert_eq!(p.disabled_handlers, vec!["cargo".to_string()]);
    }

    #[test]
    fn project_overrides_global() {
        let tmp = TempDir::new().unwrap();
        let global_path = tmp.path().join("global.toml");
        fs::write(
            &global_path,
            "[runtime]\nstrict = false\nmax_output_mb = 100\n",
        )
        .unwrap();
        write(
            tmp.path(),
            ".hoangsa/proxy/config.toml",
            "[runtime]\nstrict = true\n",
        );
        let p = Prefs::load(tmp.path(), Some(&global_path));
        assert!(p.strict);
        // Project didn't set max_output_mb → global value carries through.
        assert_eq!(p.max_output_mb, Some(100));
    }

    #[test]
    fn parse_error_warns_not_fatal() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), ".hoangsa/proxy/config.toml", "not = valid = toml");
        let p = Prefs::load(tmp.path(), Some(&tmp.path().join("nowhere.toml")));
        assert!(!p.strict);
        assert!(
            p.warnings.iter().any(|w| w.contains("config_parse_error")),
            "warnings: {:?}",
            p.warnings
        );
    }

    #[test]
    fn out_of_range_max_bytes_warns() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            ".hoangsa/proxy/config.toml",
            "[runtime]\nmax_output_mb = 99999\n",
        );
        let p = Prefs::load(tmp.path(), Some(&tmp.path().join("nowhere.toml")));
        assert_eq!(p.max_output_mb, None);
        assert!(
            p.warnings.iter().any(|w| w.contains("out_of_range")),
            "warnings: {:?}",
            p.warnings
        );
    }

    #[test]
    fn handler_disable_query() {
        let p = Prefs {
            disabled_handlers: vec!["cargo".into(), "find".into()],
            ..Default::default()
        };
        assert!(p.is_handler_disabled("cargo"));
        assert!(p.is_handler_disabled("find"));
        assert!(!p.is_handler_disabled("git"));
    }
}
