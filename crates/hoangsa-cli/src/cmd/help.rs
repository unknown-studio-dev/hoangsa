//! CLI help text. Single source of truth for `--help` and unknown-command
//! output. Keeping all topics here (instead of spreading a `help()` fn across
//! each cmd module) makes drift between the dispatch table in `main.rs` and
//! the docs easy to spot in one file.

use std::io::Write;

/// Every top-level command name exposed by `main.rs`. Used by the main help
/// output and the smoke test that guards against dispatch/docs drift.
pub const TOPICS: &[&str] = &[
    "addon",
    "bootstrap",
    "budget",
    "commit",
    "config",
    "context",
    "ctx",
    "dag",
    "enforce",
    "hook",
    "install",
    "media",
    "memory-guidance",
    "plan",
    "pref",
    "resolve-model",
    "rule",
    "session",
    "state",
    "stats",
    "trust",
    "validate",
    "verify",
    "workflow",
];

/// Entry point.
///
/// - `topic = None` → full usage banner.
/// - `topic = Some(t)` → detailed help for that command. Unknown topic falls
///   back to the main banner + note.
///
/// `to_stderr = true` for the "Unknown command" path so the banner doesn't
/// pollute stdout when callers pipe normal output. Explicit `--help` goes to
/// stdout.
pub fn print_help(topic: Option<&str>, to_stderr: bool) {
    let text = match topic {
        None => main_help(),
        Some(t) => match topic_help(t) {
            Some(s) => s,
            None => format!(
                "Unknown help topic: {t}\n\nRun `hoangsa-cli --help` for the full command list.\n"
            ),
        },
    };
    if to_stderr {
        let _ = writeln!(std::io::stderr(), "{}", text.trim_end());
    } else {
        let _ = writeln!(std::io::stdout(), "{}", text.trim_end());
    }
}

fn main_help() -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!(
        r#"hoangsa-cli {version} — orchestration + enforcement backend for the HOANGSA workflow.

Usage:
  hoangsa-cli <command> [subcommand] [args...]
  hoangsa-cli <command> --help      Detailed help for one command
  hoangsa-cli --help | -h           Show this banner
  hoangsa-cli --version | -V        Print version

Global flags:
  --cwd <dir>        Override the working directory used by the command
  --raw              Emit raw JSON (skip pretty-printing in hooks/reports)

Workflow & session:
  session init|latest|list|usage          Session lifecycle
  state init|get|update                   Session state.json
  ctx <workflow> [session_id]             Workflow-aware context pack
  context pack|get <sessionDir> <taskId>  Per-task context assembly
  budget estimate|breakdown               Token budget math

Config & preferences:
  pref get|set [projectDir] ...           ~/.hoangsa/preferences + overrides
  config get|set <projectDir> [patch]     .hoangsa/config.json
  addon list|add|remove <projectDir>      Worker-rule addons
  memory-guidance sync [projectDir]       Rewrite .hoangsa/memory-guidance.md

Planning:
  plan task-ids|resolve <plan_path>       Extract/resolve tasks from plan.json
  validate plan|spec|tests <path>         JSON schema validation
  validate plan <path> --tests <spec>     + cross-check embeddings vs TEST-SPEC
  dag check|waves <plan_path>             Task DAG sanity + wave layout

Workers:
  rules compose <dir> [--task-type][--role]  Middleware-chain worker rules → JSON
  envelope <sessionDir> <taskId> [--kind cook|fix]  Full worker prompt → stdout

Rules & enforcement:
  rule init|list|add|remove|enable|disable|sync [projectDir] [args...]
  hook enforce|post-enforce|rule-gate     PreToolUse / PostToolUse entry points
  hook stop-check|lesson-guard            Stop-hook + lesson guardrails
  hook session-start|session-archive|session-usage|statusline
  hook state-record|state-check|state-clear
  hook codex <handler>                    Same handlers, Codex hook wire
  codex render <command> [--arguments s]  Render a workflow for Codex skills
  enforce override|report                 Inspect / bypass enforcement events

Trust & verification:
  trust check|approve|revoke|list         Sandbox-trust fingerprints
  verify [projectDir]                     Self-check install integrity

Model routing:
  resolve-model <role> | --all            Resolve role → model id from config

Stats & media:
  stats record|summary|cache              Usage telemetry
  media probe|frames|montage|diff|check-ffmpeg|install-ffmpeg   (feature: media)

Install & git:
  install [flags]                         Install CLI + hooks (see `install --help`)
  update [--check] [--local]              Check for a newer release; install it
  uninstall --global|--local [--purge]    Remove everything install wrote
  bootstrap [flags]                       Bootstrap project .hoangsa/ skeleton
  commit "<msg>" --files <f1> <f2> ...    Guarded git commit wrapper

Run `hoangsa-cli <command> --help` for the full flag list on any topic.
"#
    )
}

