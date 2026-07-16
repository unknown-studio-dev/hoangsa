use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Read and parse a JSON file, returning an error object on failure.
/// Error messages include the file path and distinguish file-not-found from parse errors.
pub fn read_json(file_path: &str) -> Value {
    if !Path::new(file_path).exists() {
        return serde_json::json!({ "error": format!("File not found: {}", file_path) });
    }
    match fs::read_to_string(file_path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => serde_json::json!({ "error": format!("Invalid JSON in {}: {}", file_path, e) }),
        },
        Err(e) => serde_json::json!({ "error": format!("Cannot read {}: {}", file_path, e) }),
    }
}

/// Read a file, returning None on failure.
pub fn read_file(file_path: &str) -> Option<String> {
    fs::read_to_string(file_path).ok()
}

/// Print a JSON value to stdout with 2-space indentation.
///
/// When the `hook codex` dispatcher has set an active Codex event, the
/// value is first translated from the Claude hook shape to valid Codex
/// wire (see `codex_wire::translate`) so handlers stay harness-agnostic.
pub fn out(obj: &Value) {
    match crate::codex_wire::active_event() {
        Some(event) => println!(
            "{}",
            serde_json::to_string_pretty(&crate::codex_wire::translate(event, obj)).unwrap()
        ),
        None => println!("{}", serde_json::to_string_pretty(obj).unwrap()),
    }
}

pub const ERR_READ_CONFIG: &str = "Cannot read config.json";

/// Unwrap a required CLI argument; on None, emit the standard error JSON.
/// Call sites: `let Some(dir) = require_arg(project_dir, "projectDir") else { return };`
pub fn require_arg<'a>(val: Option<&'a str>, name: &str) -> Option<&'a str> {
    if val.is_none() {
        out(&serde_json::json!({ "error": format!("{name} is required") }));
    }
    val
}

/// Parse YAML frontmatter from markdown content.
/// Returns a map of key-value pairs, or None if no frontmatter found.
///
/// Expects format:
/// ```text
/// ---
/// key: value
/// key: "quoted value"
/// ---
/// ```
pub fn parse_frontmatter(content: &str) -> Option<BTreeMap<String, String>> {
    // Strip optional \r for Windows line endings
    let s = content
        .strip_prefix("---\r\n")
        .or_else(|| content.strip_prefix("---\n"))?;
    let end = s.find("\n---").or_else(|| s.find("\r\n---"))?;
    let block = &s[..end];

    let mut fm = BTreeMap::new();
    for line in block.lines() {
        let line = line.trim_end();
        // Find the colon separator
        let colon = match line.find(':') {
            Some(i) => i,
            None => continue,
        };
        let key = &line[..colon];
        // Key must start with word char and contain only word chars/underscores
        if key.is_empty()
            || !key.chars().next().unwrap().is_alphanumeric()
            || !key.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let val = line[colon + 1..].trim();
        // Strip surrounding quotes if present
        let val = val
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(val);
        fm.insert(key.to_string(), val.trim().to_string());
    }
    Some(fm)
}

/// Resolve the working directory from --cwd flag or current directory,
/// then walk up to find the hoangsa project root (folder containing
/// `.hoangsa/config.json`). Stops at `$HOME` to avoid treating the user's
/// home directory as a project root. Falls back to the raw cwd when no
/// project root is found.
///
/// Rejects non-absolute `--cwd` paths and paths that don't exist.
pub fn resolve_cwd(args: &[String]) -> String {
    let raw = raw_cwd(args);
    match find_project_root(Path::new(&raw)) {
        Some(root) => root.to_string_lossy().to_string(),
        None => raw,
    }
}

fn raw_cwd(args: &[String]) -> String {
    for i in 0..args.len() {
        if args[i] == "--cwd"
            && let Some(dir) = args.get(i + 1) {
                let p = Path::new(dir);
                if !p.is_absolute() {
                    eprintln!("Warning: --cwd must be an absolute path, ignoring: {dir}");
                } else if let Ok(canonical) = std::fs::canonicalize(p) {
                    return canonical.to_string_lossy().to_string();
                } else {
                    eprintln!("Warning: --cwd path does not exist, ignoring: {dir}");
                }
            }
    }
    std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

/// Walk up from `start` looking for `.hoangsa/config.json`. Stops at
/// `$HOME` so the global hoangsa data dir isn't mistaken for a project.
/// Returns `None` when no marker is found before the boundary.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut current = Some(start);
    while let Some(dir) = current {
        if let Some(h) = &home
            && dir == h.as_path()
        {
            return None;
        }
        if dir.join(".hoangsa").join("config.json").is_file() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// Check if a path is absolute.
pub fn is_absolute(p: &str) -> bool {
    Path::new(p).is_absolute()
}

/// Count tokens using tiktoken-rs cl100k_base encoding.
/// Falls back to len/4 if tiktoken init fails.
pub fn count_tokens(text: &str) -> u64 {
    match tiktoken_rs::cl100k_base() {
        Ok(bpe) => bpe.encode_with_special_tokens(text).len() as u64,
        Err(_) => text.len() as u64 / 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_tokens_nonempty() {
        let n = count_tokens("Hello, world!");
        assert!(n > 0, "expected non-zero token count for non-empty string");
    }

    #[test]
    fn test_count_tokens_empty() {
        assert_eq!(count_tokens(""), 0, "empty string should yield 0 tokens");
    }

    #[test]
    fn find_project_root_returns_marker_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".hoangsa")).unwrap();
        fs::write(root.join(".hoangsa/config.json"), "{}").unwrap();
        let nested = root.join("crates/foo/src");
        fs::create_dir_all(&nested).unwrap();
        let found = find_project_root(&nested).unwrap();
        assert_eq!(
            fs::canonicalize(&found).unwrap(),
            fs::canonicalize(root).unwrap()
        );
    }

    #[test]
    fn find_project_root_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        assert!(find_project_root(&nested).is_none());
    }

    #[test]
    fn find_project_root_stops_at_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join(".hoangsa")).unwrap();
        // Simulate the global hoangsa data dir at $HOME — must NOT be
        // returned as a project root.
        fs::write(home.join(".hoangsa/config.json"), "{}").unwrap();
        let inner = home.join("Desktop/some-uninit-project");
        fs::create_dir_all(&inner).unwrap();

        let prev_home = std::env::var_os("HOME");
        // SAFETY: tests in this crate run on a single thread for env access;
        // we restore the previous value below.
        unsafe { std::env::set_var("HOME", home) };
        let found = find_project_root(&inner);
        match prev_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        assert!(
            found.is_none(),
            "must not climb into $HOME's .hoangsa, got {found:?}"
        );
    }
}
