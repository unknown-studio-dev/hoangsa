//! Configuration types for the memory lifecycle layer.
//!
//! Owns [`MemoryConfig`] and [`CurationConfig`], parsed from
//! `<root>/config.toml`.

use std::path::Path;

/// Config controlling the memory lifecycle.
///
/// Loaded from `<root>/config.toml` via [`MemoryConfig::load_or_default`].
/// Unknown keys are ignored and missing keys fall back to the compiled
/// defaults (equivalent to [`MemoryConfig::default`]).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryConfig {
    /// Episodic TTL in days. Default 30.
    pub episodic_ttl_days: u32,
    /// Max number of episodes retained before capacity-based eviction.
    pub max_episodes: usize,
    /// Lesson confidence floor. Below this ratio (success / (success +
    /// failure + 1)) a lesson is considered harmful and dropped — but only
    /// once it has [`MemoryConfig::lesson_min_attempts`] attempts on record.
    pub lesson_floor: f32,
    /// Minimum number of success+failure attempts before a lesson can be
    /// dropped for low confidence. Prevents a single unlucky pass from
    /// killing a freshly-minted lesson.
    pub lesson_min_attempts: u32,
    /// Exponential decay rate per day (DESIGN §9).
    /// `effective = salience · exp(-λ·days_idle) · ln(e + access_count)`.
    /// At the default `λ=0.02` a never-retrieved memory decays to ~0.67
    /// of its original salience after 30 days, ~0.45 after 60 days.
    pub decay_lambda: f32,
    /// Retention floor for the decay formula. Memories whose effective
    /// score falls below this are dropped by the forget pass. A value of
    /// `0.0` disables decay-based eviction.
    pub decay_floor: f32,
    /// Hard cap for `MEMORY.md` in bytes. Default 16384 (~4K tokens).
    /// A `memory_remember_fact` that would push the file above this cap
    /// returns a structured [`CapExceededError`] instead of silently
    /// appending — the agent must call `memory_replace` or
    /// `memory_remove` first.
    ///
    /// Sized so USER + MEMORY + LESSONS combined inject < ~10K tokens
    /// (< 5% of a 200K context window) at SessionStart.
    #[serde(default = "default_cap_memory_bytes")]
    pub cap_memory_bytes: usize,
    /// Hard cap for `USER.md` in bytes. Default 4096 (~1K tokens).
    #[serde(default = "default_cap_user_bytes")]
    pub cap_user_bytes: usize,
    /// Hard cap for `LESSONS.md` in bytes. Default 16384 (~4K tokens).
    #[serde(default = "default_cap_lessons_bytes")]
    pub cap_lessons_bytes: usize,
    /// FLEXIBLE content policy (DESIGN-SPEC REQ-12). When `false` (default)
    /// MCP tool handlers only log a warning if a `remember_*` payload looks
    /// like a bare commit sha / ISO date / file path with no invariant.
    /// When `true`, such payloads are rejected with a structured error.
    #[serde(default)]
    pub strict_content_policy: bool,
}

fn default_cap_memory_bytes() -> usize {
    16_384
}

fn default_cap_user_bytes() -> usize {
    4_096
}

fn default_cap_lessons_bytes() -> usize {
    16_384
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            episodic_ttl_days: 30,
            max_episodes: 50_000,
            lesson_floor: 0.2,
            lesson_min_attempts: 3,
            decay_lambda: 0.02,
            decay_floor: 0.05,
            cap_memory_bytes: default_cap_memory_bytes(),
            cap_user_bytes: default_cap_user_bytes(),
            cap_lessons_bytes: default_cap_lessons_bytes(),
            strict_content_policy: false,
        }
    }
}

impl MemoryConfig {
    /// Load `<root>/config.toml` if it exists, otherwise fall back to
    /// [`MemoryConfig::default`]. Malformed files emit a `warn!` and still
    /// fall back — the user's memory must not become unusable because they
    /// mistyped a key.
    pub async fn load_or_default(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "memory: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        match toml::from_str::<ConfigFile>(&text) {
            Ok(cf) => cf.memory,
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "memory: config.toml parse error, using defaults");
                Self::default()
            }
        }
    }
}

/// TOML file schema — mirrors the `[memory]` and `[curation]` tables in
/// `<root>/config.toml`. We deliberately do NOT `deny_unknown_fields` at
/// the top level because the same file also hosts `[index]`,
/// `[output]`, and other per-crate tables owned by other loaders.
///
/// The `curation` field accepts the legacy table name `[discipline]` via
/// serde alias — existing configs keep working without edits.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct ConfigFile {
    pub(crate) memory: MemoryConfig,
    #[serde(default, alias = "discipline")]
    pub(crate) curation: CurationConfig,
    pub(crate) dream: DreamConfig,
}

