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

// ---------------------------------------------------------- projects commands

pub fn cmd_projects_which(root: &std::path::Path) -> anyhow::Result<()> {
    println!("{}", root.display());
    if is_global_root(root) {
        let cwd = std::env::current_dir()?;
        let slug = project_slug(&cwd);
        println!("  slug: {slug}");
        println!("  project: {}", cwd.display());
        println!("  mode: global");
    } else {
        println!("  mode: local");
    }
    Ok(())
}

pub fn cmd_projects_list() -> anyhow::Result<()> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("$HOME not set"))?;
    let registry_path = PathBuf::from(home).join(".thoth").join("projects.json");
    if !registry_path.is_file() {
        println!("No projects registered yet. Run `thoth setup` in a project directory.");
        return Ok(());
    }
    let content = std::fs::read_to_string(&registry_path)?;
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&content)?;
    if map.is_empty() {
        println!("No projects registered.");
        return Ok(());
    }
    for (slug, path) in &map {
        let path_str = path.as_str().unwrap_or("?");
        let exists = std::path::Path::new(path_str).is_dir();
        let marker = if exists { " " } else { "!" };
        println!("{marker} {slug}  {path_str}");
    }
    Ok(())
}

pub async fn cmd_projects_migrate(dry_run: bool, rm_local: bool) -> anyhow::Result<()> {
    let local = PathBuf::from(".thoth");
    if !local.is_dir() {
        anyhow::bail!("No local .thoth/ directory found in the current project.");
    }

    let dest = global_root_for_cwd()?;
    let cwd = std::env::current_dir()?;
    let slug = project_slug(&cwd);

    println!("migrate: .thoth/ → {}", dest.display());
    println!("  slug: {slug}");

    if dest.is_dir() {
        anyhow::bail!(
            "Destination already exists: {}\n  \
             Remove it first or use `thoth setup --global` for a fresh install.",
            dest.display()
        );
    }

    if dry_run {
        println!("  (dry run — no changes made)");
        return Ok(());
    }

    // Copy the entire .thoth/ tree to the global location.
    std::fs::create_dir_all(dest.parent().unwrap())?;
    copy_dir_recursive(&local, &dest)?;
    println!("  copied {} → {}", local.display(), dest.display());

    // Register in projects.json.
    register_project(&slug, &cwd)?;
    println!("  registered in ~/.thoth/projects.json");

    // Re-run hook + MCP install so THOTH_ROOT points to the new location.
    crate::hooks::install_all(crate::hooks::Scope::Project, &dest).await?;
    println!("  updated hooks + MCP to point to {}", dest.display());

    if rm_local {
        std::fs::remove_dir_all(&local)?;
        println!("  removed local .thoth/");
    } else {
        println!("  local .thoth/ kept (remove with `rm -rf .thoth/` or rerun with --rm-local)");
    }

    println!("done. Run `thoth projects which` to verify.");
    Ok(())
}

pub async fn cmd_projects_migrate_slugs(dry_run: bool) -> anyhow::Result<()> {
    let home = home_dir().ok_or_else(|| anyhow::anyhow!("$HOME not set"))?;
    let registry_path = home.join(".thoth").join("projects.json");
    if !registry_path.is_file() {
        println!("No projects registered. Nothing to migrate.");
        return Ok(());
    }
    let content = std::fs::read_to_string(&registry_path)?;
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&content).unwrap_or_default();

    let projects_dir = home.join(".thoth").join("projects");
    let mut new_map = serde_json::Map::new();
    let mut migrated = 0u32;

    for (old_slug, path_val) in &map {
        let Some(path_str) = path_val.as_str() else {
            new_map.insert(old_slug.clone(), path_val.clone());
            continue;
        };
        let project_path = Path::new(path_str);
        let new_slug = project_slug(project_path);

        if *old_slug == new_slug {
            new_map.insert(old_slug.clone(), path_val.clone());
            continue;
        }

        let old_dir = projects_dir.join(old_slug);
        let new_dir = projects_dir.join(&new_slug);

        println!("  {old_slug} → {new_slug}  ({path_str})");

        if !old_dir.is_dir() {
            println!("    ⚠ old directory missing, registering new slug only");
            new_map.insert(new_slug, path_val.clone());
            migrated += 1;
            continue;
        }
        if new_dir.is_dir() {
            println!("    ⚠ target already exists, skipping rename");
            new_map.insert(new_slug, path_val.clone());
            migrated += 1;
            continue;
        }

        if dry_run {
            new_map.insert(new_slug, path_val.clone());
            migrated += 1;
            continue;
        }

        std::fs::rename(&old_dir, &new_dir)?;
        new_map.insert(new_slug.clone(), path_val.clone());
        migrated += 1;

        if project_path.is_dir() {
            let saved_dir = std::env::current_dir()?;
            std::env::set_current_dir(project_path)?;
            let _ = crate::hooks::install_all(crate::hooks::Scope::Project, &new_dir).await;
            std::env::set_current_dir(&saved_dir)?;
            println!("    ✓ renamed + updated hooks/CLAUDE.md");
        } else {
            println!("    ✓ renamed (project dir gone, skipped hook update)");
        }
    }

    if !dry_run && migrated > 0 {
        let json = serde_json::to_string_pretty(&serde_json::Value::Object(new_map))?;
        std::fs::write(&registry_path, json)?;
    }

    if migrated == 0 {
        println!("All projects already use readable slugs.");
    } else if dry_run {
        println!("\n  {migrated} project(s) would be migrated. Run without --dry-run to apply.");
    } else {
        println!("\n✓ Migrated {migrated} project(s) to readable slugs.");
    }
    Ok(())
}

pub fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if ty.is_file() {
            std::fs::copy(&src_path, &dst_path)?;
        }
        // Skip symlinks, sockets (mcp.sock), etc.
    }
    Ok(())
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
