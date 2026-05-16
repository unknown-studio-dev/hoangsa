//! Server-wide state shared with every handler via Axum extractors.
//!
//! `project` is the active project context — reads and writes hit
//! `state.project_dir/.hoangsa/...`. It lives behind a `RwLock<Arc<...>>`
//! so the project switcher endpoint can hot-swap the target without
//! restarting the server (and without invalidating the CSRF token, which
//! lives in the URL of every open browser tab).
//!
//! `global_dir` is `~/.hoangsa` (or `project_dir/.hoangsa-global` in tests
//! when `HOME` is unset). It's immutable for the server's lifetime — the
//! registry, fastembed cache, etc. are all rooted there regardless of which
//! project is active.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use hoangsa_memory_core::projects::project_slug;

/// Active project the server is currently operating on.
#[derive(Debug, Clone)]
pub struct ProjectContext {
    /// Absolute path on disk (canonicalised when possible).
    pub project_dir: PathBuf,
    /// `project_slug(project_dir)` — last two path components, sanitised.
    pub slug: String,
    /// Human-readable display label. Defaults to the last path component or
    /// the registry's stored name when this is constructed via
    /// [`ProjectContext::from_registry_or_path`].
    pub name: String,
}

impl ProjectContext {
    /// Build a context directly from an absolute project path. Used at
    /// boot time before the registry is consulted.
    pub fn from_path(project_dir: PathBuf) -> Self {
        let abs = project_dir
            .canonicalize()
            .unwrap_or_else(|_| project_dir.clone());
        let slug = project_slug(&abs);
        let name = abs
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&slug)
            .to_string();
        Self {
            project_dir: abs,
            slug,
            name,
        }
    }
}

pub struct AppState {
    pub token: String,
    pub global_dir: PathBuf,
    project: RwLock<Arc<ProjectContext>>,
}

impl AppState {
    pub fn new(token: String, global_dir: PathBuf, project: ProjectContext) -> Self {
        Self {
            token,
            global_dir,
            project: RwLock::new(Arc::new(project)),
        }
    }

    /// Snapshot of the active project context. Cheap (Arc clone). Hold the
    /// returned `Arc` for the duration of the request rather than calling
    /// this twice — the project may be swapped between the two reads.
    pub fn current(&self) -> Arc<ProjectContext> {
        self.project
            .read()
            .expect("AppState.project rwlock poisoned")
            .clone()
    }

    /// Replace the active project. Returns the previous context so the
    /// caller can log "switched from X to Y" or roll back.
    pub fn switch(&self, project: ProjectContext) -> Arc<ProjectContext> {
        let mut guard = self
            .project
            .write()
            .expect("AppState.project rwlock poisoned");
        let prev = guard.clone();
        *guard = Arc::new(project);
        prev
    }
}
