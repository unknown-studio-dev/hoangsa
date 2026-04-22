//! `hoangsa-memory init` — create `.hoangsa/memory/` + seed markdown + scaffold config.toml.

use std::path::Path;

/// Create `.hoangsa/memory/` at `root`, seed `MEMORY.md` / `LESSONS.md`, and
/// write a documented `config.toml` on first run. Existing files are preserved.
///
/// `init` only touches markdown + config files. It intentionally does NOT
/// open the redb graph, tantivy index, or sqlite episode log — those would
/// fail with "Database already open. Cannot acquire lock" when an MCP
/// daemon is already running against the same root. Opening them has no
/// value here because `index` / `watch` / `query` all open them on demand.
pub async fn cmd_init(root: &Path) -> anyhow::Result<()> {
    let existed = root.exists();
    tokio::fs::create_dir_all(root).await?;

    let mut seeded = Vec::new();
    for name in ["MEMORY.md", "LESSONS.md"] {
        let p = root.join(name);
        if !p.exists() {
            tokio::fs::write(&p, format!("# {name}\n")).await?;
            seeded.push(name);
        }
    }

    let cfg_path = root.join("config.toml");
    if !cfg_path.exists() {
        tokio::fs::write(&cfg_path, DEFAULT_CONFIG_TOML).await?;
        seeded.push("config.toml");
    }

    let verb = if existed { "refreshed" } else { "created" };
    println!("✓ {verb} {}", root.display());
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
# any `.memoryignore` found in the project. Supports re-including with `!`.
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

# Hard caps (bytes) for the three markdown surfaces. A `memory_remember_*`
# that would push the file above its cap returns a structured
# `CapExceededError` instead of silently appending — the agent must call
# `memory_replace` / `memory_remove` first. Sized so USER + MEMORY +
# LESSONS combined inject < ~10K tokens at SessionStart.
# cap_memory_bytes  = 16384
# cap_user_bytes    = 4096
# cap_lessons_bytes = 16384

# FLEXIBLE content policy (DESIGN-SPEC REQ-12). When `false`, MCP
# `remember_*` handlers only log a warning if a payload looks like a bare
# commit sha / ISO date / file path with no invariant. Set `true` to
# reject such payloads with a structured error.
# strict_content_policy = false

[retrieve]
# Post-fusion multiplier applied to every Markdown-sourced chunk
# (MEMORY.md / LESSONS.md). Values > 1.0 lift facts/lessons over code
# for prose queries; < 1.0 pushes markdown down; 0.0 hides it.
# Clamped to [0.0, 10.0] at load time so a typo (18.0 vs 1.8) cannot
# shadow the entire code corpus. Default: 1.0 (no-op).
# rerank_markdown_boost = 1.0

[watch]
# Auto-watch the project source tree from inside the MCP server, so
# source edits are reindexed without a separate `hoangsa-memory watch`
# process. Default: false.
# enabled = false

# Debounce window (ms). Events arriving within this window after the
# first change are batched into a single reindex pass. Default: 300.
# debounce_ms = 300

[vector_store]
# Enable the in-process semantic vector store (fastembed + SQLite BLOBs).
# Default: false while Phase 2 is shaking out. Flip to `true` after the
# first run has successfully downloaded the `multilingual-e5-small`
# ONNX weights (~118 MB). Legacy `[chroma]` table is still accepted.
# enabled = false

# Custom path for the vectors SQLite file. When unset, falls back to
# `StoreRoot::vectors_path()` under the memory root.
# data_path = "/absolute/path/to/vectors.sqlite"

[curation]
# Ask for a `memory_grounding_check` on any load-bearing factual claim
# in the assistant's response. Slowest of the curation knobs — opt-in.
# Default: false.
# grounding_check = false

# How new facts and lessons land in memory:
#   "auto"   — writes go straight to MEMORY.md / LESSONS.md (default).
#   "review" — writes land in *.pending.md; a human must `memory_promote`
#              (or the CLI equivalent) before they stick.
# memory_mode = "auto"

# Lessons whose failure ratio exceeds this (once they have at least
# `quarantine_min_attempts` attempts) are moved to LESSONS.quarantined.md
# during the forget pass. Default: 0.66 (≈ twice as many failures as
# successes).
# quarantine_failure_ratio = 0.66
# quarantine_min_attempts  = 5

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
