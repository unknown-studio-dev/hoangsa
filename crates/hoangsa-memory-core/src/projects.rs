//! Project registry — tracks every project this user has indexed with
//! hoangsa-memory.
//!
//! Lives at `~/.hoangsa/projects.json`. The slug under
//! `~/.hoangsa/memory/projects/{slug}/` is one-way (last two path components,
//! sanitized) so without a registry there's no way to recover the original
//! absolute path. The UI uses this registry to render a project switcher;
//! the daemon will use it (Phase 3) to know which sockets to open.
//!
//! Concurrency: the on-disk JSON is the only shared state. Writes go through
//! [`Registry::save`] which writes-and-renames via a sibling temp file, so
//! concurrent readers see a consistent snapshot. There is no in-process lock
//! — each caller reloads from disk before mutating.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Current registry schema version. Bump when the on-disk shape changes.
pub const REGISTRY_VERSION: u32 = 1;

/// One known project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    /// Folder name under `~/.hoangsa/memory/projects/`.
    pub slug: String,
    /// Absolute path to the project root on disk.
    pub path: PathBuf,
    /// Human-readable display name. Defaults to the last path component.
    pub name: String,
    /// Unix epoch seconds.
    pub registered_at: u64,
    /// Unix epoch seconds. Updated on every CLI invocation / UI boot.
    pub last_used_at: u64,
}

/// Wire format for `~/.hoangsa/projects.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registry {
    /// Schema version. See [`REGISTRY_VERSION`].
    pub version: u32,
    /// All known projects, ordered by `last_used_at` desc when read via
    /// [`Registry::sorted`].
    pub projects: Vec<Project>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            projects: Vec::new(),
        }
    }
}

/// Path to the registry file inside a hoangsa home dir
/// (`~/.hoangsa/projects.json`).
pub fn registry_path(hoangsa_home: &Path) -> PathBuf {
    hoangsa_home.join("projects.json")
}

/// `$HOME` (or `$USERPROFILE` on Windows). `None` only when neither env
/// var is set.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `~/.hoangsa` resolved from `$HOME` (or `$USERPROFILE` on Windows). Errors
/// out only when neither env var is set.
pub fn default_hoangsa_home() -> Result<PathBuf, RegistryError> {
    home_dir()
        .map(|h| h.join(".hoangsa"))
        .ok_or(RegistryError::NoHome)
}

/// True when `<root>/graph.redb` exists and is larger than a fresh empty
/// redb file (~4 KiB header).
pub fn is_populated_root(root: &Path) -> bool {
    let graph = root.join("graph.redb");
    match std::fs::metadata(&graph) {
        Ok(m) => m.is_file() && m.len() > 4096,
        Err(_) => false,
    }
}

