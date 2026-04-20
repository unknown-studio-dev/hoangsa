//! `thoth setup` — the one-shot bootstrap for Thoth.
//!
//! Running `thoth setup` is enough to take a fresh repo from zero to a
//! fully-wired integration:
//!
//! 1. Asks a handful of questions (mode, memory handling, ignore
//!    patterns, gate knobs).
//! 2. Writes `<root>/config.toml` and seeds `MEMORY.md` + `LESSONS.md`.
//! 3. Installs hooks + skills + MCP server into `.claude/settings.json`.
//!
//! When run a second time it detects the existing install, shows a
//! status summary, and offers to reconfigure without nuking user-owned
//! edits.
//!
//! `--status` prints the detected state and exits. `--yes` (alias
//! `--accept-defaults`) and non-TTY stdout both skip the wizard and use
//! safe defaults — useful for scripted installs and CI.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
use serde_json::Value;

use crate::hooks;

/// Embedded seed body for `USER.md` — personal preferences/style file.
/// Written once on first setup; never overwritten.
const USER_MD_TEMPLATE: &str = include_str!("../assets/USER.md.template");

// Public knob keys — kept in sync with `DisciplineConfig` and the gate
// binary's `PolicyMode` parser. `nudge` is the default (pass on miss +
// stderr warning); `strict` blocks; `off` disables the gate entirely.
const MODES: &[&str] = &["nudge", "strict", "off"];
const MEMORY_MODES: &[&str] = &["auto", "review"];
const CADENCES: &[&str] = &["end", "every"];

/// Relevance threshold default — balanced setting. See the comment block
/// rendered into `config.toml` for the guidance range.
const DEFAULT_RELEVANCE_THRESHOLD: f64 = 0.30;
const DEFAULT_WINDOW_SHORT_SECS: u64 = 60;
const DEFAULT_WINDOW_LONG_SECS: u64 = 1800;

/// Default ignore patterns suggested by the wizard. The user can accept
/// them verbatim or replace them. These are layered on top of the
/// project's `.gitignore` / `.ignore`, not a replacement for them.
const DEFAULT_IGNORE_PATTERNS: &[&str] = &[
    "target/",
    "node_modules/",
    "dist/",
    "build/",
    ".venv/",
    "*.generated.rs",
];

#[derive(Clone, Debug)]
pub struct SetupAnswers {
    /// Gate verdict on a miss — `nudge` (default) / `strict` / `off`.
    pub mode: String,
    pub memory_mode: String,
    pub reflect_cadence: String,
    /// Recency shortcut — recall within this window passes automatically
    /// without a relevance check. Keep short (default 60s).
    pub gate_window_short_secs: u64,
    /// Relevance pool — how far back the gate looks for a topically-
    /// matching recall when recency alone doesn't cover it. Default
    /// 30min = typical coding-session span.
    pub gate_window_long_secs: u64,
    /// Containment threshold in `[0.0, 1.0]`. `0.0` disables relevance
    /// (time-only legacy behavior). See the `config.toml` comment block
    /// for the range guidance.
    pub gate_relevance_threshold: f64,
    /// Append every gate decision to `.thoth/gate.jsonl` — handy for
    /// calibrating the threshold. Opt-in because the file grows.
    pub gate_telemetry_enabled: bool,
    pub nudge_before_write: bool,
    pub global_fallback: bool,
    pub ignore: Vec<String>,
    /// Auto-watch source tree for changes and reindex in-process within
    /// the MCP daemon. Removes the need for a separate `thoth watch`.
    pub watch_enabled: bool,
    /// Debounce window for the auto-watcher (milliseconds).
    pub watch_debounce_ms: u64,
    /// Spawn a background `thoth review` every N mutations to
    /// auto-persist facts/lessons. Requires gate_telemetry_enabled.
    pub background_review: bool,
    /// Mutations between background reviews.
    pub background_review_interval: u32,
    /// Minimum seconds between reviews (time-based cooldown).
    pub background_review_min_secs: u64,
    /// Backend: "auto", "cli", or "api".
    pub background_review_backend: String,
    /// Model name passed to the backend (e.g. `claude-haiku-4-5`).
    pub background_review_model: String,
    /// How many MEMORY/LESSONS backups `thoth compact` keeps.
    pub compact_backup_keep: u32,
    /// Debt threshold that triggers the statusline / prompt nudge.
    pub reflect_debt_nudge: u32,
    /// Debt threshold that hard-blocks further mutations.
    pub reflect_debt_block: u32,
    /// Require fresh recall right before every edit (on top of relevance).
    pub gate_require_nudge: bool,
    /// Opt-in grounding check on factual claims.
    pub grounding_check: bool,
    /// Auto-quarantine lessons whose failure ratio crosses this.
    pub quarantine_failure_ratio: f64,
    /// Minimum attempts before quarantine is eligible.
    pub quarantine_min_attempts: u32,
}

impl Default for SetupAnswers {
    fn default() -> Self {
        Self {
            mode: "nudge".to_string(),
            memory_mode: "auto".to_string(),
            reflect_cadence: "end".to_string(),
            gate_window_short_secs: DEFAULT_WINDOW_SHORT_SECS,
            gate_window_long_secs: DEFAULT_WINDOW_LONG_SECS,
            gate_relevance_threshold: DEFAULT_RELEVANCE_THRESHOLD,
            gate_telemetry_enabled: false,
            nudge_before_write: true,
            global_fallback: true,
            ignore: DEFAULT_IGNORE_PATTERNS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            watch_enabled: false,
            watch_debounce_ms: 300,
            background_review: false,
            background_review_interval: 50,
            background_review_min_secs: 600,
            background_review_backend: "auto".to_string(),
            background_review_model: "claude-haiku-4-5".to_string(),
            compact_backup_keep: 2,
            reflect_debt_nudge: 10,
            reflect_debt_block: 20,
            gate_require_nudge: false,
            grounding_check: false,
            quarantine_failure_ratio: 0.66,
            quarantine_min_attempts: 5,
        }
    }
}

