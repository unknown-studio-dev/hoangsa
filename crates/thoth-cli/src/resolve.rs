use std::path::{Path, PathBuf};

#[derive(clap::Subcommand, Debug)]
pub enum ProjectsCmd {
    /// List all registered projects with their slugs and paths.
    List,
    /// Show which root the current directory resolves to.
    Which,
    /// Move `./.thoth/` to `~/.thoth/projects/{slug}/` and update
    /// hooks + MCP to point to the new location.
    Migrate {
        /// Print what would happen without modifying anything.
        #[arg(long)]
        dry_run: bool,
        /// Delete the local `.thoth/` after a successful copy.
        #[arg(long)]
        rm_local: bool,
    },
    /// Rename all hash-based project directories to human-readable slugs
    /// and update projects.json + hooks + CLAUDE.md.
    MigrateSlugs {
        /// Print what would happen without modifying anything.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Resolve the `.hoangsa-memory/` data root via a 4-step chain:
///
/// 1. Explicit `--root` flag (highest priority)
/// 2. `$HOANGSA_MEMORY_ROOT` env var
/// 3. Project-local `./.hoangsa-memory/` — BUT only when it actually has
///    a populated graph. An empty `.hoangsa-memory/` created by an
///    `index .` run that lost the `--root` flag used to silently pre-empt
///    the real global root; we now detect that case and fall through to
///    the global path, printing a one-line warning so the user knows why.
/// 4. Global `~/.hoangsa-memory/projects/{slug}/`
pub fn resolve_root(explicit: Option<&Path>) -> PathBuf {
    if let Some(root) = explicit {
        return root.to_path_buf();
    }
    if let Ok(env) = std::env::var("HOANGSA_MEMORY_ROOT") {
        let p = PathBuf::from(env);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let local = PathBuf::from(".hoangsa-memory");
    let local_populated = local.is_dir() && is_populated_root(&local);

    if local_populated {
        return local;
    }

    if let Some(home) = home_dir()
        && let Ok(cwd) = std::env::current_dir()
    {
        let projects = home.join(".hoangsa-memory").join("projects");
        let slug = project_slug(&cwd);
        let global_path = projects.join(&slug);
        // Readable-slug is the only global layout we accept. Legacy
        // blake3 hash dirs from the pre-rename era are intentionally NOT
        // consulted — they'd shadow a fresh install and let the indexer
        // write to an orphaned location. Users with hash-era data move
        // it manually; new installs always route to `{slug}/`.

        // Warn when we're falling through a stale local `.hoangsa-memory/`
        // to reach a populated global root. Silent if the local doesn't
        // exist at all (common, expected) or the global path is equally
        // empty (we can't tell which is "right", so don't guess).
        if local.is_dir() && is_populated_root(&global_path) {
            eprintln!(
                "hoangsa-memory: ignoring stale local .hoangsa-memory/ (no graph.redb); using {} instead. \
                 Remove ./.hoangsa-memory or run `hoangsa-memory index --root ./.hoangsa-memory .` to repopulate it.",
                global_path.display()
            );
        }
        return global_path;
    }

    local
}

/// True when the root directory looks like it has usable indexed data —
/// i.e. a `graph.redb` that is larger than a fresh empty redb file.
/// Empty redb databases are ~4 KiB (header + one free-page map entry);
/// anything under 1 KiB is definitely empty, and the 4-KiB threshold is
/// a generous cutoff for "has any rows".
fn is_populated_root(root: &Path) -> bool {
    let graph = root.join("graph.redb");
    match std::fs::metadata(&graph) {
        Ok(m) => m.is_file() && m.len() > 4096,
        Err(_) => false,
    }
}

/// Human-readable slug from a project path: last two path components,
/// lowercased, non-alphanumeric replaced with `-`, collapsed.
///
/// Example: `/Users/nat/Desktop/thoth` → `desktop-thoth`
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

/// Legacy 12-char hex slug (blake3 hash). No longer consulted by
/// [`resolve_root`] — kept only so the `projects migrate-slugs` command
/// can still locate pre-rename data directories on disk.
#[allow(dead_code)]
pub fn legacy_project_slug(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let hash = blake3::hash(canonical.to_string_lossy().as_bytes());
    hash.to_hex()[..12].to_string()
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

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_basic() {
        assert_eq!(sanitize_slug("Desktop-thoth"), "desktop-thoth");
        assert_eq!(sanitize_slug("My Project"), "my-project");
        assert_eq!(sanitize_slug("foo///bar"), "foo-bar");
        assert_eq!(sanitize_slug("--leading--"), "leading");
    }

    #[test]
    fn slug_uses_last_two_components() {
        let p = PathBuf::from("/a/b/c/Desktop/thoth");
        let components: Vec<&str> = p
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect();
        let n = components.len();
        let parts = if n >= 2 { &components[n - 2..] } else { &components[..] };
        assert_eq!(sanitize_slug(&parts.join("-")), "desktop-thoth");
    }

    #[test]
    fn legacy_slug_is_hex() {
        let p = PathBuf::from("/tmp/test-project");
        let slug = legacy_project_slug(&p);
        assert_eq!(slug.len(), 12);
        assert!(slug.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// Bug 5 — a `./.thoth/` with no `graph.redb` (or a stub one) must
    /// not shadow the populated global root. This is the explicit
    /// detection gate `resolve_root` uses to decide whether to accept
    /// the local or fall through to `~/.thoth/projects/{slug}/`.
    #[test]
    fn is_populated_root_rejects_empty_and_stub_graph() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        // No graph.redb at all — definitely not populated.
        assert!(!is_populated_root(root));

        // Stub graph.redb under the 4 KiB threshold — still not populated.
        std::fs::write(root.join("graph.redb"), vec![0u8; 1024]).expect("write stub");
        assert!(!is_populated_root(root));

        // Cross the threshold — considered populated.
        std::fs::write(root.join("graph.redb"), vec![0u8; 8 * 1024]).expect("write big");
        assert!(is_populated_root(root));
    }
}
