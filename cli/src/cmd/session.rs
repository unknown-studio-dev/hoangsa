use crate::helpers::out;
use serde_json::json;
use std::fs;
use std::path::Path;

/// Slugify a name: lowercase, replace non-alphanumeric with hyphens, collapse runs, trim edges.
fn slugify(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let mut result = String::new();
    let mut prev_dash = true;
    for c in slug.chars() {
        if c == '-' {
            if !prev_dash {
                result.push('-');
            }
            prev_dash = true;
        } else {
            result.push(c);
            prev_dash = false;
        }
    }
    if result.ends_with('-') {
        result.pop();
    }
    result
}

const KNOWN_TYPES: &[&str] = &["feat", "fix", "refactor", "perf", "test", "docs", "chore", "brainstorm"];

/// `session init <type> <name> [sessions_dir]`
pub fn cmd_init(
    session_type: Option<&str>,
    name: Option<&str>,
    sessions_dir: Option<&str>,
    cwd: &str,
) {
    let session_type = match session_type {
        Some(t) if !t.is_empty() => t,
        _ => {
            out(
                &json!({ "error": "session type is required (feat|fix|refactor|perf|test|docs|chore)" }),
            );
            return;
        }
    };
    let name = match name {
        Some(n) if !n.is_empty() => n,
        _ => {
            out(&json!({ "error": "session name is required" }));
            return;
        }
    };

    let slug = slugify(name);
    if slug.is_empty() {
        out(&json!({ "error": "name produces empty slug after sanitization" }));
        return;
    }

    let dir = sessions_dir.map(|s| s.to_string()).unwrap_or_else(|| {
        Path::new(cwd)
            .join(".hoangsa")
            .join("sessions")
            .to_string_lossy()
            .to_string()
    });

    let session_dir = Path::new(&dir).join(session_type).join(&slug);
    if session_dir.exists() {
        let mut n = 2u32;
        loop {
            let deduped = Path::new(&dir)
                .join(session_type)
                .join(format!("{slug}-{n}"));
            if !deduped.exists() {
                let id = format!("{session_type}/{slug}-{n}");
                if let Err(e) = fs::create_dir_all(&deduped) {
                    out(&json!({ "error": format!("Cannot create session dir: {}", e) }));
                    return;
                }
                out(&json!({
                    "id": id,
                    "type": session_type,
                    "name": format!("{}-{}", slug, n),
                    "dir": deduped.to_string_lossy(),
                }));
                return;
            }
            n += 1;
            if n > 100 {
                out(&json!({ "error": "Too many sessions with the same name" }));
                return;
            }
        }
    }

    let id = format!("{session_type}/{slug}");
    if let Err(e) = fs::create_dir_all(&session_dir) {
        out(&json!({ "error": format!("Cannot create session dir: {}", e) }));
        return;
    }
    out(&json!({
        "id": id,
        "type": session_type,
        "name": slug,
        "dir": session_dir.to_string_lossy(),
    }));
}

struct SessionEntry {
    id: String,
    session_type: String,
    name: String,
    dir: String,
    mtime: std::time::SystemTime,
    files: Vec<String>,
}

/// Collect all sessions from sessions/<type>/<name>/ structure.
fn collect_sessions(sessions_root: &str) -> Vec<SessionEntry> {
    let mut sessions = Vec::new();
    let root = Path::new(sessions_root);

    let type_dirs = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return sessions,
    };

    for type_entry in type_dirs.filter_map(|e| e.ok()) {
        if !type_entry
            .file_type()
            .map(|ft| ft.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let type_name = match type_entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if !KNOWN_TYPES.contains(&type_name.as_str()) {
            continue;
        }

        let name_dirs = match fs::read_dir(type_entry.path()) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for name_entry in name_dirs.filter_map(|e| e.ok()) {
            if !name_entry
                .file_type()
                .map(|ft| ft.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let session_name = match name_entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };

            let mtime = name_entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

            let files: Vec<String> = fs::read_dir(name_entry.path())
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .filter_map(|e| e.file_name().into_string().ok())
                        .collect()
                })
                .unwrap_or_default();

            sessions.push(SessionEntry {
                id: format!("{type_name}/{session_name}"),
                session_type: type_name.clone(),
                name: session_name,
                dir: name_entry.path().to_string_lossy().to_string(),
                mtime,
                files,
            });
        }
    }

    sessions.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    sessions
}

/// `session latest [sessions_dir]`
pub fn cmd_latest(sessions_dir: Option<&str>, cwd: &str) {
    let dir = sessions_dir.map(|s| s.to_string()).unwrap_or_else(|| {
        Path::new(cwd)
            .join(".hoangsa")
            .join("sessions")
            .to_string_lossy()
            .to_string()
    });

    let sessions = collect_sessions(&dir);
    if sessions.is_empty() {
        out(&json!({ "found": false }));
        return;
    }

    let s = &sessions[0];
    out(&json!({
        "found": true,
        "id": s.id,
        "type": s.session_type,
        "name": s.name,
        "dir": s.dir,
        "files": s.files,
    }));
}

/// `session list [sessions_dir]`
pub fn cmd_list(sessions_dir: Option<&str>, cwd: &str) {
    let dir = sessions_dir.map(|s| s.to_string()).unwrap_or_else(|| {
        Path::new(cwd)
            .join(".hoangsa")
            .join("sessions")
            .to_string_lossy()
            .to_string()
    });

    let sessions = collect_sessions(&dir);
    let session_values: Vec<serde_json::Value> = sessions
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "type": s.session_type,
                "name": s.name,
                "dir": s.dir,
                "files": s.files,
            })
        })
        .collect();

    out(&json!({ "sessions": session_values }));
}