/// Snapshot of what's already installed. Used to branch the bootstrap
/// between "fresh install" and "reconfigure" flows.
#[derive(Clone, Debug, Default)]
struct InstallState {
    config_path: Option<PathBuf>,
    memory_exists: bool,
    lessons_exists: bool,
    hooks_installed: bool,
    mcp_installed: bool,
    skills_installed: Vec<String>,
    watch_enabled: bool,
}

impl InstallState {
    fn is_bootstrapped(&self) -> bool {
        self.config_path.is_some() && (self.hooks_installed || self.mcp_installed)
    }
}

/// Entry point for `thoth setup`.
pub async fn run(root: &Path, status: bool, accept_defaults: bool) -> Result<()> {
    let state = detect_state(root).await?;

    if status {
        print_status(root, &state);
        return Ok(());
    }

    tokio::fs::create_dir_all(root)
        .await
        .with_context(|| format!("create {}", root.display()))?;

    let non_interactive = accept_defaults || !std::io::stdout().is_terminal();

    // If already bootstrapped and we're non-interactive, there's nothing
    // meaningful to do — just re-run the install step idempotently so
    // hook entries self-heal if the bundle shipped a new matcher.
    if state.is_bootstrapped() && non_interactive {
        println!("thoth setup: already bootstrapped at {}.", root.display());
        println!("  (non-interactive; re-running install for self-heal)");
        reinstall_integration(root).await?;
        print_final_message(root, /* reconfigured */ true);
        return Ok(());
    }

    // Already bootstrapped in interactive mode — just confirm and exit.
    // Use `thoth setup --yes` to self-heal hooks/MCP idempotently.
    if state.is_bootstrapped() && !non_interactive {
        print_status(root, &state);
        println!("\nAlready set up — nothing to do.");
        println!("  Run `thoth setup --yes` to self-heal hooks + MCP.");
        return Ok(());
    }

    let answers = if non_interactive {
        println!("thoth setup: non-interactive — writing defaults.");
        SetupAnswers::default()
    } else {
        prompt_interactive(&state)?
    };

    // Write config, seed markdown + .thothignore, then wire the integration.
    write_config(root, &answers).await?;
    seed_markdown(root).await?;
    seed_thothignore(root).await?;
    install_integration(root).await?;

    print_summary(root, &answers);
    print_final_message(root, state.is_bootstrapped());
    Ok(())
}

// --------------------------------------------------------------- detection

async fn detect_state(root: &Path) -> Result<InstallState> {
    let mut s = InstallState::default();

    let cfg = root.join("config.toml");
    if cfg.exists() {
        s.config_path = Some(cfg);
        let watch_cfg = thoth_retrieve::WatchConfig::load_or_default(root).await;
        s.watch_enabled = watch_cfg.enabled;
    }
    s.memory_exists = root.join("MEMORY.md").exists();
    s.lessons_exists = root.join("LESSONS.md").exists();

    // Probe `.claude/settings.json` for thoth-managed hook entries.
    let settings = PathBuf::from(".claude").join("settings.json");
    if let Ok(text) = tokio::fs::read_to_string(&settings).await
        && let Ok(v) = serde_json::from_str::<Value>(&text)
    {
        s.hooks_installed = has_thoth_managed_hooks(&v);
    }

    // Probe `.mcp.json` (Claude Code project-scope MCP config) for our
    // server entry. MCP config does NOT live in settings.json — Claude Code
    // ignores `mcpServers` there.
    let mcp_config = PathBuf::from(".mcp.json");
    if let Ok(text) = tokio::fs::read_to_string(&mcp_config).await
        && let Ok(v) = serde_json::from_str::<Value>(&text)
    {
        s.mcp_installed = v.get("mcpServers").and_then(|m| m.get("thoth")).is_some();
    }

    // Probe `.claude/skills/` for the ones we ship.
    let skills_dir = PathBuf::from(".claude").join("skills");
    if skills_dir.exists()
        && let Ok(mut rd) = tokio::fs::read_dir(&skills_dir).await
    {
        while let Ok(Some(entry)) = rd.next_entry().await {
            if let Some(name) = entry.file_name().to_str() {
                s.skills_installed.push(name.to_string());
            }
        }
        s.skills_installed.sort();
    }

    Ok(s)
}

