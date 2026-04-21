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

/// Resolve the `.thoth/` data root via a 4-step chain:
///
/// 1. Explicit `--root` flag (highest priority)
/// 2. `$THOTH_ROOT` env var
/// 3. Project-local `./.thoth/` (backwards compat) — BUT only when it
///    actually has a populated graph. An empty `.thoth/` created by a
///    `thoth index .` run that lost the `--root` flag used to silently
///    pre-empt the real global root; we now detect that case and fall
///    through to the global path, printing a one-line warning so the
///    user knows why.
/// 4. Global `~/.thoth/projects/{slug}/`
pub fn resolve_root(explicit: Option<&Path>) -> PathBuf {
    if let Some(root) = explicit {
        return root.to_path_buf();
    }
    if let Ok(env) = std::env::var("THOTH_ROOT") {
        let p = PathBuf::from(env);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let local = PathBuf::from(".thoth");
    let local_populated = local.is_dir() && is_populated_root(&local);

    if local_populated {
        return local;
    }

    if let Some(home) = home_dir()
        && let Ok(cwd) = std::env::current_dir()
    {
        let projects = home.join(".thoth").join("projects");
        let slug = project_slug(&cwd);
        let new_path = projects.join(&slug);
        let global_path = if new_path.is_dir() {
            new_path
        } else {
            let legacy = legacy_project_slug(&cwd);
            let legacy_path = projects.join(&legacy);
            if legacy_path.is_dir() {
                legacy_path
            } else {
                new_path
            }
        };

        // Warn when we're falling through a stale local `.thoth/` to
        // reach a populated global root. Silent if the local doesn't
        // exist at all (common, expected) or the global path is equally
        // empty (we can't tell which is "right", so don't guess).
        if local.is_dir() && is_populated_root(&global_path) {
            eprintln!(
                "thoth: ignoring stale local .thoth/ (no graph.redb); using {} instead. \
                 Remove ./.thoth or run `thoth index --root ./.thoth .` to repopulate it.",
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

/// Legacy 12-char hex slug (blake3 hash). Used for backwards-compatible
/// resolution of projects created before the readable-slug migration.
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

/// Register a project in `~/.thoth/projects.json` so `thoth projects list`
/// can map slugs back to paths.
pub fn register_project(slug: &str, project_path: &Path) -> anyhow::Result<()> {
    let home = home_dir().ok_or_else(|| anyhow::anyhow!("$HOME not set"))?;
    let global_root = home.join(".thoth");
    std::fs::create_dir_all(&global_root)?;

    let registry_path = global_root.join("projects.json");
    let mut map: serde_json::Map<String, serde_json::Value> = if registry_path.is_file() {
        let content = std::fs::read_to_string(&registry_path)?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        serde_json::Map::new()
    };

    let canonical = project_path
        .canonicalize()
        .unwrap_or_else(|_| project_path.to_path_buf());
    map.insert(
        slug.to_string(),
        serde_json::Value::String(canonical.to_string_lossy().into_owned()),
    );

    let json = serde_json::to_string_pretty(&serde_json::Value::Object(map))?;
    std::fs::write(&registry_path, json)?;
    Ok(())
}

/// Returns true when the resolved root lives under `~/.thoth/projects/`.
pub fn is_global_root(root: &Path) -> bool {
    if let Some(home) = home_dir() {
        let global_prefix = home.join(".thoth").join("projects");
        root.starts_with(&global_prefix)
    } else {
        false
    }
}

/// Compute the global root for the current working directory.
/// Used by `thoth setup --global`. Returns the readable-slug path,
/// or the legacy hash path if it already exists (not yet migrated).
pub fn global_root_for_cwd() -> anyhow::Result<PathBuf> {
    let home = home_dir().ok_or_else(|| anyhow::anyhow!("$HOME not set"))?;
    let cwd = std::env::current_dir()?;
    let projects = home.join(".thoth").join("projects");
    let slug = project_slug(&cwd);
    let new_path = projects.join(&slug);
    if new_path.is_dir() {
        return Ok(new_path);
    }
    let legacy = legacy_project_slug(&cwd);
    let legacy_path = projects.join(&legacy);
    if legacy_path.is_dir() {
        return Ok(legacy_path);
    }
    Ok(new_path)
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
}
