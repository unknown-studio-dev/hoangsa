//! `hoangsa-memory projects` — registry CLI.
//!
//! The registry at `~/.hoangsa/projects.json` is the source of truth for
//! "which projects on this machine have hoangsa-memory data". Entries are
//! created automatically by every CLI invocation that resolves to a global
//! slug path (see [`crate::auto_register`]); these subcommands let the user
//! list, rename, and prune them.

use std::path::PathBuf;

use anyhow::Context;
use clap::Subcommand;
use hoangsa_memory_core::projects::{
    Registry, default_hoangsa_home, discover_orphan_slugs, project_slug,
};

/// `projects` subcommands.
#[derive(Subcommand, Debug)]
pub enum ProjectsCmd {
    /// Print every registered project as a table (slug, last-used, path).
    List {
        /// Include orphan slugs — directories under
        /// `~/.hoangsa/memory/projects/` that aren't in the registry. The
        /// abs path of an orphan is unrecoverable; the listing exists so a
        /// user can see leftover state and decide to wipe or re-register.
        #[arg(long)]
        with_orphans: bool,
    },
    /// Register a project (idempotent — bumps `last_used_at` if already
    /// present). `path` defaults to the current directory.
    Add {
        /// Project root. Resolved to an absolute path.
        path: Option<PathBuf>,
        /// Optional human-readable display name. Defaults to the last path
        /// component on first insert; ignored if the slug already exists.
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove a project entry by slug. Does NOT delete the on-disk
    /// `~/.hoangsa/memory/projects/{slug}/` data; use `rm -rf` if that's
    /// the intent.
    Remove {
        /// Slug to remove. Find with `projects list`.
        slug: String,
    },
    /// Rename the display name of a registered project.
    Rename {
        /// Slug whose display name should change.
        slug: String,
        /// New display name (kept as-is, no slug-style sanitisation).
        name: String,
    },
    /// Print the slug + registered path that the current directory resolves
    /// to. Useful for shell scripts that want to query the registry.
    Which {
        /// Path to inspect. Defaults to the current directory.
        path: Option<PathBuf>,
    },
}

/// Entry point dispatched from `main.rs`.
pub async fn run(cmd: ProjectsCmd, json: bool) -> anyhow::Result<()> {
    let home = default_hoangsa_home().context("resolve $HOME for projects.json")?;
    match cmd {
        ProjectsCmd::List { with_orphans } => list(&home, with_orphans, json),
        ProjectsCmd::Add { path, name } => add(&home, path, name, json),
        ProjectsCmd::Remove { slug } => remove(&home, &slug, json),
        ProjectsCmd::Rename { slug, name } => rename(&home, &slug, &name, json),
        ProjectsCmd::Which { path } => which(&home, path, json),
    }
}

fn list(home: &std::path::Path, with_orphans: bool, json: bool) -> anyhow::Result<()> {
    let registry = Registry::load(home)?;
    let projects = registry.sorted();
    let orphans = if with_orphans {
        discover_orphan_slugs(home, &registry)
    } else {
        Vec::new()
    };

    if json {
        let body = serde_json::json!({
            "projects": projects,
            "orphan_slugs": orphans,
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }

    if projects.is_empty() {
        println!("(no registered projects)");
    } else {
        println!("{:<32} {:<12} {}", "SLUG", "LAST_USED", "PATH");
        for p in &projects {
            let last = format_relative(p.last_used_at);
            println!("{:<32} {:<12} {}", p.slug, last, p.path.display());
        }
    }
    if with_orphans && !orphans.is_empty() {
        println!();
        println!("Orphan slugs (data exists, abs path unknown):");
        for slug in orphans {
            println!("  {slug}");
        }
    }
    Ok(())
}

fn add(
    home: &std::path::Path,
    path: Option<PathBuf>,
    name: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let path = match path {
        Some(p) => p,
        None => std::env::current_dir().context("read cwd for `projects add`")?,
    };
    if !path.exists() {
        anyhow::bail!("project path does not exist: {}", path.display());
    }
    let mut registry = Registry::load(home)?;
    let slug = {
        let project = registry.register(&path);
        project.slug.clone()
    };
    if let Some(n) = name {
        registry.rename(&slug, &n);
    }
    registry.save(home)?;
    let project = registry
        .find(&slug)
        .expect("just inserted")
        .clone();
    if json {
        println!("{}", serde_json::to_string_pretty(&project)?);
    } else {
        println!(
            "registered {} ({}) → {}",
            project.name,
            project.slug,
            project.path.display()
        );
    }
    Ok(())
}

fn remove(home: &std::path::Path, slug: &str, json: bool) -> anyhow::Result<()> {
    let mut registry = Registry::load(home)?;
    let removed = registry.remove(slug);
    if removed {
        registry.save(home)?;
    }
    if json {
        println!("{}", serde_json::json!({ "slug": slug, "removed": removed }));
    } else if removed {
        println!("removed {slug}");
    } else {
        println!("no entry for slug {slug}");
    }
    Ok(())
}

fn rename(home: &std::path::Path, slug: &str, name: &str, json: bool) -> anyhow::Result<()> {
    let mut registry = Registry::load(home)?;
    let renamed = registry.rename(slug, name);
    if renamed {
        registry.save(home)?;
    }
    if json {
        println!(
            "{}",
            serde_json::json!({ "slug": slug, "renamed": renamed, "name": name })
        );
    } else if renamed {
        println!("renamed {slug} → {name}");
    } else {
        println!("no entry for slug {slug}");
    }
    Ok(())
}

fn which(home: &std::path::Path, path: Option<PathBuf>, json: bool) -> anyhow::Result<()> {
    let path = match path {
        Some(p) => p,
        None => std::env::current_dir().context("read cwd for `projects which`")?,
    };
    let slug = project_slug(&path);
    let registry = Registry::load(home)?;
    let entry = registry.find(&slug).cloned();
    let registered = entry.is_some();
    let abs = path.canonicalize().unwrap_or_else(|_| path.clone());
    if json {
        let body = serde_json::json!({
            "slug": slug,
            "registered": registered,
            "path": abs,
            "entry": entry,
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        println!("slug:       {slug}");
        println!("path:       {}", abs.display());
        println!(
            "registered: {}",
            if registered { "yes" } else { "no" }
        );
    }
    Ok(())
}

fn format_relative(epoch_secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if epoch_secs == 0 || epoch_secs > now {
        return "—".to_string();
    }
    let delta = now - epoch_secs;
    if delta < 60 {
        format!("{delta}s ago")
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86_400)
    }
}