fn has_thoth_managed_hooks(settings: &Value) -> bool {
    let Some(hooks) = settings.get("hooks").and_then(Value::as_object) else {
        return false;
    };
    for entries in hooks.values() {
        let Some(list) = entries.as_array() else {
            continue;
        };
        for entry in list {
            if entry
                .get("_thoth_managed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}

fn print_status(root: &Path, s: &InstallState) {
    println!("thoth status @ {}", root.display());
    println!(
        "  config.toml   : {}",
        s.config_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "missing".to_string())
    );
    println!(
        "  MEMORY.md     : {}",
        if s.memory_exists {
            "present"
        } else {
            "missing"
        }
    );
    println!(
        "  LESSONS.md    : {}",
        if s.lessons_exists {
            "present"
        } else {
            "missing"
        }
    );
    println!(
        "  hooks         : {}",
        if s.hooks_installed {
            "installed (project scope)"
        } else {
            "not installed"
        }
    );
    println!(
        "  MCP server    : {}",
        if s.mcp_installed {
            "registered"
        } else {
            "not registered"
        }
    );
    println!(
        "  skills        : {}",
        if s.skills_installed.is_empty() {
            "none".to_string()
        } else {
            s.skills_installed.join(", ")
        }
    );
    println!(
        "  auto-watch    : {}",
        if s.watch_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
}

// -------------------------------------------------------------- wizard

fn prompt_interactive(state: &InstallState) -> Result<SetupAnswers> {
    let theme = ColorfulTheme::default();

    println!("thoth setup — interactive wizard");
    println!("Press enter to accept the highlighted default.\n");

    let mode_idx = Select::with_theme(&theme)
        .with_prompt("Gate mode (nudge = warn on miss, strict = block on miss, off = disabled)")
        .items(MODES)
        .default(0)
        .interact()?;
    let mode = MODES[mode_idx].to_string();

    let memory_mode_idx = Select::with_theme(&theme)
        .with_prompt("Memory mode (auto = commit straight away, review = stage for approval)")
        .items(MEMORY_MODES)
        .default(0)
        .interact()?;
    let memory_mode = MEMORY_MODES[memory_mode_idx].to_string();

    let cadence_idx = Select::with_theme(&theme)
        .with_prompt("Reflect cadence (end = only on Stop, every = after each tool call)")
        .items(CADENCES)
        .default(0)
        .interact()?;
    let reflect_cadence = CADENCES[cadence_idx].to_string();

    // Relevance-related knobs only matter when the gate isn't `off`.
    // Skip the prompts entirely in that case so the wizard stays short.
    let (gate_window_short_secs, gate_window_long_secs, gate_relevance_threshold) = if mode == "off"
    {
        (
            DEFAULT_WINDOW_SHORT_SECS,
            DEFAULT_WINDOW_LONG_SECS,
            DEFAULT_RELEVANCE_THRESHOLD,
        )
    } else {
        let short: u64 = Input::with_theme(&theme)
            .with_prompt(
                "Recency window in seconds (recall within this passes without relevance check)",
            )
            .default(DEFAULT_WINDOW_SHORT_SECS)
            .interact_text()?;
        let long: u64 = Input::with_theme(&theme)
            .with_prompt(
                "Relevance pool window in seconds (how far back to search for a topical recall)",
            )
            .default(DEFAULT_WINDOW_LONG_SECS)
            .interact_text()?;
        let threshold: f64 = Input::with_theme(&theme)
            .with_prompt(
                "Relevance threshold [0.0=off, 0.15=permissive, 0.30=balanced, 0.50=strict]",
            )
            .default(DEFAULT_RELEVANCE_THRESHOLD)
            .interact_text()?;
        (short, long, threshold.clamp(0.0, 1.0))
    };

    let gate_telemetry_enabled = Confirm::with_theme(&theme)
        .with_prompt("Append every gate decision to .thoth/gate.jsonl? (useful for tuning)")
        .default(false)
        .interact()?;

    let nudge_before_write = Confirm::with_theme(&theme)
        .with_prompt("Enable the gate at all? (master switch — uncheck to disable everything)")
        .default(true)
        .interact()?;

    let global_fallback = Confirm::with_theme(&theme)
        .with_prompt("Fall back to `~/.thoth/` when this project has no local store?")
        .default(true)
        .interact()?;

    // Ignore patterns — suggest defaults, offer a comma-separated edit.
    println!();
    println!("Ignore patterns (layered on top of .gitignore / .ignore).");
    println!("  suggested: {}", DEFAULT_IGNORE_PATTERNS.join(", "));
    let use_defaults = Confirm::with_theme(&theme)
        .with_prompt("Use the suggested ignore patterns?")
        .default(true)
        .interact()?;
    let ignore = if use_defaults {
        DEFAULT_IGNORE_PATTERNS
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        let raw: String = Input::with_theme(&theme)
            .with_prompt("Comma-separated patterns (empty = none)")
            .default(DEFAULT_IGNORE_PATTERNS.join(", "))
            .allow_empty(true)
            .interact_text()?;
        parse_ignore_csv(&raw)
    };

    // Auto-watch: reindex source changes in the MCP daemon background.
    let watch_enabled = Confirm::with_theme(&theme)
        .with_prompt("Auto-watch source tree? (reindex on file change, no separate `thoth watch`)")
        .default(false)
        .interact()?;

    // Background review: Hermes-style auto-persist of facts/lessons.
    let background_review = Confirm::with_theme(&theme)
        .with_prompt("Enable background review? (auto-persist facts/lessons via claude CLI or API)")
        .default(false)
        .interact()?;

    // If background review is on, gate telemetry must be on too
    // (the mutation counter reads gate.jsonl).
    let gate_telemetry_enabled = if background_review && !gate_telemetry_enabled {
        println!("  (gate telemetry auto-enabled — required by background review)");
        true
    } else {
        gate_telemetry_enabled
    };

    let (
        background_review_interval,
        background_review_min_secs,
        background_review_backend,
        background_review_model,
    ) = if background_review {
        let interval: u32 = Input::with_theme(&theme)
            .with_prompt("Mutations between reviews")
            .default(50u32)
            .interact_text()?;
        let min_secs: u64 = Input::with_theme(&theme)
            .with_prompt("Minimum seconds between reviews (cooldown)")
            .default(600u64)
            .interact_text()?;
        let backend: String = Input::with_theme(&theme)
            .with_prompt("Review backend (auto / cli / api)")
            .default("auto".to_string())
            .interact_text()?;
        let model: String = Input::with_theme(&theme)
            .with_prompt("Review model (e.g. claude-haiku-4-5)")
            .default("claude-haiku-4-5".to_string())
            .interact_text()?;
        (interval, min_secs, backend, model)
    } else {
        (50, 600, "auto".to_string(), "claude-haiku-4-5".to_string())
    };

    // Small courtesy — if skills from a previous install are still there,
    // warn that the setup will leave them in place.
    if !state.skills_installed.is_empty() {
        println!(
            "\n(Existing skills at .claude/skills/ will be left in place: {})",
            state.skills_installed.join(", ")
        );
    }

    Ok(SetupAnswers {
        mode,
        memory_mode,
        reflect_cadence,
        gate_window_short_secs,
        gate_window_long_secs,
        gate_relevance_threshold,
        gate_telemetry_enabled,
        nudge_before_write,
        global_fallback,
        ignore,
        watch_enabled,
        watch_debounce_ms: 300,
        background_review,
        background_review_interval,
        background_review_min_secs,
        background_review_backend,
        background_review_model,
        compact_backup_keep: 2,
        reflect_debt_nudge: 10,
        reflect_debt_block: 20,
        gate_require_nudge: false,
        grounding_check: false,
        quarantine_failure_ratio: 0.66,
        quarantine_min_attempts: 5,
    })
}

fn parse_ignore_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

// ------------------------------------------------------------- filesystem

async fn write_config(root: &Path, a: &SetupAnswers) -> Result<()> {
    let path = root.join("config.toml");
    let body = render_toml(a);
    tokio::fs::write(&path, body)
        .await
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn render_toml(a: &SetupAnswers) -> String {
    // `ignore` is rendered inline; the rest is a flat template.
    let mut ignore_block = String::new();
    if a.ignore.is_empty() {
        ignore_block.push_str("# ignore = []\n");
    } else {
        ignore_block.push_str("ignore = [\n");
        for pat in &a.ignore {
            ignore_block.push_str(&format!("    \"{}\",\n", escape_toml_str(pat)));
        }
        ignore_block.push_str("]\n");
    }

    format!(
        "# Generated by `thoth setup`. Edit freely.\n\
         # Docs: https://github.com/unknown-studio-dev/thoth#configuration\n\
         \n\
         [index]\n\
         # Gitignore-syntax patterns layered on top of `.gitignore` / `.ignore`.\n\
         # Prefix with `!` to re-include a previously ignored path.\n\
         {ignore_block}\n\
         [memory]\n\
         # TTL (in days) for episodic log entries. 0 = keep forever.\n\
         episodic_ttl_days = 30\n\
         # Let the agent run `thoth.nudge` proactively (Mode::Full).\n\
         enable_nudge      = true\n\
         # Byte caps for the three durable memory files. Writes that would\n\
         # push a file past its cap are rejected with a structured error;\n\
         # the agent is expected to curate (forget/compact) before retrying.\n\
         cap_memory_bytes       = 16384\n\
         cap_user_bytes         = 4096\n\
         cap_lessons_bytes      = 16384\n\
         # Strict content policy — when true, REQ-12 violations (e.g.\n\
         # session-handoff prose, path-only entries) are hard-rejected.\n\
         # Default false: policy is warn-only.\n\
         strict_content_policy  = false\n\
         \n\
         [discipline]\n\
         # Master switch — flip to `false` to disable the gate entirely.\n\
         nudge_before_write         = {nudge_before_write}\n\
         # Fall back to ~/.thoth when this project has no .thoth/.\n\
         global_fallback            = {global_fallback}\n\
         # `end` (only on Stop) or `every` (after each tool call).\n\
         reflect_cadence            = \"{reflect_cadence}\"\n\
         # `auto` commits straight to MEMORY.md/LESSONS.md.\n\
         # `review` stages to *.pending.md — user must promote/reject.\n\
         memory_mode                = \"{memory_mode}\"\n\
         \n\
         # --- gate v2 -------------------------------------------------------\n\
         # Verdict on a miss:\n\
         #   `off`    — disable the gate entirely (pass every call silently).\n\
         #   `nudge`  — pass on miss but print a stderr warning. [default]\n\
         #   `strict` — block on miss.\n\
         mode                       = \"{mode}\"\n\
         \n\
         # Recency shortcut — a recall within this window passes without\n\
         # running the relevance check. Keep short; long windows enable\n\
         # ritual recall (\"call recall once, edit forever\").\n\
         gate_window_short_secs     = {gate_window_short_secs}\n\
         \n\
         # Relevance pool — the gate looks at recalls within this window\n\
         # when scoring topic overlap against the upcoming edit.\n\
         gate_window_long_secs      = {gate_window_long_secs}\n\
         \n\
         # Relevance threshold in `[0.0, 1.0]` — containment ratio\n\
         # (|edit tokens ∩ recall tokens| / min(|edit|, |recall|)).\n\
         #   0.0   — disable relevance (time-only, legacy behavior).\n\
         #   0.15  — permissive; catches only clear mismatch.\n\
         #   0.30  — balanced (recommended default).\n\
         #   0.50  — strict; forces strong token overlap.\n\
         #   0.70+ — very strict; expect noticeable friction.\n\
         gate_relevance_threshold   = {gate_relevance_threshold}\n\
         \n\
         # Append every decision to `.thoth/gate.jsonl` for calibration.\n\
         gate_telemetry_enabled     = {gate_telemetry_enabled}\n\
         \n\
         # Require a fresh `thoth_recall` immediately before each edit,\n\
         # in addition to (not instead of) the relevance check above.\n\
         # Rarely wanted — the recall already covers this.\n\
         gate_require_nudge         = {gate_require_nudge}\n\
         \n\
         # Bash commands matching these prefixes bypass the gate.\n\
         # This list is additive — built-in defaults (cargo test, git status,\n\
         # grep, rg, ls, …) are always included.\n\
         # gate_bash_readonly_prefixes = [\"pnpm lint\", \"just check\"]\n\
         \n\
         # --- reflection debt --------------------------------------------------\n\
         # Debt = session mutations (Write/Edit/NotebookEdit) minus\n\
         # remembers (thoth_remember_fact / thoth_remember_lesson).\n\
         # Above `reflect_debt_nudge` the statusline + UserPromptSubmit\n\
         # hook surface a soft reminder. Above `reflect_debt_block` the\n\
         # PreToolUse gate hard-blocks mutations until you persist\n\
         # something or use an escape hatch.\n\
         #\n\
         # Escape hatches (in order of preference):\n\
         #   1. Persist a real fact/lesson (decrements debt).\n\
         #   2. MCP tool `thoth_defer_reflect` — 30-min bypass marker.\n\
         #   3. Edits to `.thoth/config.toml` or `.thoth/*.bak-*` — always pass.\n\
         #   4. `THOTH_DEFER_REFLECT=1` env (requires restart).\n\
         reflect_debt_nudge         = {reflect_debt_nudge}\n\
         reflect_debt_block         = {reflect_debt_block}\n\
         \n\
         # Opt-in: thoth.grounding_check on every factual claim.\n\
         grounding_check            = {grounding_check}\n\
         \n\
         # Auto-quarantine lessons whose failure ratio crosses this (0.0-1.0).\n\
         quarantine_failure_ratio   = {quarantine_failure_ratio}\n\
         # Minimum attempts before quarantine is eligible.\n\
         quarantine_min_attempts    = {quarantine_min_attempts}\n\
         \n\
         # Actor-specific policy overrides. `THOTH_ACTOR` env var selects\n\
         # the policy; first matching glob wins. Uncomment to enable.\n\
         # [[discipline.policies]]\n\
         # actor = \"hoangsa/*\"            # Hoangsa wave workers\n\
         # mode = \"nudge\"\n\
         # window_short_secs = 300\n\
         # relevance_threshold = 0.20\n\
         #\n\
         # [[discipline.policies]]\n\
         # actor = \"ci-*\"                 # automation — trust it\n\
         # mode = \"off\"\n\
         \n\
         # --- background review ------------------------------------------------\n\
         # Hermes-style auto-persist: spawn `thoth review` every N mutations\n\
         # to analyze the session and save durable facts/lessons/skills.\n\
         # Requires gate_telemetry_enabled = true (counter reads gate.jsonl).\n\
         background_review              = {background_review}\n\
         background_review_interval     = {background_review_interval}\n\
         # Hard floor on review spawn rate (seconds). Mutation bursts can't\n\
         # fire back-to-back reviews within this window. 0 disables.\n\
         background_review_min_secs     = {background_review_min_secs}\n\
         # Backend: \"auto\" (API key → api, else cli), \"cli\", or \"api\".\n\
         background_review_backend      = \"{background_review_backend}\"\n\
         # Model passed to the backend. Haiku is plenty for curation;\n\
         # leaving this empty lets the CLI inherit the user's interactive\n\
         # default (often Opus) which is expensive.\n\
         background_review_model        = \"{background_review_model}\"\n\
         \n\
         # --- compact (memory consolidation) -----------------------------------\n\
         # `thoth compact` backs MEMORY.md + LESSONS.md to sibling .bak-<unix>\n\
         # files before each rewrite. This knob caps how many pairs are kept —\n\
         # older backups are deleted after each successful compact. 0 disables\n\
         # pruning (keep every backup forever).\n\
         compact_backup_keep            = {compact_backup_keep}\n\
         \n\
         [enforcement]\n\
         # Auto-promote / auto-demote engine for lesson tiers.\n\
         # Lessons climb Advise → Require → Block after repeated failures,\n\
         # and descend after repeated successes.\n\
         # promote_threshold = 2    # failures before auto-promote one tier\n\
         # demote_threshold  = 2    # successes before auto-demote one tier\n\
         # auto_promote      = true # master switch for promotion\n\
         # auto_demote       = true # master switch for demotion\n\
         # recall_within_turns        = 3  # turns the gate looks back for RequireRecall\n\
         # workflow_violation_threshold = 3 # violations in 7 days before hard block\n\
         \n\
         [chroma]\n\
         # ChromaDB semantic search via Python sidecar.\n\
         enabled = true\n\
         # data_path omitted → per-project default: <project_root>/chroma/\n\
         # Uncomment to override:\n\
         # data_path = \"/custom/path/to/chromadb\"\n\
         \n\
         [watch]\n\
         # Auto-watch the source tree for changes and reindex in the MCP\n\
         # daemon background. Removes the need for `thoth watch`.\n\
         enabled      = {watch_enabled}\n\
         # Debounce window (ms). Events within this window after the first\n\
         # change are batched into a single reindex pass.\n\
         debounce_ms  = {watch_debounce_ms}\n",
        ignore_block = ignore_block,
        mode = a.mode,
        memory_mode = a.memory_mode,
        reflect_cadence = a.reflect_cadence,
        gate_window_short_secs = a.gate_window_short_secs,
        gate_window_long_secs = a.gate_window_long_secs,
        gate_relevance_threshold = a.gate_relevance_threshold,
        gate_telemetry_enabled = a.gate_telemetry_enabled,
        nudge_before_write = a.nudge_before_write,
        global_fallback = a.global_fallback,
        watch_enabled = a.watch_enabled,
        watch_debounce_ms = a.watch_debounce_ms,
        background_review = a.background_review,
        background_review_interval = a.background_review_interval,
        background_review_min_secs = a.background_review_min_secs,
        background_review_backend = a.background_review_backend,
        background_review_model = a.background_review_model,
        compact_backup_keep = a.compact_backup_keep,
        reflect_debt_nudge = a.reflect_debt_nudge,
        reflect_debt_block = a.reflect_debt_block,
        gate_require_nudge = a.gate_require_nudge,
        grounding_check = a.grounding_check,
        quarantine_failure_ratio = a.quarantine_failure_ratio,
        quarantine_min_attempts = a.quarantine_min_attempts,
    )
}

fn escape_toml_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Create `MEMORY.md` and `LESSONS.md` if they don't exist. Never
/// overwrites — an existing file is left alone, matching `thoth init`.
///
/// On first creation `MEMORY.md` is seeded with a single init-date fact.
/// That entry survives `/clear` and `/compact` via the SessionStart
/// banner (which dumps MEMORY.md into context), so a freshly-installed
/// project has at least one in-context pointer telling the agent when
/// Thoth was wired in and how the discipline loop works. Re-running
/// `thoth setup` on an existing project does NOT overwrite — the user
/// may have pruned or edited the seed fact, and their state wins.
async fn seed_markdown(root: &Path) -> Result<()> {
    let memory = root.join("MEMORY.md");
    if !memory.exists() {
        tokio::fs::write(&memory, render_memory_seed(&today_ymd()))
            .await
            .with_context(|| format!("write {}", memory.display()))?;
    }
    let lessons = root.join("LESSONS.md");
    if !lessons.exists() {
        tokio::fs::write(&lessons, "# LESSONS.md\n")
            .await
            .with_context(|| format!("write {}", lessons.display()))?;
    }
    let user = root.join("USER.md");
    if !user.exists() {
        tokio::fs::write(&user, USER_MD_TEMPLATE)
            .await
            .with_context(|| format!("write {}", user.display()))?;
    }
    Ok(())
}

/// Today in `YYYY-MM-DD`, UTC. Thoth's other memory entries use bare
/// dates (not timestamps); UTC sidesteps the timezone-shift edge cases
/// that would otherwise put the init date a day off for users on the
/// other side of midnight from where the binary runs.
pub(crate) fn today_ymd() -> String {
    use time::OffsetDateTime;
    OffsetDateTime::now_utc()
        .format(&time::macros::format_description!("[year]-[month]-[day]"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Body written to `MEMORY.md` on first creation. Matches the on-disk
/// format produced by `thoth_remember_fact` (`### heading`, blank line,
/// body paragraph, trailing `tags:` line) so the SessionStart banner
/// renders this entry the same as any other fact.
fn render_memory_seed(date: &str) -> String {
    format!(
        "# MEMORY.md\n\
         ### Thoth initialized on {date}.\n\
         \n\
         This project uses Thoth MCP as its long-term memory. Facts live in this file; \
         lessons live in `LESSONS.md`. Call `mcp__thoth__thoth_recall` before Write / \
         Edit / Bash; persist new facts via `mcp__thoth__thoth_remember_fact` and new \
         lessons via `mcp__thoth__thoth_remember_lesson`. See also `./CLAUDE.md` for \
         the full policy block loaded into every session.\n\
         tags: thoth, init, bootstrap\n"
    )
}

/// Seed a `.thothignore` in the project root (parent of `.thoth/`) if one
/// doesn't exist. Detects the project's language stack from common marker
/// files and emits language-appropriate ignore patterns. Never overwrites
/// — the user's edits win.
async fn seed_thothignore(root: &Path) -> Result<()> {
    // `.thothignore` lives in the project root, not inside `.thoth/`.
    let project_root = root.parent().unwrap_or(Path::new("."));
    let path = project_root.join(".thothignore");
    if path.exists() {
        return Ok(());
    }

    let body = render_thothignore(project_root);
    tokio::fs::write(&path, body)
        .await
        .with_context(|| format!("write {}", path.display()))?;
    println!("✓ Seeded {}", path.display());
    Ok(())
}

/// Detect languages from marker files and render a `.thothignore` body.
fn render_thothignore(project_root: &Path) -> String {
    let mut lines = vec![
        "# .thothignore — Thoth-specific ignore rules (gitignore syntax).",
        "# Layered on top of .gitignore. Edit freely.",
        "",
        "# Thoth data (always ignored by the watcher, but explicit here too)",
        ".thoth/",
        "",
    ];

    // Detect languages and add relevant patterns.
    let has = |name: &str| project_root.join(name).exists();

    // Rust
    if has("Cargo.toml") {
        lines.push("# Rust");
        lines.push("target/");
        lines.push("Cargo.lock");
        lines.push("");
    }

    // Node / JS / TS
    if has("package.json") {
        lines.push("# Node / JS / TS");
        lines.push("node_modules/");
        lines.push("dist/");
        lines.push("build/");
        lines.push(".next/");
        lines.push(".nuxt/");
        lines.push("coverage/");
        lines.push("*.min.js");
        lines.push("*.bundle.js");
        lines.push("package-lock.json");
        lines.push("yarn.lock");
        lines.push("pnpm-lock.yaml");
        lines.push("");
    }

    // Python
    if has("pyproject.toml") || has("setup.py") || has("requirements.txt") || has("Pipfile") {
        lines.push("# Python");
        lines.push("__pycache__/");
        lines.push("*.pyc");
        lines.push(".venv/");
        lines.push("venv/");
        lines.push(".eggs/");
        lines.push("*.egg-info/");
        lines.push(".mypy_cache/");
        lines.push(".pytest_cache/");
        lines.push("");
    }

    // Common generated / large files
    lines.push("# Common generated / large files");
    lines.push("*.generated.*");
    lines.push("*.min.css");
    lines.push("*.map");
    lines.push("*.pb.rs");
    lines.push("");

    lines.join("\n")
}

// -------------------------------------------------- integration wiring

/// Full install — hooks + skills + MCP — into `./.claude/settings.json`.
/// When the root lives under `~/.thoth/projects/`, also register it in
/// the global project registry.
async fn install_integration(root: &Path) -> Result<()> {
    hooks::install_all(hooks::Scope::Project, root).await?;
    if crate::resolve::is_global_root(root)
        && let Ok(cwd) = std::env::current_dir()
    {
        let slug = crate::resolve::project_slug(&cwd);
        let _ = crate::resolve::register_project(&slug, &cwd);
    }
    Ok(())
}

/// Re-run install without touching `config.toml`. Used by the
/// already-bootstrapped branch.
///
/// Also backfills memory-state additions introduced after the user's
/// original setup: creates missing `USER.md` from the embedded template
/// and appends any `[memory]` cap keys that are absent from their
/// existing `config.toml`. Existing values are left alone.
async fn reinstall_integration(root: &Path) -> Result<()> {
    seed_markdown(root).await?;
    backfill_memory_caps(root).await?;
    install_integration(root).await
}

/// Append `cap_memory_bytes` / `cap_user_bytes` / `cap_lessons_bytes` /
/// `strict_content_policy` to an existing `config.toml` if those keys
/// are missing. Preserves user edits to values already present.
async fn backfill_memory_caps(root: &Path) -> Result<()> {
    let cfg_path = root.join("config.toml");
    let Ok(body) = tokio::fs::read_to_string(&cfg_path).await else {
        return Ok(());
    };
    let mut pending: Vec<&str> = Vec::new();
    if !body.contains("cap_memory_bytes") {
        pending.push("cap_memory_bytes       = 16384");
    }
    if !body.contains("cap_user_bytes") {
        pending.push("cap_user_bytes         = 4096");
    }
    if !body.contains("cap_lessons_bytes") {
        pending.push("cap_lessons_bytes      = 16384");
    }
    if !body.contains("strict_content_policy") {
        pending.push("strict_content_policy  = false");
    }
    if pending.is_empty() {
        return Ok(());
    }

    // Inject after the `[memory]` header so keys stay in their section.
    // Fall back to appending at EOF if the section isn't present — the
    // missing-section case means the file was hand-written without a
    // [memory] block; the MemoryConfig parser accepts top-level keys too.
    let patch = pending.join("\n");
    let new_body = match body.find("[memory]") {
        Some(hdr_start) => {
            let after_hdr = body[hdr_start..]
                .find('\n')
                .map(|rel| hdr_start + rel + 1)
                .unwrap_or(body.len());
            let mut out = String::with_capacity(body.len() + patch.len() + 64);
            out.push_str(&body[..after_hdr]);
            out.push_str(
                "# Backfilled by `thoth setup` self-heal — byte caps for durable memory files.\n",
            );
            out.push_str(&patch);
            out.push('\n');
            out.push_str(&body[after_hdr..]);
            out
        }
        None => {
            let mut out = body;
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("\n[memory]\n");
            out.push_str(&patch);
            out.push('\n');
            out
        }
    };
    tokio::fs::write(&cfg_path, new_body)
        .await
        .with_context(|| format!("backfill caps into {}", cfg_path.display()))?;
    println!("✓ Backfilled [memory] caps into {}", cfg_path.display());
    Ok(())
}

// ------------------------------------------------------ output helpers

fn print_summary(root: &Path, a: &SetupAnswers) {
    let cfg = root.join("config.toml");
    println!("\n✓ Wrote {}", cfg.display());
    println!("  mode                     = {}", a.mode);
    println!("  memory_mode              = {}", a.memory_mode);
    println!("  reflect_cadence          = {}", a.reflect_cadence);
    println!("  gate_window_short_secs   = {}", a.gate_window_short_secs);
    println!("  gate_window_long_secs    = {}", a.gate_window_long_secs);
    println!(
        "  gate_relevance_threshold = {:.2}",
        a.gate_relevance_threshold
    );
    println!("  gate_telemetry_enabled   = {}", a.gate_telemetry_enabled);
    println!("  watch.enabled            = {}", a.watch_enabled);
    println!("  background_review        = {}", a.background_review);
    if a.background_review {
        println!(
            "  background_review_interval = {}",
            a.background_review_interval
        );
        println!(
            "  background_review_min_secs = {}",
            a.background_review_min_secs
        );
        println!(
            "  background_review_backend  = {}",
            a.background_review_backend
        );
        println!(
            "  background_review_model    = {}",
            a.background_review_model
        );
    }
    println!("  ignore patterns          = {}", a.ignore.len());
}

fn print_final_message(root: &Path, reconfigured: bool) {
    let cfg = root.join("config.toml");
    println!();
    if reconfigured {
        println!("✓ Thoth integration refreshed.");
    } else {
        println!("✓ Thoth is wired into Claude Code for this project.");
    }
    println!();
    println!("Next:");
    println!("  1. Open {} and adjust if needed.", cfg.display());
    println!("  2. thoth index .              # build the code index");
    println!();
    println!("Re-run `thoth setup` any time to reconfigure or repair the install.");
}

// --------------------------------------------------------- thoth init command

/// Scaffold written on `thoth init`. Every setting is commented out so the
/// file only documents defaults — uncomment to override. Kept inline (not
/// a separate `include_str!` fixture) so the CLI binary has no external
/// runtime dependency on the repo layout.
pub const DEFAULT_CONFIG_TOML: &str = r#"# Thoth config. All fields are optional; defaults shown.
# Uncomment the ones you want to change.

[index]
# Gitignore-syntax patterns. Applied on top of `.gitignore`, `.ignore`, and
# any `.thothignore` found in the project. Supports re-including with `!`.
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

# Whether to run the LLM nudge at session end (Mode::Full only).
# enable_nudge = true

[discipline]
# Gate master switch — set `false` to disable discipline entirely.
# nudge_before_write = true

# Gate verdict on a miss:
#   "off"    — disable the gate (pass silently).
#   "nudge"  — pass + stderr warning. [default]
#   "strict" — block.
# mode = "nudge"

# Recency shortcut — recall within this window passes without a relevance
# check. Keep short; long windows enable ritual recall.
# gate_window_short_secs = 60

# Relevance pool — how far back the gate looks for a topical recall.
# gate_window_long_secs = 1800

# Relevance threshold [0.0, 1.0] — containment score of edit tokens vs
# the best-matching recent recall. Guidance:
#   0.0  — disable relevance (time-only legacy behavior).
#   0.15 — permissive; catches only clear mismatch.
#   0.30 — balanced. [default]
#   0.50 — strict; forces strong token overlap.
#   0.70+ — very strict; expect friction.
# gate_relevance_threshold = 0.30

# Append decisions to `.thoth/gate.jsonl` for calibration.
# gate_telemetry_enabled = false

# Reflection-debt thresholds. Debt = mutations (successful Write/Edit/
# NotebookEdit passes, read from gate.jsonl) minus remembers (append
# ops in memory-history.jsonl), windowed to the current session.
#
# Above the nudge threshold, the Stop + UserPromptSubmit hooks inject
# a reminder into agent context. Above the block threshold, the gate
# hard-blocks new mutations until the agent calls thoth_remember_fact
# or thoth_remember_lesson. Set `THOTH_DEFER_REFLECT=1` in the env to
# bypass for one session.
#
# Set either to `0` to disable that tier.
# reflect_debt_nudge = 10
# reflect_debt_block = 20

# Additional Bash prefixes that bypass the gate (additive with built-ins
# like `cargo test`, `git status`, `grep`).
# gate_bash_readonly_prefixes = ["pnpm lint", "just check"]

# Actor-specific policy overrides. `THOTH_ACTOR` env var selects the
# policy; first matching glob wins. Omit `[[discipline.policies]]`
# entirely to apply the default policy to every actor.
#
# [[discipline.policies]]
# actor = "hoangsa/*"
# mode = "nudge"
# window_short_secs = 300
# relevance_threshold = 0.20
#
# [[discipline.policies]]
# actor = "ci-*"
# mode = "off"

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

# Node count above which `thoth_impact` groups results by file
# rather than listing every node. Default: 50. Set to 0 to disable
# grouping (always flat list).
# impact_group_threshold = 50
"#;

/// `thoth init` — create `.thoth/` + seed markdown and a documented config.
pub async fn cmd_init(root: &std::path::Path) -> anyhow::Result<()> {
    use thoth_store::StoreRoot;
    let existed = root.exists();
    let store = StoreRoot::open(root).await?;
    // Seed empty source-of-truth files so `memory show` isn't confused.
    let mut seeded = Vec::new();
    for name in ["MEMORY.md", "LESSONS.md"] {
        let p = store.path.join(name);
        if !p.exists() {
            tokio::fs::write(&p, format!("# {name}\n")).await?;
            seeded.push(name);
        }
    }
    // Scaffold a documented config.toml on first run. Every knob is
    // commented-out so the file documents defaults without overriding
    // them — users uncomment only what they want to change. Never
    // overwrite an existing config.
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
    println!("  next:   thoth index .");
    Ok(())
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn setup_creates_user_md_seed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        seed_markdown(root).await.expect("seed_markdown");

        let user = root.join("USER.md");
        assert!(user.exists(), "USER.md should be created");
        let body = tokio::fs::read_to_string(&user)
            .await
            .expect("read USER.md");
        assert!(body.contains("# USER.md"), "USER.md header missing");
        assert!(
            body.contains("preferences") || body.contains("first-person"),
            "seed should mention preferences"
        );

        // Idempotent: existing USER.md is not overwritten.
        tokio::fs::write(&user, "# custom\n").await.unwrap();
        seed_markdown(root).await.expect("second seed");
        let after = tokio::fs::read_to_string(&user).await.unwrap();
        assert_eq!(after, "# custom\n", "existing USER.md must be preserved");
    }

    #[test]
    fn render_toml_includes_memory_caps_and_policy() {
        let toml = render_toml(&SetupAnswers::default());
        assert!(toml.contains("cap_memory_bytes       = 16384"));
        assert!(toml.contains("cap_user_bytes         = 4096"));
        assert!(toml.contains("cap_lessons_bytes      = 16384"));
        assert!(toml.contains("strict_content_policy  = false"));
    }
}
