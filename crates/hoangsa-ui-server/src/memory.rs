//! Memory daemon health probe + restart trigger for the UI's degraded-mode
//! handling. The actual MCP daemon lives in `hoangsa-memory-mcp` and is
//! normally spawned by Claude Code itself; we don't manage its lifecycle —
//! `restart` just SIGTERMs it and lets the MCP client respawn on next use.

use serde::Serialize;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct DaemonStatus {
    pub socket_path: String,
    pub socket_exists: bool,
    pub connectable: bool,
    pub project_slug: String,
}

/// Per-project slug — last two path components, lowercased, non-alnum → `-`.
/// Mirrors `hoangsa_memory::resolve::project_slug` exactly. We replicate it
/// instead of taking a heavyweight dep on hoangsa-memory just for this.
pub fn project_slug(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let components: Vec<&str> = canonical
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    let n = components.len();
    let parts = if n >= 2 { &components[n - 2..] } else { &components[..] };
    sanitize_slug(&parts.join("-"))
}

fn sanitize_slug(raw: &str) -> String {
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

pub fn socket_for(project_dir: &Path, global_dir: &Path) -> PathBuf {
    let slug = project_slug(project_dir);
    global_dir.join(format!("memory/projects/{slug}/mcp.sock"))
}

pub fn status(project_dir: &Path, global_dir: &Path) -> DaemonStatus {
    let slug = project_slug(project_dir);
    let sock = socket_for(project_dir, global_dir);
    let exists = sock.exists();
    let connectable = if exists {
        UnixStream::connect_timeout_compat(&sock, Duration::from_millis(200)).is_ok()
    } else {
        false
    };
    DaemonStatus {
        socket_path: sock.display().to_string(),
        socket_exists: exists,
        connectable,
        project_slug: slug,
    }
}

#[derive(Debug, Serialize)]
pub struct RestartOutcome {
    pub killed: bool,
    pub message: String,
}

/// SIGTERMs every running `hoangsa-memory-mcp` process. We don't respawn —
/// the MCP client (Claude Code) does that on next memory tool call. The UI
/// shows a hint asking the user to retry their action.
pub fn restart() -> RestartOutcome {
    let out = Command::new("pkill").arg("-f").arg("hoangsa-memory-mcp").output();
    match out {
        Ok(o) if o.status.success() => RestartOutcome {
            killed: true,
            message: "Daemon killed. Claude Code will respawn it on the next memory tool call."
                .to_string(),
        },
        Ok(_) => RestartOutcome {
            killed: false,
            message: "No matching process — daemon was not running.".to_string(),
        },
        Err(e) => RestartOutcome {
            killed: false,
            message: format!("pkill failed: {e}"),
        },
    }
}

trait UnixStreamExt {
    fn connect_timeout_compat(path: &Path, timeout: Duration) -> std::io::Result<UnixStream>;
}

impl UnixStreamExt for UnixStream {
    fn connect_timeout_compat(path: &Path, _timeout: Duration) -> std::io::Result<UnixStream> {
        // std::os::unix::net has no connect_timeout for UnixStream; the socket is
        // local so the kernel returns immediately on success or ECONNREFUSED.
        UnixStream::connect(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_uses_last_two_components() {
        let p = std::path::PathBuf::from("/tmp/foo/my-project");
        let s = project_slug(&p);
        assert!(s.contains("foo-my-project") || s == "foo-my-project", "got {s}");
    }

    #[test]
    fn status_when_socket_missing() {
        let dir = tempfile::tempdir().unwrap();
        let st = status(dir.path(), dir.path());
        assert!(!st.socket_exists);
        assert!(!st.connectable);
    }
}
