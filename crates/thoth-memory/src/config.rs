//! Configuration types for the memory lifecycle layer.
//!
//! Owns [`MemoryConfig`], [`DisciplineConfig`], [`EnforcementConfig`], and
//! [`ActorPolicyConfig`], all of which are parsed from `<root>/config.toml`.

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
    /// Whether to invoke the LLM nudge at session end (Mode::Full only).
    pub enable_nudge: bool,
    /// Hard cap for `MEMORY.md` in bytes. Default 16384 (~4K tokens).
    /// A `thoth_remember_fact` that would push the file above this cap
    /// returns a structured [`CapExceededError`] instead of silently
    /// appending — the agent must call `thoth_memory_replace` or
    /// `thoth_memory_remove` first.
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
            enable_nudge: true,
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

/// TOML file schema — mirrors the `[memory]` and `[discipline]` tables in
/// `<root>/config.toml`. We deliberately do NOT `deny_unknown_fields` at
/// the top level because the same file also hosts `[index]`,
/// `[output]`, and other per-crate tables owned by other loaders.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct ConfigFile {
    pub(crate) memory: MemoryConfig,
    #[serde(default)]
    pub(crate) discipline: DisciplineConfig,
    #[serde(default)]
    pub(crate) enforcement: EnforcementConfig,
}

/// Enforcement-layer policy (DESIGN-SPEC REQ-28).
///
/// Controls the auto-promote / auto-demote engine, the recall-window used
/// by the outcome harvester, and the workflow-violation threshold consumed
/// by the gate binary. All fields have serde defaults so an absent
/// `[enforcement]` block deserialises to [`Self::default`] — the canonical
/// `2 / 2 / 3 / 3 / true / true` starting point.
///
/// Ownership note: this struct is parsed by the same `config.toml` loader
/// that owns `[memory]` and `[discipline]` (see `ConfigFile`). Keep defaults
/// aligned with the table in DESIGN-SPEC §"Assumptions & Decisions" #10–#12.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct EnforcementConfig {
    /// Successful applications of a lesson before it auto-promotes one
    /// tier (e.g. `nudge` → `require`). Default `2` — first hit is noise,
    /// second is pattern.
    #[serde(default = "default_promote_threshold")]
    pub promote_threshold: u32,
    /// Violations of a lesson before it auto-demotes one tier
    /// (`require` → `nudge` → `off`). Default `2`.
    #[serde(default = "default_demote_threshold")]
    pub demote_threshold: u32,
    /// How many turns back the gate looks for a `thoth_recall` event when
    /// enforcing `RequireRecall` rules. Default `3` — enough for a small
    /// task without trapping long-running mutation bursts.
    #[serde(default = "default_recall_window")]
    pub recall_within_turns: u32,
    /// Workflow violations within the rolling 7-day window that cause the
    /// gate to hard-block further mutations (user must run
    /// `thoth workflow reset`). Default `3`.
    #[serde(default = "default_workflow_threshold")]
    pub workflow_violation_threshold: u32,
    /// Master switch for the auto-promotion engine. When `false` the
    /// harvester still records outcomes but never rewrites lesson tiers —
    /// operators must promote manually via `thoth lesson promote`.
    /// Default `true`.
    #[serde(default = "default_true")]
    pub auto_promote: bool,
    /// Master switch for the auto-demotion engine. Mirrors `auto_promote`
    /// on the downward path. Default `true`.
    #[serde(default = "default_true")]
    pub auto_demote: bool,
}

fn default_promote_threshold() -> u32 {
    2
}

fn default_demote_threshold() -> u32 {
    2
}

fn default_recall_window() -> u32 {
    3
}

fn default_workflow_threshold() -> u32 {
    3
}

fn default_true() -> bool {
    true
}

impl Default for EnforcementConfig {
    fn default() -> Self {
        Self {
            promote_threshold: default_promote_threshold(),
            demote_threshold: default_demote_threshold(),
            recall_within_turns: default_recall_window(),
            workflow_violation_threshold: default_workflow_threshold(),
            auto_promote: default_true(),
            auto_demote: default_true(),
        }
    }
}

impl EnforcementConfig {
    /// Load `<root>/config.toml` if it exists, returning the `[enforcement]`
    /// table (or [`Self::default`] if the file / table are missing).
    pub async fn load_or_default(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "enforcement: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        Self::parse_or_default(&text, &path)
    }