/// Resolve the `.hoangsa/memory/` data root via a 4-step chain:
///
/// 1. `explicit_root` argument (e.g. `--root` flag)
/// 2. `$HOANGSA_MEMORY_ROOT` env var
/// 3. `<project_dir>/.hoangsa/memory/` — only when populated (a stale
///    empty local from a misrouted `index` run used to silently shadow the
///    real global root; we now detect that case and fall through, printing
///    a one-line warning)
/// 4. `<home>/.hoangsa/memory/projects/<project_slug(project_dir)>/`
///
/// Falls back to the local path if no home dir is resolvable.
pub fn resolve_root(project_dir: &Path, explicit_root: Option<&Path>) -> PathBuf {
    if let Some(root) = explicit_root {
        return root.to_path_buf();
    }
    if let Ok(env) = std::env::var("HOANGSA_MEMORY_ROOT") {
        let p = PathBuf::from(env);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let local = project_dir.join(".hoangsa").join("memory");
    if local.is_dir() && is_populated_root(&local) {
        return local;
    }

    if let Some(home) = home_dir() {
        let projects = home.join(".hoangsa").join("memory").join("projects");
        let slug = project_slug(project_dir);
        let global = projects.join(&slug);
        if local.is_dir() && is_populated_root(&global) {
            eprintln!(
                "hoangsa-memory: ignoring stale local .hoangsa/memory/ (no graph.redb); using {} instead. \
                 Remove ./.hoangsa/memory or run `hoangsa-memory index --root ./.hoangsa/memory .` to repopulate it.",
                global.display()
            );
        }
        return global;
    }

    local
}

/// Per-project slug — last two canonical path components, lowercased,
/// non-alphanumeric collapsed to `-`.
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

fn epoch_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Errors raised by registry operations.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// `$HOME` / `$USERPROFILE` unset — can't resolve `~/.hoangsa`.
    #[error("cannot determine home directory")]
    NoHome,
    /// I/O error reading or writing the registry file.
    #[error("registry I/O at {path}: {source}")]
    Io {
        /// File path the operation was on.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// `projects.json` exists but the bytes don't parse.
    #[error("registry parse at {path}: {source}")]
    Parse {
        /// File path that failed to parse.
        path: PathBuf,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },
}

impl Registry {
    /// Load the registry from `<hoangsa_home>/projects.json`. Returns the
    /// default empty registry when the file doesn't exist (first-run case).
    pub fn load(hoangsa_home: &Path) -> Result<Self, RegistryError> {
        let path = registry_path(hoangsa_home);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(RegistryError::Io { path, source }),
        };
        let registry: Self = serde_json::from_slice(&bytes)
            .map_err(|source| RegistryError::Parse { path, source })?;
        Ok(registry)
    }

    /// Atomically write the registry to disk via [`crate::io::atomic_write`]
    /// — a crash mid-write can't corrupt the existing file.
    pub fn save(&self, hoangsa_home: &Path) -> Result<(), RegistryError> {
        let path = registry_path(hoangsa_home);
        let json = serde_json::to_vec_pretty(self).map_err(|source| RegistryError::Parse {
            path: path.clone(),
            source,
        })?;
        crate::io::atomic_write(&path, &json).map_err(|source| RegistryError::Io {
            path: path.clone(),
            source,
        })
    }

    /// Idempotently upsert a project by absolute path. Returns the inserted
    /// or updated [`Project`] entry.
    ///
    /// - Slug is computed from `path` (canonicalised when possible).
    /// - When the slug already exists, `last_used_at` is bumped and `path`
    ///   is updated (a moved repo keeps its slug if the last two components
    ///   match; otherwise a different slug is computed and a new entry
    ///   appears).
    /// - `name` is derived from the last path component on first insert and
    ///   left untouched on subsequent calls (user may have edited it).
    pub fn register(&mut self, path: impl AsRef<Path>) -> &Project {
        let path = path.as_ref();
        let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let slug = project_slug(&abs);
        let now = epoch_now();
        let display_name = abs
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&slug)
            .to_string();
        if let Some(idx) = self.projects.iter().position(|p| p.slug == slug) {
            self.projects[idx].path = abs;
            self.projects[idx].last_used_at = now;
            return &self.projects[idx];
        }
        self.projects.push(Project {
            slug,
            path: abs,
            name: display_name,
            registered_at: now,
            last_used_at: now,
        });
        self.projects.last().expect("just pushed")
    }

    /// Bump `last_used_at` for an existing slug. No-op when the slug is
    /// unknown — the caller usually wants [`Registry::register`] in that
    /// case.
    pub fn touch(&mut self, slug: &str) -> bool {
        let now = epoch_now();
        if let Some(p) = self.projects.iter_mut().find(|p| p.slug == slug) {
            p.last_used_at = now;
            true
        } else {
            false
        }
    }

    /// Remove a project by slug. Returns `true` when something was removed.
    /// Note: this does NOT delete the on-disk memory data at
    /// `~/.hoangsa/memory/projects/{slug}/` — that's a destructive op the
    /// caller must do explicitly.
    pub fn remove(&mut self, slug: &str) -> bool {
        let len_before = self.projects.len();
        self.projects.retain(|p| p.slug != slug);
        self.projects.len() != len_before
    }

    /// Find a project by slug.
    pub fn find(&self, slug: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.slug == slug)
    }

    /// Set the human-readable display name for a slug. Returns `true` when
    /// the slug exists.
    pub fn rename(&mut self, slug: &str, new_name: &str) -> bool {
        if let Some(p) = self.projects.iter_mut().find(|p| p.slug == slug) {
            p.name = new_name.to_string();
            true
        } else {
            false
        }
    }

    /// Projects sorted by `last_used_at` descending — most recently used
    /// first. Doesn't mutate the underlying ordering.
    pub fn sorted(&self) -> Vec<&Project> {
        let mut v: Vec<&Project> = self.projects.iter().collect();
        v.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));
        v
    }
}

