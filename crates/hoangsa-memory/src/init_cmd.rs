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
# Default: false — the `multilingual-e5-small` ONNX weights cost a
# ~465 MB shared cache plus a resident inference session, so semantic
# retrieval is opt-in; BM25 + symbol + graph retrieval work without it.
# To turn it on: set `enabled = true` below, then run
# `hoangsa-memory prefetch-embed` once so the first recall doesn't stall
# on the download. Legacy `[chroma]` table is still accepted.
# enabled = false

# Custom path for the vectors SQLite file. When unset, falls back to
# `StoreRoot::vectors_path()` under the memory root.
# data_path = "/absolute/path/to/vectors.sqlite"

[curation]
# Ask for a `memory_grounding_check` on any load-bearing factual claim
# in the assistant's response. Slowest of the curation knobs — opt-in.
# Default: false.
# grounding_check = false

# Lessons whose failure ratio exceeds this (once they have at least
# `quarantine_min_attempts` attempts) are moved to LESSONS.quarantined.md
# during the forget pass. Default: 0.66 (≈ twice as many failures as
# successes).
# quarantine_failure_ratio = 0.66
# quarantine_min_attempts  = 5

[dream]
# The dream pass: while the daemon is idle, a model re-reads MEMORY.md /
# LESSONS.md / USER.md and consolidates them — merging duplicates, rewriting
# stale entries, dropping ones that are wrong or obsolete. This is the half
# the forget pass can't do: TTL and counters can't tell that a fact stopped
# being true. Run it by hand with `hoangsa-memory memory dream`.
#
# Off by default — it spends tokens.
# enabled = false

# What the pass may do with its verdicts:
#   "review" — proposals go to DREAM.md and nothing else is touched (default).
#   "auto"   — verdicts are applied. Dropped entries are archived to
#              <SURFACE>.dropped.md with the model's reason, and every op is
#              logged to memory-history.jsonl, so nothing vanishes silently.
# Anything not recognised as "auto" is treated as review.
# mode = "review"

# Don't dream until the project has been untouched this long (minutes),
# and never more often than this many hours apart.
# idle_minutes       = 20
# min_interval_hours = 12

# Skip the pass when the three surfaces hold fewer entries than this —
# there's nothing to consolidate yet.
# min_entries = 8

# Wall-clock limit for the model subprocess, in seconds.
# timeout_secs = 300

# Explicit argv for the model. The prompt is appended as the final argument.
# Empty (the default) means auto-detect: `claude -p`, then `codex exec`.
# The subprocess runs in the project root, so the model can verify a claim
# against the actual code before calling it stale.
# command = ["claude", "-p", "--model", "claude-sonnet-5"]

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

#[cfg(test)]
mod tests {
    use super::DEFAULT_CONFIG_TOML;

    /// The `[vector_store]` comment block, up to the next table header.
    fn vector_store_block() -> &'static str {
        let start = DEFAULT_CONFIG_TOML
            .find("[vector_store]")
            .expect("template must document [vector_store]");
        let rest = &DEFAULT_CONFIG_TOML[start + "[vector_store]".len()..];
        match rest.find("\n[") {
            Some(end) => &rest[..end],
            None => rest,
        }
    }

    /// The seeded template documented `Default: true` for a field whose
    /// `Default` impl is `false`, so a user reading their own config
    /// believed semantic retrieval was on while it was off. The text and
    /// the behaviour must agree.
    #[test]
    fn seeded_config_states_vector_store_opt_in() {
        let block = vector_store_block();
        assert!(
            !block.contains("Default: true"),
            "[vector_store] claims a true default; VectorStoreConfig::enabled defaults to false"
        );
        assert!(
            block.contains("Default: false"),
            "[vector_store] must state the real default"
        );
        // Leaving `enabled` commented out is what keeps the seeded default
        // false — an uncommented `enabled = true` would flip behaviour.
        assert!(
            !block.lines().any(|l| {
                let t = l.trim();
                !t.starts_with('#') && t.starts_with("enabled")
            }),
            "`enabled` must stay commented out in the seeded template"
        );
    }

    /// Behavioural half of the gate: parse the template we actually seed
    /// and confirm the vector store resolves to disabled.
    #[test]
    fn seeded_config_resolves_vector_store_disabled() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), DEFAULT_CONFIG_TOML).unwrap();
        let cfg = hoangsa_memory_retrieve::VectorStoreConfig::load_or_default_sync(dir.path());
        assert!(
            !cfg.enabled,
            "seeded config must leave the vector store off (opt-in)"
        );
    }

    /// `~118 MB` was the pre-`multilingual-e5-small` cache size; the real
    /// cache is ~465 MB. The same stale figure was already corrected once
    /// in `uninstall.sh` (see CHANGELOG 0.6.0), so it must not come back
    /// through the seeded config either.
    #[test]
    fn seeded_config_has_no_stale_model_size() {
        assert!(
            !DEFAULT_CONFIG_TOML.contains("118"),
            "template carries the stale ~118 MB model-cache figure"
        );
        assert!(
            vector_store_block().contains("465 MB"),
            "[vector_store] must state the real ~465 MB cache cost"
        );
    }
}