/// Knobs for the background **dream** pass — the LLM-driven consolidation
/// sweep that re-reads `MEMORY.md` / `LESSONS.md` / `USER.md` and merges,
/// drops, or flags entries that have gone stale or wrong.
///
/// Distinct from the deterministic forget pass ([`MemoryConfig`]): that one
/// prunes on counters and clocks, this one reads the *content* and needs a
/// model to do it. Both run from the same daemon loop.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct DreamConfig {
    /// Master switch. Default `false` — dreaming spends tokens, so it is
    /// opt-in per project.
    pub enabled: bool,
    /// What the dream pass is allowed to do with its verdicts:
    ///
    /// - `"review"` (default) — write every proposal to `DREAM.md` and touch
    ///   nothing else. The user reads it and applies what they agree with.
    /// - `"auto"` — apply merges and drops directly, archiving each dropped
    ///   entry to `<SURFACE>.dropped.md` with the model's reason, and logging
    ///   every op to `memory-history.jsonl` so it can be audited or reverted.
    pub mode: String,
    /// Minimum idle time (minutes, no MCP request served) before the daemon
    /// will start a dream. Keeps the sweep off the critical path while the
    /// user is actively working.
    pub idle_minutes: u64,
    /// Floor on how often a project may dream, in hours. Also enforced
    /// across daemon restarts via the `last_dream_at` marker file.
    pub min_interval_hours: u64,
    /// Skip the pass entirely when the three surfaces hold fewer than this
    /// many entries combined — there is nothing to consolidate yet.
    pub min_entries: usize,
    /// Hard wall-clock limit for the model subprocess, in seconds.
    pub timeout_secs: u64,
    /// Explicit argv for the model subprocess, e.g.
    /// `["claude", "-p", "--model", "claude-sonnet-5"]`. The prompt is
    /// appended as the final argument. Empty (default) means auto-detect:
    /// `claude` first, then `codex exec`.
    pub command: Vec<String>,
}

impl Default for DreamConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: "review".to_string(),
            idle_minutes: 20,
            min_interval_hours: 12,
            min_entries: 8,
            timeout_secs: 300,
            command: Vec::new(),
        }
    }
}

impl DreamConfig {
    /// `true` if the pass may write to the canonical surfaces itself.
    /// Anything other than `"auto"` is treated as review — an unrecognised
    /// mode must fail closed, not silently grant write access.
    pub fn is_auto(&self) -> bool {
        self.mode.eq_ignore_ascii_case("auto")
    }

    /// Load `<root>/config.toml`, falling back to defaults on a missing or
    /// malformed file — same tolerant contract as the sibling loaders.
    pub async fn load_or_default(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "dream: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        match toml::from_str::<ConfigFile>(&text) {
            Ok(cf) => cf.dream,
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "dream: config.toml parse error, using defaults");
                Self::default()
            }
        }
    }
}

/// Live policy knobs for the memory-curation loop.
///
/// Read by the MCP server to decide whether the `memory.grounding_check`
/// prompt is advertised, and by the forget pass to quarantine lessons with
/// a bad success ratio.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct CurationConfig {
    /// Ask for a `memory_grounding_check` on any load-bearing factual claim
    /// in the assistant's response. Default `false` (opt-in — it's the
    /// slowest of the three).
    pub grounding_check: bool,
    /// Lessons whose `failure_count / (success_count + failure_count)`
    /// exceeds this ratio (once they have at least
    /// [`Self::quarantine_min_attempts`] attempts) are moved from
    /// `LESSONS.md` to `LESSONS.quarantined.md` during the forget pass.
    /// Default `0.66` — i.e. twice as many failures as successes.
    pub quarantine_failure_ratio: f32,
    /// Minimum `success_count + failure_count` before a lesson is eligible
    /// for quarantine. Default `5` — a freshly minted lesson with one
    /// failure shouldn't get yanked.
    pub quarantine_min_attempts: u32,
}

impl Default for CurationConfig {
    fn default() -> Self {
        Self {
            grounding_check: false,
            quarantine_failure_ratio: 0.66,
            quarantine_min_attempts: 5,
        }
    }
}

impl CurationConfig {
    /// Load `<root>/config.toml` if it exists, else return defaults.
    ///
    /// Same tolerant behaviour as [`MemoryConfig::load_or_default`]: missing
    /// file → defaults, malformed file → warn + defaults.
    pub async fn load_or_default(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "curation: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        Self::parse_or_default(&text, &path)
    }

    /// Sync twin of [`Self::load_or_default`] for callers that can't
    /// spin a tokio runtime (the `hoangsa-cli enforce` hook binary).
    pub fn load_or_default_sync(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "curation: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        Self::parse_or_default(&text, &path)
    }

    fn parse_or_default(text: &str, path: &Path) -> Self {
        match toml::from_str::<ConfigFile>(text) {
            Ok(cf) => cf.curation,
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "curation: config.toml parse error, using defaults");
                Self::default()
            }
        }
    }
}
