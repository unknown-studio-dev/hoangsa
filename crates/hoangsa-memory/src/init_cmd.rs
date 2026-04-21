//! `hoangsa-memory init` — create `.hoangsa-memory/` + seed markdown + scaffold config.toml.

use std::path::Path;

use hoangsa_memory_store::StoreRoot;

/// Create `.hoangsa-memory/` at `root`, seed `MEMORY.md` / `LESSONS.md`, and
/// write a documented `config.toml` on first run. Existing files are preserved.
pub async fn cmd_init(root: &Path) -> anyhow::Result<()> {
    let existed = root.exists();
    let store = StoreRoot::open(root).await?;

    let mut seeded = Vec::new();
    for name in ["MEMORY.md", "LESSONS.md"] {
        let p = store.path.join(name);
        if !p.exists() {
            tokio::fs::write(&p, format!("# {name}\n")).await?;
            seeded.push(name);
        }
    }

    let cfg_path = store.path.join("config.toml");
    if !cfg_path.exists() {
        tokio::fs::write(&cfg_path, DEFAULT_CONFIG_TOML).await?;
        seeded.push("config.toml");
    }

    let verb = if existed { "refreshed" } else { "created" };
    println!("✓ {verb} {}", store.path.display());
    if !seeded.is_empty() {
        println!("  seeded: {}", seeded.join(", "));
    }
    println!("  next:   hoangsa-memory index .");
    Ok(())
}

const DEFAULT_CONFIG_TOML: &str = r#"# hoangsa-memory config. All fields are optional; defaults shown.
# Uncomment the ones you want to change.

[index]
# Gitignore-syntax patterns. Applied on top of `.gitignore`, `.ignore`, and
# any `.hoangsa-memoryignore` found in the project. Supports re-including with `!`.
#
# ignore = [
#     "target/",
#     "node_modules/",
#     "dist/",
#     "build/",
#     "*.generated.rs",
#     "docs/internal/",
#     "!docs/internal/README.md",
# ]

# Max file size (bytes) considered for indexing. Files larger than this
# are skipped with a debug log. Default: 2 MiB.
# max_file_size = 2097152

# Descend into hidden dirs (e.g. `.github`). Default: false.
# include_hidden = false

# Follow symlinks. Default: false — prevents indexing sibling projects.
# follow_symlinks = false

[memory]
# How many days an episode survives before TTL eviction. Default: 30.
# episodic_ttl_days = 30

# Hard cap on episode count before capacity-based eviction. Default: 50_000.
# max_episodes = 50000

# Lessons with a success ratio below this floor (and at least
# `lesson_min_attempts` attempts) are dropped by the forget pass.
# lesson_floor = 0.2
# lesson_min_attempts = 3

# Exponential decay rate per day for the retention score, and the floor
# below which an episode is dropped. Set `decay_floor = 0.0` to disable
# decay-based eviction entirely (Mode::Zero deterministic).
# decay_lambda = 0.02
# decay_floor  = 0.05

[output]
# Recall/impact text-rendering budgets. Structured JSON (`--json` /
# MCP `data`) is never truncated — only the human-readable text
# surface honours these caps.

# Maximum body lines rendered per recall chunk. Excess lines become
# a `[… truncated, M more lines. Read <path>:L<a>-L<b> for full
# body]` marker. Default: 200. Set to 0 to disable.
# max_body_lines = 200

# Soft cap on total rendered bytes per recall. A chunk in progress
# finishes, but no new chunk starts once the budget is crossed.
# Remaining chunks are elided with a footer. Default: 32768.
# Set to 0 to disable.
# max_total_bytes = 32768

# Node count above which `memory_impact` groups results by file
# rather than listing every node. Default: 50. Set to 0 to disable
# grouping (always flat list).
# impact_group_threshold = 50
"#;