    /// Sync twin of [`Self::load_or_default`] for callers that can't
    /// spin a tokio runtime (e.g. the gate hook binary).
    pub fn load_or_default_sync(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "enforcement: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        Self::parse_or_default(&text, &path)
    }

    fn parse_or_default(text: &str, path: &Path) -> Self {
        match toml::from_str::<ConfigFile>(text) {
            Ok(cf) => cf.enforcement,
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "enforcement: config.toml parse error, using defaults");
                Self::default()
            }
        }
    }
}

/// Enforcement policy for the memory-discipline loop.
///
/// Read by hook runners and by the plugin skills; the MCP server itself
/// doesn't enforce anything — it just exposes memory tools. What the
/// `discipline` block controls is **how loud** the plugin skills get when
/// they detect that a lesson was violated or a reflect-step was skipped.
///
/// Two modes:
///
/// - `soft` (default): the skill reminds the agent but never blocks.
/// - `strict`: the skill returns a hard `deny` to Claude Code hooks, which
///   aborts the tool call until the agent re-plans with lessons in hand.
///
/// `global_fallback = true` (default) lets the plugin fall back to the
/// user-level `~/.thoth/` memory when no project-local `.thoth/` exists,
/// so lessons travel across checkouts of scratch repos.
// NOTE: `deny_unknown_fields` stays off. All gate-v2 keys
// (`gate_window_*`, `gate_relevance_threshold`, `gate_telemetry_enabled`,
// `gate_bash_readonly_prefixes`, `[[discipline.policies]]`) now live on
// this struct — the previous two-loader-one-file split in 2026-04-17's
// `thoth-mcp/bin/gate.rs::DisciplineFile` was the hazard that masked
// `reflect_debt_*` when strictness was briefly enabled. Consolidating
// here means the gate binary reads exactly what the rest of the world
// sees; drift between the two is no longer possible.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct DisciplineConfig {
    /// `"soft"` (warn only) or `"strict"` (deny on violation). Default
    /// `"soft"` — match the principle of "nudge, don't yoke".
    pub mode: String,
    /// Fall back to `~/.thoth/` memory when the current project has no
    /// `.thoth/` directory. Default `true`.
    pub global_fallback: bool,
    /// Ask for a `thoth.reflect` pass after every tool call (`"every"`) or
    /// only at session end (`"end"`). Default `"end"` — avoid thrash on
    /// trivial edits.
    pub reflect_cadence: String,
    /// Ask for a `thoth.nudge` pass before destructive actions. Default
    /// `true`.
    pub nudge_before_write: bool,
    /// Ask for a `thoth.grounding_check` on any load-bearing factual claim
    /// in the assistant's response. Default `false` (opt-in — it's the
    /// slowest of the three).
    pub grounding_check: bool,
    /// How new facts and lessons land in memory:
    ///
    /// - `"auto"` (default) — `thoth_remember_fact` and
    ///   `thoth_remember_lesson` write straight to `MEMORY.md` / `LESSONS.md`.
    /// - `"review"` — writes land in `MEMORY.pending.md` / `LESSONS.pending.md`
    ///   and a human must run `thoth_memory_promote` (or the CLI equivalent)
    ///   to accept them. Rejected entries are archived with a reason.
    ///
    /// Teams that want hard curation should switch to `"review"`; teams that
    /// trust the agent can stay on `"auto"` and rely on the forget pass +
    /// confidence counters to prune bad memory later.
    pub memory_mode: String,
    /// In `strict` mode, the gate also requires a `nudge_invoked` event
    /// within [`Self::gate_window_secs`] before a `Write`/`Edit`/`Bash`
    /// tool call. This forces the agent to actually expand `thoth.nudge`
    /// (not just run a no-op `thoth_recall`). Default `false`.
    pub gate_require_nudge: bool,
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
    /// Reflection debt — number of **mutations** (successful Write/Edit/
    /// NotebookEdit tool calls, derived from `gate.jsonl`) since the last
    /// `thoth_remember_fact` / `thoth_remember_lesson` call (derived from
    /// `memory-history.jsonl`). Above [`Self::reflect_debt_nudge`] the
    /// hooks surface a soft reminder; above [`Self::reflect_debt_block`]
    /// the gate hard-blocks new mutations until the agent reflects.
    ///
    /// Rationale: pre-action recall is enforced by the gate, but
    /// post-action reflection was previously a prompt contract only —
    /// agents drift. This turns reflection into an enforced loop with
    /// the same mechanism (hook injection + PreToolUse block) that
    /// proved effective for recall.
    ///
    /// Default `10`. Set to `0` to disable the soft reminder.
    pub reflect_debt_nudge: u32,
    /// Reflection debt that triggers a hard gate block on mutations.
    /// Set `THOTH_DEFER_REFLECT=1` to bypass one session when the user
    /// genuinely wants to land a batch before reflecting.
    ///
    /// Default `20`. Set to `0` to disable the hard block.
    pub reflect_debt_block: u32,

    // ------------------------------------------------------------------
    // Gate-v2 keys. Consumed by `thoth-mcp/bin/gate.rs` to decide
    // pass / nudge / block on mutation tool calls. Owned here so the
    // project's `config.toml` has a single parser; see the struct-level
    // NOTE above for the hazard consolidation fixes.
    // ------------------------------------------------------------------
    /// Recency shortcut window (seconds). Any `query_issued` event
    /// within this window passes the gate without a relevance check.
    /// Accepts the legacy key name `gate_window_secs` via serde alias
    /// so existing v1-era configs continue to parse. Default `60`.
    #[serde(alias = "gate_window_secs")]
    pub gate_window_short_secs: u64,
    /// Relevance pool window (seconds). Past recalls up to this age
    /// are scored by token overlap with the incoming edit; best score
    /// ≥ `gate_relevance_threshold` passes. Default `1800` (30 min).
    pub gate_window_long_secs: u64,
    /// Containment ratio threshold in `[0.0, 1.0]`. `0.0` disables the
    /// relevance check (recency-only behaviour). Default `0.30`.
    pub gate_relevance_threshold: f64,
    /// Append every gate decision to `<root>/gate.jsonl` when `true`.
    /// Off by default — telemetry is opt-in. Consumed by the gate
    /// binary and by `ReflectionDebt::compute` (for mutation counts).
    pub gate_telemetry_enabled: bool,
    /// Additional Bash command prefixes that bypass the gate.
    /// Additive to the hard-coded built-ins (`cargo test`, `git status`,
    /// `thoth curate`, …) — entries here *extend* the whitelist, they
    /// don't replace it. Default empty.
    pub gate_bash_readonly_prefixes: Vec<String>,
    /// Actor-specific overrides. First entry whose `actor` glob matches
    /// the `THOTH_ACTOR` env var wins; the default policy applies when
    /// none match. Default empty.
    pub policies: Vec<ActorPolicyConfig>,

    // ------------------------------------------------------------------
    // Background review. A lightweight fork of the Hermes-style
    // "background review" that spawns `claude -p` (subscription) or
    // calls the Anthropic API directly to auto-persist facts/lessons
    // mid-session.
    // ------------------------------------------------------------------
    /// Enable periodic background reviews. When `true`, the PostToolUse
    /// hook spawns a detached `thoth review` process every
    /// [`Self::background_review_interval`] mutations. Default `false`
    /// (opt-in).
    pub background_review: bool,
    /// Number of mutations (Write/Edit/NotebookEdit) between background
    /// reviews. Default `50`. Was `10` pre-2026-04-18 but that fires so
    /// often a 50-mutation session can spawn 5+ reviews, each running
    /// through the whole MEMORY.md/LESSONS.md and tending to snowball
    /// reworded near-duplicates.
    pub background_review_interval: u32,
    /// Minimum seconds between two background reviews, regardless of
    /// mutation count. A hard floor on spawn rate so a rapid burst of
    /// edits can't fire back-to-back reviews. Default `600` (10 min).
    pub background_review_min_secs: u64,
    /// Backend for the review LLM call. `"auto"` checks
    /// `ANTHROPIC_API_KEY` → API, else `claude` CLI (subscription).
    /// `"cli"` forces the CLI path. `"api"` forces the API path.
    /// Default `"auto"`.
    pub background_review_backend: String,
    /// Model name passed to the backend. For the CLI backend this
    /// becomes `claude --model <name>`; for the API backend it's the
    /// `model` field in the request body. Default
    /// `"claude-haiku-4-5"` — Haiku is plenty for memory curation, and
    /// leaving this unset on the CLI path previously meant reviews ran
    /// under whatever default the user's interactive session uses
    /// (often Opus), burning tokens.
    pub background_review_model: String,

    /// How many `MEMORY.md.bak-*` / `LESSONS.md.bak-*` files `thoth
    /// compact` keeps around after a successful rewrite. Older
    /// backups (by filename timestamp) are deleted. `0` disables
    /// pruning (keep every backup forever). Default `2`.
    pub compact_backup_keep: u32,
}