fn topic_help(topic: &str) -> Option<String> {
    let body = match topic {
        "addon" => {
            "addon — worker-rule addons under .hoangsa/worker-rules/addons/

Usage:
  hoangsa-cli addon list   <projectDir>
  hoangsa-cli addon add    <projectDir> '<json_array>'
  hoangsa-cli addon remove <projectDir> '<json_array>'

<json_array> is a JSON list of addon slugs, e.g. '[\"react\",\"fastapi\"]'."
        }
        "bootstrap" => {
            "bootstrap — seed `.hoangsa/` skeleton in a project.

Usage:
  hoangsa-cli bootstrap [--project <path>] [--force] [--json]

Flags:
  --project <path>   Target project directory (default: cwd)
  --force            Overwrite existing .hoangsa/ entries
  --json             Emit machine-readable report on stdout"
        }
        "budget" => {
            "budget — token-budget math over plan.json.

Usage:
  hoangsa-cli budget estimate  <plan_path> <task_id>
  hoangsa-cli budget breakdown <plan_path>"
        }
        "commit" => {
            "commit — guarded git commit wrapper.

Usage:
  hoangsa-cli commit \"<message>\" --files <f1> [<f2> ...]

Stages the listed files then commits with the given message. Files are
passed verbatim to `git add`; on any add/commit failure the JSON error
surfaces on stdout."
        }
        "config" => {
            "config — read / patch .hoangsa/config.json.

Usage:
  hoangsa-cli config get <projectDir>
  hoangsa-cli config set <projectDir> <jsonPatch>

<jsonPatch> is an RFC-6902 patch or a partial object to merge.

Top-level keys: profile, harness (claude|codex|cowork — set by
`install --harness`, overridden by HOANGSA_HARNESS), model_overrides,
preferences, codebase, task_manager."
        }
        "context" => {
            "context — per-task context assembly.

Usage:
  hoangsa-cli context pack <sessionDir> <taskId>
  hoangsa-cli context get  <sessionDir> <taskId>

`pack` writes the assembled context to the session dir; `get` prints it."
        }
        "ctx" => {
            "ctx — workflow-aware context pack (project + session snapshot).

Usage:
  hoangsa-cli ctx <workflow> [session_id]

<workflow> is one of: menu, prepare, cook, taste, fix, ship, check, audit,
research, brainstorm, plate, serve, rule, addon, init, index, update.
Additional sections are included when the workflow implies them (e.g.
cook/taste/check add the live plan)."
        }
        "dag" => {
            "dag — task DAG sanity + wave layout.

Usage:
  hoangsa-cli dag check <plan_path>
  hoangsa-cli dag waves <plan_path>

`check` reports cycles and missing deps; `waves` prints the topological
wave assignment for parallel execution."
        }
        "enforce" => {
            "enforce — inspect / bypass enforcement events.

Usage:
  hoangsa-cli enforce override <ruleId> <target> [reason]
  hoangsa-cli enforce report

`override` appends an `override` event to the enforcement log, unlocking
one stateful rule for <target>. `report` prints a summary of recent
enforcement activity (blocks, warnings, overrides)."
        }
        "hook" => {
            "hook — Claude Code hook entry points.

Usage:
  hoangsa-cli hook enforce          PreToolUse — pattern + stateful rules
  hoangsa-cli hook post-enforce     PostToolUse — record outcomes
  hoangsa-cli hook rule-gate        Legacy alias for pattern-only gating
  hoangsa-cli hook stop-check       Stop — block if workflow not closed
  hoangsa-cli hook prompt-guard     UserPromptSubmit — frustration sensor
  hoangsa-cli hook lesson-guard     PreToolUse — inject lessons before edits
  hoangsa-cli hook graph-affordance PreToolUse — nudge toward graph tools on repeated search
  hoangsa-cli hook session-start    SessionStart — auto-inject USER/MEMORY/LESSONS
  hoangsa-cli hook session-archive  PreCompact — archive curated turns
  hoangsa-cli hook session-usage    Session usage snapshot
  hoangsa-cli hook state-record     Record an intent (memory_impact, detect_changes)
  hoangsa-cli hook state-check      Re-read enforcement state
  hoangsa-cli hook state-clear      Wipe enforcement state for the session
  hoangsa-cli hook statusline       Print statusline JSON for Claude Code
  hoangsa-cli hook codex <handler>  Run any handler above under Codex CLI —
                                    output translated to Codex hook wire

All hooks read a JSON payload on stdin (per Claude Code hook contract;
Codex mirrors it) and emit a decision JSON on stdout."
        }
        "update" => {
            "update — check for a newer HOANGSA release and install it.

Usage:
  hoangsa-cli update [--check] [--local] [--dry-run] [--yes]

Flags:
  --check      Report current vs latest and exit; never installs
  --local      Target the project install (./.claude) instead of the global one
  --dry-run    Print the installer command that would run; run nothing
  --yes, -y    Skip the confirmation prompt

The installed version is read from `<install dir>/manifest.json` — the file the
installer actually writes. Upgrading shells out to the release installer, which
is what knows how to fetch the binary for this platform, then clears the
update-check cache so the statusline badge clears with it.

Exit codes:
  0  up to date, or the update finished
  1  the check or the install failed
  2  bad arguments
 10  --check only: an update is available"
        }
        "uninstall" => {
            "uninstall — remove everything `install` wrote.

Usage:
  hoangsa-cli uninstall --global [--dry-run] [--purge] [--yes]
  hoangsa-cli uninstall --local  [--dry-run]

Flags:
  --global     Remove the global install (Claude config dir + ~/.hoangsa bins)
  --local      Remove the project install (./.claude/ + ./.mcp.json entry)
  --dry-run    Print what would be removed; touch nothing
  --purge      (global only) Also delete ~/.hoangsa entirely, including the
               memory store. Per-project .hoangsa/ dirs are never touched.
  --yes, -y    Skip the confirmation prompt --purge asks on a terminal

Removes binaries, every template tracked in the manifest, hook entries marked
`_hoangsa_managed`, the hoangsa-memory MCP registration, the managed PATH block
in your shell rc, and the model cache.

Preserved unless --purge: ~/.hoangsa/memory, ~/.hoangsa/share, and every
per-project .hoangsa/ directory.

Environment:
  HOANGSA_INSTALL_DIR   Install root (default: ~/.hoangsa)
  HOANGSA_CLI_DIR       CLI bin dir  (default: $HOANGSA_INSTALL_DIR/bin)
  CLAUDE_CONFIG_DIR     Config dir to clean (default: the first that exists)
  HOANGSA_NO_PATH_EDIT  If \"1\", leave shell rc files alone

Start with --dry-run. `scripts/uninstall.sh` does the same job from a checkout,
for when this binary is missing or will not run."
        }
        "install" => {
            "install — install hoangsa-cli + hooks into ~/.hoangsa or ./.claude.

Usage:
  hoangsa-cli install [flags]

Flags:
  --global            Install into ~/.hoangsa (default)
  --local             Install into ./.claude (project-local)
  --dry-run           Print actions without writing
  --harness=<claude|codex|cowork>  Target harness (default: claude)
  --task-manager=<clickup|asana|none>  Pre-select task manager integration
  --no-memory         Skip hoangsa-memory MCP daemon install
  --skip-path-edit    Don't modify shell rc files

An explicit --harness on a local install is also recorded as `harness` in
.hoangsa/config.json, which is where `resolve-model` and `envelope` read it.
--global has no project config to write to, so export HOANGSA_HARNESS
instead. Other config keys are left untouched, and --dry-run writes nothing."
        }
        "media" => {
            "media — ffmpeg-backed screenshot + video helpers (feature: media).

Usage:
  hoangsa-cli media probe   <path>
  hoangsa-cli media frames  <video> [--interval <secs>] [--max-frames <n>] [--output-dir <dir>]
  hoangsa-cli media montage <frames_dir> [--cols <n>] [--timestamps] [--output <path>]
  hoangsa-cli media diff    <frames_dir> [--cols <n>] [--output <path>]
  hoangsa-cli media check-ffmpeg
  hoangsa-cli media install-ffmpeg

`diff` takes the frames *directory* (it overlays consecutive frames), not a
pair of images.

Only available when built with `--features media`."
        }
        "memory-guidance" => {
            "memory-guidance — regenerate .hoangsa/memory-guidance.md.

Usage:
  hoangsa-cli memory-guidance sync [projectDir]

Rewrites the canonical memory-guidance snippet from the shipped template,
preserving any user edits inside project-override markers."
        }
        "plan" => {
            "plan — read/query plan.json.

Usage:
  hoangsa-cli plan task-ids <plan_path>
  hoangsa-cli plan resolve  <plan_path>

`task-ids` lists every task id; `resolve` returns each task with its
absolute paths and inherited config."
        }
        "pref" => {
            "pref — user + project preferences.

Usage:
  hoangsa-cli pref get [projectDir] [key]
  hoangsa-cli pref set [projectDir] <key> <value>

Without <key>, `get` prints the whole preferences map.

`workflow_profile` (full|balanced|minimal) is a preset: setting it writes
simplify_pass, quality_gate, test_runs, research_mode, context_mode and
memory_strict in one go. It is NOT the top-level `profile` key, which routes
models (quality|balanced|budget|minimal) — see `help resolve-model`. The old
spelling `pref set . profile <x>` still works but warns and now writes only
the workflow key; it used to overwrite model routing with a name that
vocabulary doesn't have."
        }
        "resolve-model" => {
            "resolve-model — role → model id resolution.

Usage:
  hoangsa-cli resolve-model <role>
  hoangsa-cli resolve-model --all

<role> is one of the eight fixed roles: researcher, designer, planner,
orchestrator, worker, reviewer, tester, committer. `--all` prints every
role → model mapping.

Resolution order: `model_overrides.<role>` in .hoangsa/config.json, then
the `profile` in that file, then the balanced profile as a fallback.
Profiles: quality | balanced (default) | budget | minimal.

Tiers: fable > opus > sonnet > haiku. `fable` (Claude Fable 5, ~2× opus)
belongs to no profile — reach it per role via `model_overrides`, e.g.
`{ \"model_overrides\": { \"designer\": \"fable\" } }`.

Harness-aware. With `\"harness\": \"codex\"` in .hoangsa/config.json (or
HOANGSA_HARNESS=codex) the tier is not a model id — Codex scales by
reasoning effort, so the tier maps to one and the session keeps its own
model:

  fable, opus → high      sonnet → medium      haiku → low

The reported `model` then comes from ~/.codex/config.toml and is
informational; HOANGSA never overrides it, and worker envelopes carry
`REASONING EFFORT:` instead of `MODEL:`."
        }
        "rule" => {
            "rule — manage .hoangsa/rules.json.

Usage:
  hoangsa-cli rule init    [projectDir]            Seed defaults (idempotent)
  hoangsa-cli rule list    [projectDir]
  hoangsa-cli rule add     [projectDir] '<json>'   Insert a rule
  hoangsa-cli rule remove  [projectDir] <id>
  hoangsa-cli rule enable  [projectDir] <id>
  hoangsa-cli rule disable [projectDir] <id>
  hoangsa-cli rule sync    [projectDir]            Write Prompt rules → CLAUDE.md

Scope: enforcement reads the global layer (~/.hoangsa/rules.json) overlaid
by the project layer (.hoangsa/rules.json); a project rule overrides a global
rule with the same id. A missing file at either layer contributes no rules —
an uninitialised project simply has nothing enforced (no implicit defaults).
These `rule` subcommands manage the PROJECT layer.

Note: `rule sync` refreshes CLAUDE.md from the current rules.json but
does NOT reconcile default rules. To pick up newly-added default rules
in an existing project, delete rules.json and re-run `rule init`."
        }
        "session" => {
            "session — session lifecycle under .hoangsa/sessions/.

Usage:
  hoangsa-cli session init    <type> <name> [sessions_dir]
  hoangsa-cli session latest  [sessions_dir]
  hoangsa-cli session list    [sessions_dir]
  hoangsa-cli session usage   [session_id] [sessions_dir]

<type> is one of the canonical workflow types (fix, menu, cook, taste,
ship, brainstorm, audit, research, plate, check). `usage` summarises
token + wall-clock usage; with <session_id> it scopes to one session."
        }
        "state" => {
            "state — session state.json read / patch.

Usage:
  hoangsa-cli state init   <sessionDir>
  hoangsa-cli state get    <sessionDir>
  hoangsa-cli state update <sessionDir> <jsonPatch>

<jsonPatch> is a partial object merged into state.json."
        }
        "stats" => {
            "stats — session telemetry.

Usage:
  hoangsa-cli stats record '<json>'
  hoangsa-cli stats summary [--last N] [--complexity low|medium|high]
  hoangsa-cli stats cache   [-n top] [-s session_id]
  hoangsa-cli stats report  --all [projectDir]

`record` appends an event; `summary` aggregates recent sessions; `cache`
prints cache-hit statistics per session."
        }
        "trust" => {
            "trust — sandbox-trust fingerprints for MCP servers & agents.

Usage:
  hoangsa-cli trust check   <projectDir>
  hoangsa-cli trust approve <fingerprint> <name>
  hoangsa-cli trust revoke  <fingerprint>
  hoangsa-cli trust list

Fingerprints are sha256 hashes printed by the target binary on first
launch; `approve` records a (fingerprint, name) pair into the trust
store so subsequent launches are non-interactive."
        }
        "validate" => {
            "validate — JSON-schema validation.

Usage:
  hoangsa-cli validate plan  <path> [--tests <TEST-SPEC path>]
  hoangsa-cli validate spec  <path>
  hoangsa-cli validate tests <path>
  hoangsa-cli validate scope <sessionDir> <taskId> [--rev <sha>]

`validate scope` checks a task's commit (default HEAD) against that
task's `files` in plan.json: any path the commit touched that the plan
did not hand the worker is an error; a declared file the commit left
alone is a warning. This is the check that turns \"only modify
task.files\" from a worker rule into a gate.

`validate spec` is category-aware: a `category: code` DESIGN-SPEC must
carry a non-empty ## Behavior / Logic (the logic a fresh-context worker
implements from), a ## Risk Sweep answering all 8 risk classes
(boundary, invalid input, concurrency & TOCTOU, idempotency & retry,
partial failure & rollback, auth & permission, limits, backward compat)
and a RESOLVED/DEFERRED status on every ## Open Questions entry — an
unstatused question is one that was never put to the user.

`validate plan` requires every \"type\": \"impl\" task to carry a
non-empty `behavior` array (waivable with an explicit \"N/A — <reason>\"
entry). With --tests it also cross-checks plan.json against the
TEST-SPEC: every Edge Cases row must be embedded in ≥1 implementation
task AND ≥1 test task, every spec test must appear in some task's
test_cases, ## E2E Tests requires an e2e task, and surface: ui requires
a task flagged \"ui\": true.

Emits the schema-path + message for each violation."
        }
        "workflow" => {
            "workflow — resolve and print a workflow file.

Usage:
  hoangsa-cli workflow show <name>

Prints the first `<name>.md` found, searching in order: $HOANGSA_ROOT,
the project-local install (./.claude/hoangsa/), $CLAUDE_CONFIG_DIR, then
~/.claude/. stdout IS the workflow text — the /hoangsa:* commands run this
and follow the output, so resolution lives here rather than being spelled
out in twenty command files."
        }
        "verify" => {
            "verify — self-check install integrity.

Usage:
  hoangsa-cli verify [projectDir]

Runs the built-in diagnostic suite: binaries on PATH, hook scripts
present, rules.json parseable, memory daemon reachable, etc. Emits a
JSON report with one { check, status, detail } per row."
        }
        _ => return None,
    };
    Some(body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_has_help() {
        for topic in TOPICS {
            assert!(
                topic_help(topic).is_some(),
                "topic {topic:?} declared in TOPICS but topic_help() returned None"
            );
        }
    }

    #[test]
    fn main_help_mentions_every_topic() {
        let body = main_help();
        for topic in TOPICS {
            assert!(
                body.contains(topic),
                "main help does not mention top-level command {topic:?}"
            );
        }
    }

    #[test]
    fn unknown_topic_is_none() {
        assert!(topic_help("no-such-command").is_none());
    }
}
