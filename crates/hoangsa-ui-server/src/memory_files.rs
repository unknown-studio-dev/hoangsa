//! Degraded-mode reader for the three top-level markdown memory files.
//!
//! When `hoangsa-memory-mcp` is down, the Memory tab's Files sub-tab
//! falls back here: read `USER.md` / `MEMORY.md` / `LESSONS.md` straight
//! off disk so the user can at least *see* what's in memory while they
//! restart the daemon. Mutations stay disabled — only the daemon can
//! safely append + invalidate the embedder index.

use std::path::{Path, PathBuf};

/// One markdown file's worth of degraded content. `body` is `None` when
/// the file doesn't exist yet (a fresh project before anything has been
/// remembered), distinguishing "empty by design" from "file missing".
#[derive(Debug, serde::Serialize)]
pub struct FileSnapshot {
    pub path: String,
    pub body: Option<String>,
    pub bytes: Option<usize>,
}

/// Snapshot of the three markdown files for a given project memory
/// root. Read errors (other than `NotFound`) are folded into `body =
/// None` — the FS-direct degraded view is best-effort by design; a
/// real I/O problem will already be visible through the daemon-health
/// banner.
#[derive(Debug, serde::Serialize)]
pub struct MemoryFiles {
    pub user: FileSnapshot,
    pub memory: FileSnapshot,
    pub lessons: FileSnapshot,
}

pub fn project_memory_root(global_dir: &Path, project_slug: &str) -> PathBuf {
    global_dir
        .join("memory")
        .join("projects")
        .join(project_slug)
}

pub fn read_files(global_dir: &Path, project_slug: &str) -> MemoryFiles {
    let root = project_memory_root(global_dir, project_slug);
    MemoryFiles {
        user: read_one(&root, "USER.md"),
        memory: read_one(&root, "MEMORY.md"),
        lessons: read_one(&root, "LESSONS.md"),
    }
}

fn read_one(root: &Path, name: &str) -> FileSnapshot {
    let path = root.join(name);
    match std::fs::read_to_string(&path) {
        Ok(body) => FileSnapshot {
            path: path.display().to_string(),
            bytes: Some(body.len()),
            body: Some(body),
        },
        Err(_) => FileSnapshot {
            path: path.display().to_string(),
            body: None,
            bytes: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn returns_none_body_when_files_missing() {
        let dir = tempdir().unwrap();
        let snap = read_files(dir.path(), "alpha");
        assert!(snap.user.body.is_none());
        assert!(snap.memory.body.is_none());
        assert!(snap.lessons.body.is_none());
        // Paths still resolved so the UI can show the location.
        assert!(snap.user.path.ends_with("alpha/USER.md"));
    }

    #[test]
    fn reads_present_files() {
        let dir = tempdir().unwrap();
        let root = project_memory_root(dir.path(), "alpha");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("MEMORY.md"), "hello").unwrap();
        let snap = read_files(dir.path(), "alpha");
        assert_eq!(snap.memory.body.as_deref(), Some("hello"));
        assert_eq!(snap.memory.bytes, Some(5));
        assert!(snap.user.body.is_none());
    }
}