/// Per-actor gate policy override. Missing fields inherit from the
/// top-level [`DisciplineConfig`] defaults. Mirrors
/// `[[discipline.policies]]` in `config.toml`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct ActorPolicyConfig {
    /// Glob matching the `THOTH_ACTOR` env var (e.g. `"hoangsa/*"`,
    /// `"ci-*"`, `"*"`). Empty string matches nothing.
    pub actor: String,
    /// Gate mode for this actor. `"off"` / `"nudge"` / `"strict"`. When
    /// `None`, inherits [`DisciplineConfig::mode`] / the gate's default.
    pub mode: Option<String>,
    /// Recency window override in seconds. Inherits when `None`.
    pub window_short_secs: Option<u64>,
    /// Relevance window override in seconds. Inherits when `None`.
    pub window_long_secs: Option<u64>,
    /// Relevance threshold override in `[0.0, 1.0]`. Inherits when `None`.
    pub relevance_threshold: Option<f64>,
}

/// Gate-v2 defaults, exposed so the gate binary and the config layer
/// agree on concrete numbers without duplicating literals.
pub mod gate_defaults {
    /// Default recency shortcut window in seconds.
    pub const WINDOW_SHORT_SECS: u64 = 60;
    /// Default relevance pool window in seconds.
    pub const WINDOW_LONG_SECS: u64 = 1800;
    /// Default containment-ratio threshold.
    pub const RELEVANCE_THRESHOLD: f64 = 0.30;
}

