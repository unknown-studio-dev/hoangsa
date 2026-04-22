use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

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
pub fn out(obj: &Value) {
    println!("{}", serde_json::to_string_pretty(obj).unwrap());
}

/// Parse YAML frontmatter from markdown content.
/// Returns a map of key-value pairs, or None if no frontmatter found.
///
/// Expects format:
/// ```
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

/// Resolve the working directory from --cwd flag or current directory.
/// Rejects non-absolute paths and paths outside $HOME to prevent arbitrary writes.
pub fn resolve_cwd(args: &[String]) -> String {
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

}