/// Slugs present under `~/.hoangsa/memory/projects/` that aren't tracked in
/// `projects.json`. Useful for the UI to surface "I see you have data for
/// these slugs but I don't know their abs paths" — user can then resolve by
/// pointing at the correct folder.
///
/// Returns an empty vec when the directory doesn't exist or can't be read.
pub fn discover_orphan_slugs(hoangsa_home: &Path, registry: &Registry) -> Vec<String> {
    let projects_dir = hoangsa_home.join("memory").join("projects");
    let entries = match std::fs::read_dir(&projects_dir) {
        Ok(it) => it,
        Err(_) => return Vec::new(),
    };
    let mut orphans = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else { continue };
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(String::from) else { continue };
        if registry.find(&name).is_none() {
            orphans.push(name);
        }
    }
    orphans.sort();
    orphans
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn slug_basic() {
        assert_eq!(sanitize_slug("Desktop-my-project"), "desktop-my-project");
        assert_eq!(sanitize_slug("My Project"), "my-project");
        assert_eq!(sanitize_slug("foo///bar"), "foo-bar");
        assert_eq!(sanitize_slug("--leading--"), "leading");
    }

    #[test]
    fn load_returns_empty_when_missing() {
        let dir = tempdir().unwrap();
        let reg = Registry::load(dir.path()).unwrap();
        assert!(reg.projects.is_empty());
        assert_eq!(reg.version, REGISTRY_VERSION);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempdir().unwrap();
        let mut reg = Registry::default();
        let project_dir = dir.path().join("Desktop").join("my-project");
        std::fs::create_dir_all(&project_dir).unwrap();
        reg.register(&project_dir);
        reg.save(dir.path()).unwrap();
        let reloaded = Registry::load(dir.path()).unwrap();
        assert_eq!(reloaded.projects.len(), 1);
        assert_eq!(reloaded.projects[0].name, "my-project");
        assert!(reloaded.projects[0].slug.ends_with("my-project"));
    }

    #[test]
    fn register_is_idempotent_and_bumps_last_used() {
        let dir = tempdir().unwrap();
        let project_dir = dir.path().join("repo");
        std::fs::create_dir_all(&project_dir).unwrap();

        let mut reg = Registry::default();
        let slug_first;
        let registered_at_first;
        {
            let p = reg.register(&project_dir);
            slug_first = p.slug.clone();
            registered_at_first = p.registered_at;
        }
        // Sleep one second so last_used_at can advance (epoch is in seconds).
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let p2 = reg.register(&project_dir).clone();
        assert_eq!(reg.projects.len(), 1, "duplicate register should not add a row");
        assert_eq!(p2.slug, slug_first);
        assert_eq!(p2.registered_at, registered_at_first);
        assert!(p2.last_used_at >= registered_at_first);
    }

    #[test]
    fn remove_returns_false_for_unknown() {
        let mut reg = Registry::default();
        assert!(!reg.remove("does-not-exist"));
    }

    #[test]
    fn discover_orphans_lists_slugs_not_in_registry() {
        let home = tempdir().unwrap();
        let projects = home.path().join("memory").join("projects");
        std::fs::create_dir_all(projects.join("orphan-one")).unwrap();
        std::fs::create_dir_all(projects.join("orphan-two")).unwrap();
        std::fs::create_dir_all(projects.join("registered-one")).unwrap();

        let mut reg = Registry::default();
        // Manually craft an entry so we don't need a real path canonicalisation.
        reg.projects.push(Project {
            slug: "registered-one".into(),
            path: PathBuf::from("/does/not/matter"),
            name: "registered-one".into(),
            registered_at: 0,
            last_used_at: 0,
        });

        let orphans = discover_orphan_slugs(home.path(), &reg);
        assert_eq!(orphans, vec!["orphan-one", "orphan-two"]);
    }

    #[test]
    fn sorted_orders_by_last_used_desc() {
        let mut reg = Registry::default();
        reg.projects.push(Project {
            slug: "old".into(),
            path: PathBuf::from("/a"),
            name: "old".into(),
            registered_at: 100,
            last_used_at: 100,
        });
        reg.projects.push(Project {
            slug: "new".into(),
            path: PathBuf::from("/b"),
            name: "new".into(),
            registered_at: 200,
            last_used_at: 200,
        });
        let sorted = reg.sorted();
        assert_eq!(sorted[0].slug, "new");
        assert_eq!(sorted[1].slug, "old");
    }
}