impl Default for DisciplineConfig {
    fn default() -> Self {
        Self {
            mode: "soft".to_string(),
            global_fallback: true,
            reflect_cadence: "end".to_string(),
            nudge_before_write: true,
            grounding_check: false,
            memory_mode: "auto".to_string(),
            gate_require_nudge: false,
            quarantine_failure_ratio: 0.66,
            quarantine_min_attempts: 5,
            reflect_debt_nudge: 10,
            reflect_debt_block: 20,
            gate_window_short_secs: gate_defaults::WINDOW_SHORT_SECS,
            gate_window_long_secs: gate_defaults::WINDOW_LONG_SECS,
            gate_relevance_threshold: gate_defaults::RELEVANCE_THRESHOLD,
            gate_telemetry_enabled: false,
            gate_bash_readonly_prefixes: Vec::new(),
            policies: Vec::new(),
            background_review: false,
            background_review_interval: 50,
            background_review_min_secs: 600,
            background_review_backend: "auto".to_string(),
            background_review_model: "claude-haiku-4-5".to_string(),
            compact_backup_keep: 2,
        }
    }
}

impl DisciplineConfig {
    /// `true` if new memory should be staged (pending) instead of
    /// auto-committed.
    pub fn requires_review(&self) -> bool {
        self.memory_mode.eq_ignore_ascii_case("review")
    }

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
                    "discipline: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        Self::parse_or_default(&text, &path)
    }

    /// Sync twin of [`Self::load_or_default`] for callers that can't
    /// spin a tokio runtime (the `thoth-gate` hook binary).
    pub fn load_or_default_sync(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "discipline: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        Self::parse_or_default(&text, &path)
    }

    fn parse_or_default(text: &str, path: &Path) -> Self {
        match toml::from_str::<ConfigFile>(text) {
            Ok(cf) => cf.discipline,
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "discipline: config.toml parse error, using defaults");
                Self::default()
            }
        }
    }

}

#[cfg(test)]
mod config_tests {
    //! Config-layer serde tests (DESIGN-SPEC REQ-28).
    use super::*;

    /// An empty `[enforcement]` table must deserialise to the canonical
    /// `2 / 2 / 3 / 3 / true / true` default set.
    #[test]
    fn enforcement_defaults() {
        // Empty table → all serde defaults.
        let cfg: EnforcementConfig = toml::from_str("").expect("empty toml parses");
        assert_eq!(cfg.promote_threshold, 2);
        assert_eq!(cfg.demote_threshold, 2);
        assert_eq!(cfg.recall_within_turns, 3);
        assert_eq!(cfg.workflow_violation_threshold, 3);
        assert!(cfg.auto_promote);
        assert!(cfg.auto_demote);

        // `Default` impl must match the serde path exactly.
        let def = EnforcementConfig::default();
        assert_eq!(def.promote_threshold, cfg.promote_threshold);
        assert_eq!(def.demote_threshold, cfg.demote_threshold);
        assert_eq!(def.recall_within_turns, cfg.recall_within_turns);
        assert_eq!(
            def.workflow_violation_threshold,
            cfg.workflow_violation_threshold
        );
        assert_eq!(def.auto_promote, cfg.auto_promote);
        assert_eq!(def.auto_demote, cfg.auto_demote);

        // Missing `[enforcement]` section inside a bigger config also
        // yields defaults — guards against accidental `deny_unknown_fields`
        // regressions on `ConfigFile`.
        let file: ConfigFile =
            toml::from_str("[memory]\n").expect("config.toml without enforcement parses");
        assert_eq!(file.enforcement.promote_threshold, 2);
        assert!(file.enforcement.auto_promote);

        // Partial override keeps untouched defaults intact.
        let partial: EnforcementConfig =
            toml::from_str("promote_threshold = 5\nauto_demote = false\n")
                .expect("partial toml parses");
        assert_eq!(partial.promote_threshold, 5);
        assert_eq!(partial.demote_threshold, 2);
        assert!(partial.auto_promote);
        assert!(!partial.auto_demote);
    }

    // --- T-24 gap coverage: EnforcementConfig edge cases --------------------

    /// A fully-populated `[enforcement]` block round-trips through TOML
    /// without loss — guards serialize/deserialize symmetry.
    #[test]
    fn enforcement_full_override_roundtrips() {
        let src = "promote_threshold = 7\n\
                   demote_threshold = 8\n\
                   recall_within_turns = 9\n\
                   workflow_violation_threshold = 10\n\
                   auto_promote = false\n\
                   auto_demote = false\n";
        let cfg: EnforcementConfig = toml::from_str(src).expect("full override parses");
        assert_eq!(cfg.promote_threshold, 7);
        assert_eq!(cfg.demote_threshold, 8);
        assert_eq!(cfg.recall_within_turns, 9);
        assert_eq!(cfg.workflow_violation_threshold, 10);
        assert!(!cfg.auto_promote);
        assert!(!cfg.auto_demote);

        // Re-serialize and parse back — value-identical.
        let s = toml::to_string(&cfg).expect("reserializes");
        let back: EnforcementConfig = toml::from_str(&s).expect("reparses");
        assert_eq!(back.promote_threshold, cfg.promote_threshold);
        assert_eq!(back.demote_threshold, cfg.demote_threshold);
        assert_eq!(back.recall_within_turns, cfg.recall_within_turns);
        assert_eq!(
            back.workflow_violation_threshold,
            cfg.workflow_violation_threshold
        );
        assert_eq!(back.auto_promote, cfg.auto_promote);
        assert_eq!(back.auto_demote, cfg.auto_demote);
    }

    /// Wrong value type (string where u32 expected) must fail to parse —
    /// we do NOT want silent fallback to default that would mask a config
    /// typo. Complements the positive default / partial tests above.
    #[test]
    fn enforcement_wrong_type_rejected() {
        let bad = "promote_threshold = \"two\"\n";
        let err = toml::from_str::<EnforcementConfig>(bad);
        assert!(err.is_err(), "non-int must error, got {:?}", err);
    }

    /// Unknown keys in `[enforcement]` are ignored (no
    /// `deny_unknown_fields`) — guards against a future regression that
    /// would break `config.toml` forward-compat when new keys are added.
    #[test]
    fn enforcement_unknown_key_ignored() {
        let src = "promote_threshold = 4\n\
                   future_knob = 42\n";
        let cfg: EnforcementConfig = toml::from_str(src).expect("unknown key must not error");
        assert_eq!(cfg.promote_threshold, 4);
        assert!(cfg.auto_promote);
    }

    /// Boolean flip of only `auto_promote` leaves `auto_demote` at its
    /// default — exercises the per-field #[serde(default = ...)] wiring
    /// distinct from the struct-level `#[serde(default)]`.
    #[test]
    fn enforcement_single_bool_flip() {
        let src = "auto_promote = false\n";
        let cfg: EnforcementConfig = toml::from_str(src).expect("parses");
        assert!(!cfg.auto_promote);
        assert!(cfg.auto_demote, "auto_demote default preserved");
        assert_eq!(cfg.promote_threshold, 2);
    }
}
