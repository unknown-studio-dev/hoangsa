# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.0] - 2026-07-27

### Security

- **`hoangsa-cli hook enforce` now fails closed.** An unreadable rules layer
  was treated as "no rules", so a corrupt or permission-denied file silently
  disabled every gate it carried.
- **`pre_invoke_gate` is gone.** Addons could ship a shell command that the
  envelope executed on every worker spawn — a repository-supplied gate was
  arbitrary code execution by design. Replaced with declarative
  `requires_pref`, which `rules compose` evaluates without a shell.
- **`update` builds its installer invocation as argv** instead of a shell
  string, and rejects release tags that are not `v<semver>`.

### Fixed

- **The seeded `config.toml` contradicted the code it configures.** The
  `[vector_store]` block documented `Default: true`; `VectorStoreConfig::enabled`
  derives `false` and is opt-in. Stores seeded by any earlier version tell
  their owner semantic retrieval is on while recall runs BM25 + graph only —
  edit the block by hand, or set `enabled = true` if you want the vector lane.
  The model-cache figure is corrected from `~118 MB` to `~465 MB` in the
  template, `prefetch-embed --help`, and the vector-store failure hint.
- **One source file could occupy two stored paths.** `Indexer::index_path`
  recorded the root exactly as spelled, so `index .` wrote `./crates/x.rs`
  while the session-start bootstrap's `index /abs/proj` wrote the absolute
  form. Both survived every reindex — purge-before-write only clears the
  flavour it was handed — and both competed for the same recall slots. Roots
  are canonicalised now. **Existing stores keep their duplicate rows: delete
  `graph.redb` and `fts.tantivy` under the project store and reindex to clear
  them.**
- **`detect_changes` resolves needles the way the indexer stores them** for
  absolute paths, and deliberately leaves relative ones alone — canonicalising
  a repo-relative diff path against the daemon's cwd (one directory shared by
  every project in service mode) could name a file in an unrelated repo.
- **The addon migration could lose entries.** `active_addons` is written
  before the rename, so a process killed mid-migration cannot drop them.
- **Addons are no longer copied into the project tier**; existing copies
  migrate to `.bak`, and `verify.rs` self-tests follow the no-copy contract.
- **Six `rule` dispatch arms swallowed their errors**; `cmd_rule_list` and
  `cmd_rule_gate` are fallible so the documented error paths hold.
- **The MCP daemon stopped reaping idle socket connections.**

### Changed

- Rule regexes compile once at load; the 8-positional-parameter rule builder
  is gone.
- `config get` and `pref get` share one `default_task_manager()`.

### Tests

- Test suites for the rule engine, addon migration, `enforce`, `update`, and
  the installer argv path, driven end to end through the built binary.
- `cli_rule_success_paths_unchanged` compares stdout as JSON documents rather
  than bytes: `hoangsa-proxy` enables serde_json's `preserve_order` and cargo
  unifies features across a workspace build, so key order differed between
  `cargo test -p hoangsa-cli` and `cargo test --workspace` — the suite passed
  alone and failed in CI.

## [0.6.0] - 2026-07-26

### Added
- **`hoangsa-cli update` and `hoangsa-cli uninstall`.** The tool could not
  report its own version or remove itself. Updating meant a Claude Code slash
  command; uninstalling meant cloning the repo, which `install.sh` said out
  loud: *"To uninstall, use scripts/uninstall.sh from a checkout of the repo."*
  - `update [--check] [--local] [--dry-run] [--yes]` reads the installed
    version from `<install dir>/manifest.json` and resolves the latest release
    tag. `--check` exits 10 when an update is available, so a hook or script
    can branch on it. Version comparison is numeric and strips the `v` prefix —
    comparing the tag `v0.6.0` against the manifest's `0.6.0` as strings
    reported an available update forever, on a machine that was up to date.
  - `uninstall --global|--local [--dry-run] [--purge]` removes binaries,
    manifest-tracked templates, managed hook entries, the MCP registration, the
    PATH block, and the model cache. `~/.hoangsa/memory`, `~/.hoangsa/share`,
    and every per-project `.hoangsa/` survive without `--purge`.
  - Removing the running binary is fine on Unix — the inode outlives the
    unlink — so the command completes after deleting itself.

### Fixed
- **The uninstaller silently left 71 of 99 tracked files behind.** The manifest
  is keyed by the *template source* path and the installer routes destinations
  separately (`workflows/x` → `hoangsa/workflows/x`, `skills/hoangsa/x` →
  `skills/x`). `scripts/uninstall.sh` joined the raw key onto the config dir,
  found nothing at that path, and moved on — printing "uninstall complete"
  either way. On this machine a dry run went from 36 paths to 107 once the
  mapping was applied. `route_rel` is now shared between install and uninstall
  so the two cannot drift, and the shell script applies the same mapping.
- **`/hoangsa:update` never worked.** It detected the install by reading
  `<config>/hoangsa/VERSION`, a file no installer has ever written, under a
  hardcoded `~/.claude` that is wrong for anyone with `CLAUDE_CONFIG_DIR` set.
  Every machine got "not installed" — in a shape indistinguishable from a
  genuinely missing install, which is the one answer a version checker cannot
  afford to get wrong, because nobody re-checks it. The workflow now calls
  `hoangsa-cli update`, and a `verify` gate rejects a reintroduced VERSION
  probe while still allowing the prose that explains why it was removed.
- The uninstaller needed `jq` to touch `settings.json` and the MCP config, and
  warned-and-skipped when it was absent — reporting success while leaving hook
  entries pointing at a binary it had just deleted. The Rust path has
  serde_json compiled in and cannot skip.
- `templates/commands/hoangsa/update.md` advertised "npm version checking"
  while its own workflow reads **"never invoke `npm` or `npx`"**.
- The fastembed cache size in `uninstall.sh` said `~118 MB`; it is ~465 MB
  measured (448 MB of ONNX weights plus a 16 MB tokenizer).
- **Gate 4's analyzers now ship with HOANGSA.** Cook's quality gate used to
  spawn four Claude Code agents installed separately (`pr-test-analyzer`,
  `silent-failure-hunter`, `type-design-analyzer`, `comment-analyzer`). On a
  machine without them the gate passed by default — the worst failure a gate
  can have, because it spends the reader's trust without checking anything.
  Four native analyzers replace them under `templates/agents/`:
  - `hoangsa-analyzer-coverage` — the one dimension a generic reviewer cannot
    do: `plan.json` already carries the `test_cases` and `edge_cases` a human
    approved, so coverage is a checklist, not a guess. Each declared case is
    matched to a test that drives the same input *and* asserts the stated
    outcome; a declared case with no covering test is CRITICAL. For every
    covering test it must name the single-line mutation that turns the test
    red — a test that catches no mutation is empty however well it reads —
    and flags the opposite failure, tests that go red without a behaviour
    change.
  - `hoangsa-analyzer-failure` — enumerates every error-handling site in the
    diff before ranking any of them, then per site: what breaks, who is
    misled, *what unrelated failures this handler also swallows*, and whether
    the silence is deliberate. Fail-open in an enforcement path is CRITICAL
    regardless of how unlikely the trigger looks.
  - `hoangsa-analyzer-types` — splits each type's invariants into enforced /
    checked / documented / assumed (the last two are the findings), demands a
    constructible invalid value before reporting anything, and rates
    encapsulation, expression, usefulness and enforcement 1–10 against defined
    bands so the numbers mean the same thing every run.
  - `hoangsa-analyzer-claims` — checks the prose nothing else checks. Every
    comment and doc line the diff touched is classified OBSERVED / DERIVED /
    PRIOR / ASSUMED per Worker Rules § Claim Discipline, then checked against
    the code: signatures, referenced symbols, counts, quantities, and whether
    the *reason* in a "safe because…" comment still holds. Also covers
    comments left standing on code the change edited underneath them.
  - All four are read-only, take their model from `resolve-model reviewer`,
    and are language-neutral by construction — a `verify` gate rejects any
    analyzer that names one language's syntax, since the same file grades Rust,
    Python, TS and Go projects alike.
  - `verify` also pins the three-way agreement between the install list, the
    shipped templates, and the workflow that spawns them by name.

### Changed
- **`hoangsa-simplify` says how to simplify.** It was fifteen lines, two of
  which were an essay about model routing, and none of which described the
  work. Rewritten against Anthropic's official `code-simplifier`, minus its
  stack-specific rules (that agent prescribes ES modules, arrow-function style
  and React props to every codebase it is pointed at) and plus the constraints
  that are actually HOANGSA's:
  - The agent has Edit and no Bash, so it cannot run a single thing it changes.
    That constraint now sets the bar: if a change cannot be seen to preserve
    behaviour by reading alone, it is out of scope. "The tests would catch it"
    is not a reason available to an agent that cannot run tests.
  - A stop list it must report rather than edit: public signatures, error
    semantics, concurrency and ordering, generated or vendored files, anything
    carrying a comment explaining why it is odd, and any step named by the
    task's `behavior` contract — Gate 5 checks the implementation against those
    steps, so collapsing one fails the gate even when every test passes.
  - An explicit account of over-simplification, because the failure mode is
    optimising for fewer lines: nested ternaries, dense one-liners, inlining a
    well-named helper, merging two functions that fail for different reasons.
  - `memory_impact` required before touching a symbol with callers outside the
    file. An empty report is a valid result.
- **`hoangsa-reviewer` says what it reviews against.** Thirteen lines became a
  contract, built from Anthropic's official `code-reviewer` (its 0–100
  confidence scale and the report-only-≥80 filter) plus what HOANGSA has and it
  does not: an approved spec to review against.
  - Three sources outrank the reviewer's instinct, in order: the DESIGN-SPEC
    and each task's `behavior` steps, then the project's written rules, then
    correctness. A dropped or reordered `behavior` step is a finding *even when
    every test passes* — that is what the contract is for. Anything outside the
    three is a style opinion and does not get to spend the gate.
  - It is the one Gate-4 agent with Bash, so it must use it: run the acceptance
    command and quote the real output, reproduce the bug with the smallest
    command that shows it, settle "this is never called" with `memory_impact`
    rather than by inspection. Read-only still means read-only.
  - Pre-existing defects are out of scope unless the change made them
    reachable. A passing review is a real result. Whatever could not be checked
    must be named — an unchecked dimension reported as checked is what makes a
    gate worthless.
- The language-neutrality gate covers every shipped agent, not just the Gate-4
  analyzers.

### Removed
- The three `verify` checks asserting the absence of `get-shit-done/`,
  `commands/gsd/`, and `gsd-*` files, plus the `find_files_matching` helper they
  were the only caller of. That layout was removed long ago and no code path can
  recreate it, so the checks could never go red — a gate that cannot fail is not
  a gate, it is 20 lines of the suite reading as coverage it does not provide.
- **The dream pass — background LLM consolidation of long-term memory.** The
  forget pass prunes on clocks and counters; it cannot see that two lessons say
  the same thing, or that a fact quietly stopped being true. So while the daemon
  is idle, a model now re-reads `MEMORY.md` / `LESSONS.md` / `USER.md` and
  returns a verdict per entry — keep, rewrite, merge, or drop.
  - Runs in the project directory, so the model verifies a claim against the
    real code before calling it stale. Unverifiable entries are kept: a wrong
    drop costs the user knowledge, a wrong keep costs a few tokens.
  - `[dream].mode = "review"` (default) writes proposals to `DREAM.md` and
    touches nothing. `"auto"` applies them, archiving every dropped entry to
    `<SURFACE>.dropped.md` with the model's reason and logging each op to
    `memory-history.jsonl`. Any unrecognised mode fails closed to review.
  - Verdicts address entries by snapshot index, never by fuzzy query, and each
    index may be claimed once — an out-of-range, duplicate, or textless verdict
    is dropped with a warning instead of aborting the pass. A model that
    answers in prose produces no verdicts rather than a guess.
  - Model comes from the harness CLI already on `PATH` (`claude -p`, then
    `codex exec`), so no API key is needed; override with `[dream].command`.
  - Gated by `enabled` (off by default), `idle_minutes`, `min_interval_hours`
    (persisted across daemon restarts), and `min_entries`.
  - Manual trigger: `hoangsa-memory memory dream [--force] [--dry-run]`.
  - Applying verdicts reconciles against the surface as it is on disk *at
    apply time*, matching entries by heading. The model call can run for
    minutes; a rewrite driven by the snapshot alone erased anything the agent
    appended in that window, with no archive row and no history line.
  - A failed pass consumes the interval too. Only the success path stamped
    the marker, and that marker is the daemon's only gate — a harness CLI
    that is present but exits non-zero turned a twice-a-day pass into an LLM
    subprocess every 15 minutes indefinitely, at debug log level.
  - `--dry-run` and `review` now count through the same planner the apply
    path uses, so a `rewrite` carrying `merge_with` reports the entries it
    will remove instead of `0 merged`.
  - The prompt budget is per surface, so a large `MEMORY.md` can no longer
    hide `lessons` and `user` entirely; verdicts addressing an entry that was
    omitted from the snapshot are refused rather than applied blind.
  - A type-level malformation in one verdict (`"index": -1`, missing
    `action`) skips that verdict instead of discarding the whole batch —
    which is what the surrounding docs already promised.
- **The forget pass now actually runs.** `MemoryManager::forget_pass` — TTL,
  capacity, decay eviction, low-confidence lesson drop, quarantine — had no
  caller outside its own tests since it was written, so no install had ever
  executed it. The daemon's new maintenance loop runs it before each dream.
- **Spec rigor gates** — the design phase now has to answer the questions it
  used to skip, and `hoangsa-cli` enforces it instead of asking nicely:
  - DESIGN-SPEC `## Behavior / Logic` (required for code specs): per-REQ
    trigger, steps, decision table, error paths, state transitions. `prepare`
    embeds it per task as `behavior`, and `envelope` renders it as the worker's
    behavior contract — a worker no longer receives a signature and a test name
    and invents the logic in between.
  - DESIGN-SPEC `## Risk Sweep` (required for code specs): 8 fixed risk classes
    — boundary, invalid input, **concurrency & TOCTOU**, idempotency & retry,
    partial failure & rollback, auth & permission, limits, backward compat —
    each APPLIES with concrete handling (plus an Edge Cases row proving it) or
    N/A with a reason. `validate spec` names any class that's missing.
  - `## Open Questions` entries must carry `RESOLVED` or `DEFERRED`;
    `validate spec` fails on a statusless row, so a question cannot reach the
    plan without having been put to the user. menu Step 6c and brainstorm
    Step 5c are the rounds that ask them.
  - `validate plan` requires a non-empty `behavior` on every `type: impl` task
    (waivable with an explicit `"N/A — <reason>"` entry).
- **Workers can now see the memory skills they were already told to use.** The
  worker skill registry listed 3 of the 11 shipped skills, while envelope
  Instruction 2 ordered a `memory_impact` call before touching any symbol —
  the skill that teaches exactly that was invisible. Added
  `memory-impact-analysis`, `memory-refactoring`, `memory-debugging` and
  `memory-exploring`. `memory-cli` (fights the daemon for the store lock) and
  `memory-guide` (tool-catalog tour) stay out on purpose, and common.md now
  says so.
- **Model routing is harness-aware.** With `"harness": "codex"` in
  `.hoangsa/config.json` (or `HOANGSA_HARNESS=codex`), a profile tier no
  longer resolves to a Claude model id — Codex scales by reasoning effort,
  not by swapping models, so fable/opus → `high`, sonnet → `medium`,
  haiku → `low`, and the Codex session model is left alone (`resolve-model`
  reports it from `~/.codex/config.toml` for information only). Worker
  envelopes open with `REASONING EFFORT:` instead of `MODEL:` there, and the
  Codex command-player rules were updated to read it — previously they were
  told to ignore the `MODEL:` line and nothing replaced it, so Codex workers
  carried no routing signal at all.
  - An explicit `install --harness <x>` now records `harness` in
    `.hoangsa/config.json`, so the choice reaches `resolve-model` without a
    manual edit. Other keys are preserved, a repeat install rewrites nothing,
    and `--dry-run` writes nothing. `--global` has no project config to write
    to and says so, pointing at `HOANGSA_HARNESS` (which overrides the file
    when set). `harness: "claude"` is now part of the default config.
- **`minimal` model profile is selectable.** It existed only in `model.rs`:
  absent from the `/hoangsa:init` wizard, the README table and every help
  topic, so it could only be reached by hand-editing `config.json`. Now
  offered in init Step 2b and documented as a fourth column. (It is `budget`
  with the orchestrator kept on sonnet — and therefore slightly *more*
  expensive than `budget`, despite the name.)
- `hoangsa-cli verify` pins the model-profile table in README.md and init.md
  against `model.rs` role by role — three copies of one table with nothing
  holding them together.
- **brainstorm**: unknown ledger (answered-from-context / answered-by-user /
  assumption — never a silent guess), blocking questions exempt from the
  6-question budget, a required failure & concurrency section, and Risk Seeds
  that feed the menu-phase Risk Sweep.
- **Worker execution fidelity** — the pipeline used to ask a worker only "is it
  green?", never "what did you actually do?":
  - Worker rules §7 now define a required completion report: one line per
    behavior step and per edge case with the `path:line` where it lives, or an
    explicit `⚠️ DEVIATED` / `⚠️ NOT HANDLED` line. Aggregating or omitting
    lines makes the task incomplete (cook Gate 8).
  - Worker rules §5 "the contract is an input, not a variable": retries change
    the implementation, never the acceptance command, an `expected` value, an
    edge case, or a behavior step. Weakening/skipping a test, stubbing the unit
    under test, or swallowing an error to go green is a blocker report.
    Expected outcomes come from the spec, never from observed output.
  - `hoangsa-cli validate scope <sessionDir> <taskId> [--rev <sha>]` — checks a
    task's commit against its `files`; touching an undeclared file is an error
    (cook Gate 7, fix Gate 5). "Only modify task.files" is now a gate, not an
    honor system.
  - cook re-runs a task's `acceptance` after the simplify pass and reverts the
    refactor commit on failure — the simplify agent has no Bash and could not
    verify what it changed, so that commit was previously unverified.

- **LLM reranking of recall results** (`[rerank] enabled`, off by default).
  Every stage before it ranks on form — term overlap, identifier equality,
  graph edges, rank position — and none reads a chunk to ask whether it
  answers the question, so a literal string match could outrank the hit that
  actually explains the thing. The pass shows the top `candidates` fused rows
  to a model and applies the ordering it returns. It runs the harness CLI
  already on `PATH` (`claude -p`, then `codex exec`), so no API key is needed.
  Because recall sits on the agent's hot path it is **fail-open** — missing
  binary, timeout, non-zero exit, prose instead of JSON, all return the fused
  order untouched — and **reordering-only**: the model cannot add, drop or
  duplicate a result, and anything it does not mention keeps its fused rank at
  the back. It also materializes a window larger than `top_k` before ranking,
  so it can rescue a good hit sitting just past the cutoff rather than only
  shuffling what already made it — verified against a live `claude -p`: an
  answer planted at index 23 of a 24-wide window came back at rank 0, a decoy
  stuffed with the query term lost to the function that implements it, a
  Vietnamese query found the right English code, and a snippet carrying
  "IGNORE ALL PREVIOUS INSTRUCTIONS" did not move the ranking.

### Changed
- **Semantic retrieval (the ONNX embedder) is now opt-in.**
  `[vector_store] enabled` defaults to `false`. It is the heaviest component
  in the tool — a ~465 MB model cache plus a resident ONNX session whose
  CPU arena ratchets to ~150-300 MB — and BM25 + symbol + graph retrieval
  cover most recalls without it, so a new install no longer pays for it before
  deciding it wants it. A default-path `memory show` now peaks at ~19 MB RSS
  and downloads nothing. Turn it on per project in `<memory root>/config.toml`
  (`[vector_store]` / `enabled = true`) and warm the cache once with
  `hoangsa-memory prefetch-embed`; the global `no-embed` marker still
  overrides it to off everywhere.

### Removed
- **`[curation].memory_mode = "review"` and the `*.pending.md` staging path.**
  Turning it on staged every new fact and lesson into `MEMORY.pending.md` /
  `LESSONS.pending.md` and told the caller to "run `memory_promote` to accept".
  `memory_promote` never existed — not as an MCP tool, not as a CLI
  subcommand — and `promote_pending_*` / `reject_pending_*` /
  `read_pending_*` had no caller outside their own tests. Nothing read the
  pending files either: not `memory_show`, not `memory_wakeup`. Enabling the
  mode silently black-holed memory. The `stage` argument on
  `memory_remember_fact` / `memory_remember_lesson` (advertised in the MCP
  catalog) reached the same dead end from `"auto"` mode.

  `[dream].mode = "review"` now covers the same intent — hold proposals for a
  human — and it works, so the half-built path is gone rather than finished.
- **A duplicate lesson trigger no longer vanishes.** It used to be staged into
  the unreadable pending file; `memory_remember_lesson` now returns an error
  carrying the existing advice and the exact `memory_replace` call that
  resolves it.
- **Docs that described a CLI and tool surface that does not exist.** Verified
  every documented name against `dispatch.rs` and the real `--help`:
  `memory-cli` listed `setup`, `uninstall`, `eval`, `domain sync`, `skills`,
  and six `memory` subcommands (`pending`, `promote`, `reject`, `forget`,
  `log`, `nudge`) — none are commands on this binary — while omitting `init`,
  `archive`, `prefetch-embed`, `projects`, and `lesson-feedback`.
  `memory-guide` documented nine MCP tools that do not exist and omitted
  twenty that do. Five skills instructed agents to call
  `memory_lesson_outcome`, which is not a tool — the Stop hook already bumps
  those counters via `memory lesson-feedback`. `check.md` counted entries in
  the pending files.

  `memory-guide` was then rewritten against `catalog.rs`: every one of the 31
  real tools is documented with its actual parameters, including the graph
  suite (`memory_graph_query` / `_paths` / `_communities` / `_processes`,
  `memory_taint_paths`, `memory_event_trace`), the archive suite
  (`memory_archive_*`, `memory_turn_save`, `memory_turns_search`), and
  `memory_wakeup` / `memory_detail`. The three MCP prompts were named with
  dots (`memory.reflect`); they use underscores. Every command in the CLI
  parity table was executed to confirm it resolves.
- **Stale `ChromaDB` in user-facing text.** The Python sidecar was replaced by
  the in-process sqlite vector store, but `archive ingest` / `archive purge`
  still advertised ChromaDB in their `--help`, and the archive tracker's docs
  claimed verbatim content lived there. Historical references that explain
  what the current design replaced are left alone.

### Fixed
- **Agent model pinning defeated config routing.** `hoangsa-worker-impl` and
  `hoangsa-worker-readonly` pinned `model: sonnet`, `hoangsa-reviewer` pinned
  `model: haiku` — a pinned tier wins whenever a spawn call omits the model, so
  a `quality` profile (worker/reviewer → opus) or a `budget` profile
  (→ haiku) silently did nothing. The three routed agents now pin no model and
  take it from the envelope's `MODEL:` line / `REVIEWER_MODEL`
  (`resolve-model <role>`); `hoangsa-simplify` keeps an intentional, documented
  `haiku` pin. `hoangsa-cli verify` guards the invariant so a pin can't creep
  back. Existing installs keep the old agent files until `/hoangsa:update`.
- **`validate plan` was silent between 45k and 80k tokens.** `prepare` gate 3
  and `budget.rs` both treat 45k as the per-task split target, but the
  validator only warned above 80k — so a task the planner should have split
  validated clean. It now warns at both tiers (45k target, 80k hard limit).
- **`state init` omitted `tasks`**, so `state get` returned a different shape
  before and after the first `state update`. It now writes `"tasks": []`.
- **A second `hoangsa-memory-mcp` on the same project crash-looped.** redb's
  store lock is exclusive per process, so a second instance (two Claude Code
  sessions on one repo) exited at startup with "Database already open. Cannot
  acquire lock." and the client respawned it — a spawn loop that reads as a
  runaway CPU hog. The second instance now relays its stdio to the incumbent
  over the existing `mcp.sock` sidecar (same wire format), and a lost startup
  race waits ~1 s for the winner's socket instead of failing. Covered by
  `tests/relay.rs`, which spawns the real binary twice.
- **`common.md` told workers to run `hoangsa-cli media analyze`**, a command
  that has never existed — video analysis in the media-detection step was
  dead on arrival. Replaced with the real `media frames` → `montage` → `diff`
  sequence (which the `visual-debug` skill already had right).
- **`hoangsa-cli help media` documented flags that don't exist** (`--output`,
  `--fps`, `--out`) and gave `media diff` the wrong signature — it takes the
  frames directory, not two images. `help resolve-model` listed 4 of the 8
  roles, claimed roles come from config.json, and never mentioned the four
  profiles, the `fable` tier (which belongs to no profile and is reachable
  only via `model_overrides`), or the fact that the whole mechanism is inert
  under `--harness codex`. Both corrected against the implementation.
- `hoangsa-cli verify` now covers the skill tree: every skill ships a
  `SKILL.md`, every skill named in common.md's worker registry exists, and
  the fallback copy of that registry in `envelope.rs` matches common.md —
  common.md says "edit here, not in Rust", which was previously unenforced.
- **`stats cache` billed unrecognized models at sonnet rates, silently.**
  `get_pricing` matched `claude-opus-4*`, `claude-opus-4-1`/`-3`,
  `claude-sonnet*`, `claude-haiku-4-5`/`-3` and `claude-fable*`, and dropped
  everything else into the sonnet branch — so `claude-opus-5` was costed at
  $3/$15 per MTok with no indication anything was assumed. Unknown models are
  now flagged: every session carries a `cost_basis` field that reads `exact`
  or names the models that fell back. Prices themselves are unchanged; adding
  a real entry for a new model is still a one-line edit.
- **The service daemon unlinked sockets it did not own, and never rebound.**
  `spawn_listener` returns `Ok(())` with no handle when a stdio instance
  already owns a project's socket — but `unregister` removed the socket file
  regardless, pulling it out from under that live process and recreating the
  redb crash loop for the next spawn. It now unlinks only when its own
  listener handle proves it bound the socket. The mirror gap: `reconcile` only
  bound slugs it did not already know, so a slot skipped once stayed
  listener-less for the daemon's whole lifetime; it now retries registered
  slugs that have no listener.
- **Two Claude Code sessions in one repo corrupted each other's enforcement
  state.** `.hoangsa/state/enforcement.events` is per-project but its contents
  are per-session facts. Session B's SessionStart deleted the whole file, so
  session A was blocked on its next edit for a `memory_impact` it had already
  run; and in the other direction A's event satisfied B's gate for a file B
  never analysed. Events now carry their owning session and `state-clear`
  prunes only its own. (`state-clear` was also draining stdin twice, so the
  `/clear` cost-baseline reset never fired — same read now serves both.)
- **Concurrent `index` runs left one project with no embeddings, silently.**
  The vector lock is machine-global on purpose — it stops two ~300 MB ONNX
  embedders being resident at once — but the loser returned immediately and
  indexed *without* embeddings, announcing it on a stderr line the bootstrap
  worker discards. It now waits (up to 3 min) for the shared embedder instead
  of degrading.
- **SQLite had WAL but no `busy_timeout`**, so the default 0 ms handler failed
  a second writer instantly instead of waiting — an `archive purge` beside an
  `archive ingest` errored mid-purge with no retry. All three connections
  (`episodes.db`, `archive_sessions.db`, `vectors.sqlite`) now wait 5 s.
- **`hsp` hung on any command that backgrounds a process.** The reader threads
  were joined unbounded, so a grandchild inheriting stdout (a watcher, a
  daemon started by an npm script) pinned the tool call until *that* process
  died — measured 4.0 s for `sh -c 'sleep 4 & echo hi'`, now 1.2 s with the
  output intact and a note on stderr. A user Rhai handler also had no
  operation ceiling, so an accidental infinite loop hung every tool call for
  that command with no diagnostic; the interpreter is now bounded.
- **Concurrent writers lost updates to `projects.json` and `LESSONS.md`.**
  Making the *publish* atomic (above) stopped the files being corrupted, but
  load → mutate → save was still unserialised: 40 concurrent project
  registrations silently dropped 20, and 12 concurrent lesson bumps landed 1.
  A dropped slug is not cosmetic either — the service daemon's registry watcher
  reads the removal as a de-registration and unlinks that project's live
  socket. Both paths now hold an advisory file lock across the whole
  read-modify-write (measured 40/40 and 12/12 after). The lock is advisory,
  released by the OS on exit, and falls through after 5 s: a lost update is
  recoverable, a hung memory write blocks the agent.
- **taste inherited cook's Tier 2 as its own Gate 1 on every normal run.**
  Cook auto-chains into taste, so `verified_head` always matched HEAD and the
  one gate in the verification phase with a real command behind it was skipped
  — while the same file claims independence twice. Cook runs a *suite*; Gate 1
  is a *per-task acceptance command*; those are not the same check. Inheritance
  now requires the task's `acceptance` to appear verbatim among the commands
  cook actually ran, and whatever is inherited is counted in the summary
  (`Gate 1: N run, M inherited`) instead of vanishing into a pass.
  taste's change-aware pass also hardcoded `git diff main...HEAD`, which yields
  an empty diff — and a silent no-op — on any repo whose default branch is
  `master`; it now resolves `$BASE_BRANCH`.
- **`validate scope` was unusable in a monorepo and passed every merge
  commit.** `git show --name-only` prints repo-root-relative paths whatever
  directory it runs in, but they were resolved against `workspace_dir` — so
  any task whose workspace was not the git root reported the same file as both
  "touched but undeclared" and "declared but unchanged". Both lists are now
  resolved against their own root and canonicalized, so a symlinked path
  (`/tmp` → `/private/tmp`) no longer breaks the comparison either. And a
  merge commit prints no file list at all, which the gate read as "touched
  nothing" and approved — the one commit shape that can carry arbitrary files
  is now refused outright.
- **Workers received no lessons on any project with a global memory root.**
  `envelope` read `<workspace>/.hoangsa/memory/LESSONS.md` directly, so every
  project migrated to `~/.hoangsa/memory/projects/<slug>/` shipped an empty
  lessons section — the identical bug was already fixed in the SessionStart
  hook and this copy was left behind. It now tries the resolved root and the
  local path (resolution only prefers a local root once it is *populated*, so
  a project with LESSONS.md and no index needs the fallback). The
  `lesson-feedback` shell-out in the Stop hook had the same hardcode and was
  dropping every counter update.
- **The dream pass could delete an entry the model told it to keep.**
  `reconcile` matched entries by heading; with two entries sharing one, a
  concurrent delete of the first made the survivor pop the first queued plan
  slot. Entries are now matched on their full text — the exact bytes the model
  judged — so two identical entries stay interchangeable and two different
  ones can never be confused.
- **Every workflow invoked the CLI through a path that has never existed.**
  88 call sites across 16 files ran `$HOANGSA_ROOT/bin/hoangsa-cli`, but the
  binary installs under `~/.hoangsa/bin/` while `$HOANGSA_ROOT` names the
  template tree in the Claude config dir — which has no `bin/` at all. Nothing
  in the prompt layer even assigned `HOANGSA_ROOT`. `common.md` now resolves
  three separate roots (`HOANGSA_BIN` via `command -v` with a `~/.hoangsa/bin`
  fallback, `HOANGSA_ROOT` for workflows, `HOANGSA_AGENTS` for agent
  definitions), every call site uses `$HOANGSA_BIN`, and cook's agent table
  points at `<config>/agents/` where the installer actually writes them.
  `hoangsa-cli verify` now fails on any reappearance of either bad path and on
  a `common.md` that stops assigning the variables the rest of the layer
  spends.
- **Commands were denied the tools their workflows require.** `/hoangsa:qc`
  produces `QC-TESTCASES.md`, `QC-REPORT.md` and an evidence tree, and
  `/hoangsa:taste` writes task statuses back to plan.json — neither command
  allowed `Write` or `Edit`. `/hoangsa:check` and `/hoangsa:plate` call
  `memory_*` MCP tools their allowlists never mentioned. `/hoangsa:ship`
  declared `Agent` where every other command declares `Task`.
- **`install` destroyed exactly the files the user had edited.** The
  patch-backup gate required a previous manifest *and* an entry in it, so when
  the manifest was missing (a different `HOANGSA_INSTALL_DIR`, an interrupted
  uninstall) the only files the install overwrote were the ones whose content
  had drifted — the user's edits — and it backed up none of them. The gate now
  skips the backup only when the manifest proves the file on disk is
  byte-for-byte what we last wrote.
- **Upgrading from 0.5.0 silently routed every role to haiku.** 0.5.0's
  documented `pref set . profile minimal` wrote a workflow-preset name into the
  model-routing key, and `minimal` is valid in both vocabularies. `install` now
  migrates it — `profile` back to `balanced`, the preset moved to
  `preferences.workflow_profile` — and says so on stderr. `full` is
  unambiguous; `minimal` is only migrated when `test_runs == 0` fingerprints it
  as the preset, so a deliberate `minimal` routing choice is left alone. The
  migration runs first and unconditionally: it repairs damage an earlier
  version did, so it must not depend on this install succeeding.
- **`require-detect-changes` could deadlock a commit permanently.** The
  PostToolUse handler read `tool_result`; Claude Code sends `tool_response`
  (an object for MCP tools), so the field was never found and every event was
  logged with an empty file list — while the guard blocked on exactly that
  emptiness. An agent that ran `memory_detect_changes` as instructed stayed
  blocked forever with no exit but `enforce override`. The handler now reads
  the right field, and the guard keys on *whether the tool ran* rather than on
  how many paths could be scraped out of its output.
- **`archive ingest` ignored `[vector_store] enabled` and the `no-embed`
  marker entirely.** It read only `data_path` from the config, never
  `enabled` — so a machine installed with `--no-embed` still loaded the full
  ONNX model and embedded every transcript chunk. Because the hook spawns it
  detached with no console, this surfaced only as an unexplained process at
  **770% CPU on an 11-core box**. It now refuses to run when embeddings are
  disabled.
- **ONNX Runtime sized its thread pool to the core count.** Nothing capped it,
  so one embed pass saturated the machine. Capped via ORT's global threading
  options to half the cores (min 1, max 4), overridable with
  `HOANGSA_ONNX_THREADS` (`0` restores ORT's default). The hook-spawned
  background ingest additionally runs under `nice -n 10` with 2 threads.
  Measured on the same command: 770% → 396% foreground, → **213% niced** for
  the background path. (Note for anyone tempted by the obvious fix:
  `OMP_NUM_THREADS` and `ORT_INTRA_OP_NUM_THREADS` have no effect on this ORT
  build — measured at 836% with both set.)
- **The MCP server let a client pin every core.** Numeric tool arguments were
  taken verbatim: `memory_turns_search`'s `top_k` reached SQLite as
  `k as i64`, so `usize::MAX` became `-1` — which SQLite reads as *no limit*
  and dumps the whole archive; `memory_recall`'s `top_k` reached
  `TopDocs::with_limit` and `Vec::with_capacity`; and four graph traversals
  took `max_depth`/`max_nodes`/`max_findings` straight from the caller while
  their siblings in the same file already clamped. A `memory_taint_paths`
  `sources: [""]` matched every node (`contains("")`), so the early-exit never
  fired. Measured at 890% CPU from one malformed argument — and these
  arguments are written by a model. All are now clamped to their declared
  schema maxima, and blank taint patterns are dropped.
- **One CJK or Vietnamese turn poisoned the archive permanently.**
  `archive_tools.rs` truncated `commit_sha`, `session_id` and turn content at
  raw byte offsets. `memory_turn_save` panicked outright, and because the
  content is *persisted*, every later `memory_turns_search` that matched it
  re-crashed the server. Now truncated on char boundaries.
- **`atomic_write` was not atomic between processes.** Both
  `hoangsa-memory-core::io::atomic_write` and `markdown.rs::write_atomic` used
  a fixed `<path>.tmp` shared by every writer, so two processes interleaved
  *inside the temp file* and renamed a spliced result into place. Reproduced
  corrupting `projects.json` — which then stopped the service daemon for every
  project and never self-healed — and splicing two bodies together in
  `MEMORY.md`. Temp names now carry the pid.
- **`state update` truncated the live session file.** It wrote `state.json`
  with a bare `fs::write`, so a short write over an in-flight long write left
  valid JSON followed by a stale tail; 16 concurrent writers corrupted the
  file in 2 of 15 rounds. Now written via temp + rename (0 of 12 rounds after
  the fix). Concurrent patches can still be lost — that is recoverable, the
  destroyed file was not.
- **The simplify agent was pinned to `haiku`.** It edits code, has no Bash so
  it cannot verify anything it changed, and commits its own `refactor(...)`;
  cook's acceptance re-run only catches a failing test. It is now a routed
  role (`resolve-model simplify`) that follows `model_profile` like any other
  code-editing agent — quality→opus, balanced→sonnet, budget/minimal→haiku.
- **`pref set . profile <x>` silently rewired model routing.** `profile` is
  the top-level model-routing key (`quality|balanced|budget|minimal`), but the
  workflow quality preset — a different feature with the vocabulary
  `full|balanced|minimal` — wrote its name into that same key. `full` is not a
  routing profile, so routing fell back to balanced while `resolve-model`
  reported `"profile": "full", "source": "profile"` as though it had resolved;
  and `minimal` exists in *both* vocabularies with different meanings, so
  choosing the cheap workflow preset also downgraded every model to haiku.
  `pref get . profile` compounded it by reading `preferences.profile`, which
  nothing wrote — it always returned null.
  The preset is now `workflow_profile`, stored under `preferences`, and never
  touches model routing. `pref set . profile` still works as a deprecated
  alias (with a warning) and `pref get` resolves it too. `resolve-model` now
  emits a `warning` when `config.json` names a profile that does not exist,
  so a config already damaged by the old behaviour reports itself instead of
  quietly running on balanced.
- **Three reachable panics on non-ASCII or wildcard input**, all found in a
  pre-release sweep and all now covered by regression tests:
  - `memory_recall` aborted on a long non-ASCII query. `sanitize.rs` sliced
    the tail at `len - MAX_QUERY_LEN` with no char-boundary guard —
    deterministic for CJK, a coin flip for Vietnamese, and it runs on *every*
    recall. In stdio mode it took the whole server down.
  - `--pdg` indexing aborted on any statement over 120 bytes containing a
    multi-byte character (`pdg.rs` truncated on a raw byte offset).
  - `memory_graph_processes({entry_globs: ["*"]})` panicked on an inverted
    slice, even though the tool schema advertises `*` support.
- **`hsp hook rewrite` broke every command with a leading env assignment.**
  It prepended `hsp ` to the whole string, so `RUST_BACKTRACE=1 cargo test`
  became `hsp RUST_BACKTRACE=1 cargo test` — the shell then looked for a
  program named `RUST_BACKTRACE=1` and the tool call died with 127. The
  assignments now stay in front (`RUST_BACKTRACE=1 hsp cargo test`), which
  preserves the semantics since `hsp` passes its environment to the child.
- **A relay whose owner died never exited.** Its stdin read sits in a
  blocking-pool thread that runtime shutdown joins forever, so the process
  stayed alive answering nothing until its client happened to write another
  byte. `run_stdio_proxy` now reports why it stopped and the relay exits
  outright when the owner is gone — it holds no socket and no store lock.
- **`install` was the only command that ignored the resolved project root.**
  Every other dispatch arm gets main.rs's `cwd` (which walks up to
  `.hoangsa/`); `cmd_install` called `current_dir()`. Running
  `install --harness codex` from a subdirectory wrote `harness` into a config
  that `resolve-model` never reads. The harness is also now recorded *after*
  the install succeeds, not before it can fail.
- **Monorepo detection only saw packages a workspace file declared.** A
  sub-project with its own manifest that no workspace lists — the common shape
  for a web frontend living inside a Rust repo — was invisible to every check
  in init-detect A15, so it landed in neither `packages` nor `frameworks` nor
  `tech_stack`, and its worker-rule addons were never matched. A15 now sweeps
  for unclaimed manifests (pruning `node_modules`/`target`/`vendor`/…) and
  reconciles them against the declared members.
- **`codebase.packages[].frameworks` was referenced but never written.** The
  addon matcher in `init.md` reads it as one of three inputs and init-detect
  A15 is told to detect it, but no package schema ever declared the field — so
  that input was permanently empty. Added to the schema. `hoangsa-cli verify`
  now cross-checks every `packages[].<field>` a workflow reads against the
  fields init actually writes, so a prompt can't reference a phantom key again.
- Dropped four config keys that nothing ever read: `preferences.auto_compact`,
  `auto_compact_interval`, `auto_compact_cooldown_secs` (present in the
  defaults and the `pref` whitelist, with no consumer in any crate or
  workflow), `codebase.testing.file_pattern`, and `codebase.packages[].dev`.
  Existing configs keep the keys harmlessly; `pref get auto_compact` now
  reports an unknown key instead of returning a setting that did nothing.
- Repaired stale self-tests in `hoangsa-cli verify`: the TEST-SPEC fixture
  predated the `## Edge Cases` requirement (now satisfied, with a negative
  twin), and `cook → context get` asserted a command cook stopped calling when
  `envelope` took over prompt assembly.

## [0.5.0] - 2026-07-16

### Added
- **Full OpenAI Codex CLI support** (`--harness codex`):
  - `hoangsa-cli hook codex <handler>` — Codex hook entry point reusing every
    existing handler; output auto-translated to Codex wire (no
    `decision:"approve"`, advisory reasons → `additionalContext` /
    `systemMessage`, `deny_unknown_fields`-safe).
  - `hoangsa-cli install --harness codex` — hooks.json merge (idempotent,
    preserves user entries, sweeps legacy hand-rolled hoangsa entries),
    `[mcp_servers.hoangsa-memory]` registration in `config.toml` via
    comment-preserving TOML edits, skills → `~/.agents/skills/hoangsa/`,
    generated command wrapper skills + shared `hoangsa-command-player`,
    workflows → `~/.codex/hoangsa/workflows/` with path/MCP-name adaptation.
  - `hoangsa-cli codex render <command> --arguments …` — workflow renderer
    backing the Codex command skills.
  - `hsp init|uninit --codex` and `hsp hook rewrite --codex`
    (`permissionDecision:"allow"` + `updatedInput`); doctor checks for
    `~/.codex/hooks.json`.
  - Enforcement guards understand Codex `apply_patch` payloads (per-file
    first-touch impact checks parsed from the patch envelope).
  - Archive ingest + session usage read Codex rollout files
    (`~/.codex/sessions/**/rollout-*.jsonl`) alongside Claude transcripts.
- **Claude Cowork memory** (`--harness cowork`): registers `hoangsa-memory`
  in Claude Desktop's `claude_desktop_config.json` (bridged into the Cowork
  task VM). Memory tools only — hooks/skills need a plugin bundle, and the
  host CLI is unreachable from inside the VM.
- **Claude plugin bundle** (`plugin/` + `.claude-plugin/marketplace.json`):
  install the `/hoangsa:*` commands, worker agents, skills, and workflows
  via `/plugin marketplace add unknown-studio-dev/hoangsa` — works in Claude
  Code and Claude Cowork. Instruction layer only: hooks and the MCP server
  stay installer-managed (bundling them would double-register on Claude Code
  and cannot reach the host binaries from Cowork's VM). Regenerate with
  `make plugin` (`hoangsa-cli plugin package`).

### Changed
- `AGENTS.md` guidance block now uses a plain file reference instead of the
  Claude-only `@path` import (Codex and other harnesses read it literally).

## [0.4.0] - 2026-07-15

### Added
- **Code graph traversal & analytics.** New MCP tools + `hoangsa-memory graph …` CLI:
  `memory_graph_query` (BFS traversal from seed symbols → callers/callees/refs/imports,
  edge-kind filtered, JSON or Graphviz DOT export), `memory_graph_paths` (shortest
  dependency path A→B), `memory_graph_communities` (architecture clusters via label
  propagation), `memory_graph_processes` (execution-flow tracing from entry points).
- **PDG + taint analysis** (opt-in `index --pdg`). Statement-level control-flow (`Cfg`) and
  data-dependence (`DataDep`) graph for Rust & Python, with an interprocedural call-arg
  bridge. `memory_taint_paths` MCP tool + `graph taint` CLI trace source→sink flows over
  `DataDep`/`Calls` only (never control-flow reachability), with built-in default
  source/sink patterns.
- **`memory_event_trace`** surfaced — publishers/subscribers of an event-bus topic.
- **Self-repair loop.** `prompt-guard` (UserPromptSubmit) frustration sensor with
  lesson-gated Stop escalation; `hoangsa-memory lesson feedback` wires the previously
  dormant lesson success/failure counters so lessons accrue evidence.
- **`graph-affordance` hook** — a non-blocking PreToolUse nudge toward the graph tools on
  repeated code-searching, plus sharpened tool descriptions and graph tools surfaced in the
  memory-guidance list and the `memory-exploring` / `memory-cli` skills.
- **`stats report --all`** cross-session token aggregation; **model routing** — the worker
  envelope now stamps the config-resolved `MODEL:` on line 1 and records it in stats.
- **`/hoangsa:qc`** workflow — spec-driven QC with evidence-backed pass/fail verdicts.
- **Wiring & drift gates** (machine-enforced consistency): `no_orphan_tools` (every MCP
  tool must be surfaced to the agent), `catalog↔dispatch`, `hook↔dispatch`, and
  `envelope MODEL-line` parity tests.
- **CI workflow** running `cargo test`, `clippy -D warnings`, and a changed-file `rustfmt`
  check on every push and pull request.
- **Claude Fable 5 support** — the `fable` alias (tier above opus, ~2× opus pricing) is
  recognized in cache cost analytics, the statusline, the init per-role model picker, and
  the READMEs; route it per role via `model_overrides` in `config.json`.

### Changed
- Workflows refactored from step-by-step scripts to **contracts + CLI gates**: spec
  contracts machine-enforced (`validate spec|tests|plan`), worker-prompt assembly moved
  into the CLI (`envelope`, `rules compose`), bulk specs lazy-loaded, and an efficiency
  loop (no duplicate cook→taste test runs, stats consumed by `check`/`menu`, phase
  chaining).
- Large modules split into submodules (`hook.rs`, `install.rs`, `server.rs`).

### Fixed
- Patched 8 Dependabot advisories (1 high).
- Statement text is now persisted into the graph node payload so taint patterns match
  statement content, not just the FQN.
- Revived `lesson-guard`, which was silently inert (hardcoded memory root missed migrated
  projects; the query CLI stripped bodies the guard read).
- Audit round 2: removed unused dependencies and dead code, tightened the recall/hook hot
  paths, and fixed a machine-dependent test.

## [0.3.0] - 2026-07-01

### Added
- Zero-dep `curl | sh` installer as a per-tag GitHub Release asset. See README for the one-liner.
- New Rust subcommand `hoangsa-cli install [--global|--local] [--install-chroma] [--dry-run]` owning all install logic.
- CI smoke tests on alpine, ubuntu, and macOS for the install pipeline.
- `scripts/uninstall.sh [--global|--local] [--dry-run] [--purge]` — standalone POSIX-sh uninstaller that removes binaries, manifest-tracked templates, managed hook entries, the `hoangsa-memory` MCP registration, and the managed PATH block.
- **Global rules layer.** Enforcement now merges a global `~/.hoangsa/rules.json` under the project `.hoangsa/rules.json`; a project rule overrides a global rule with the same `id` (set `enabled: false` to disable a global rule per project). Each layer degrades independently, so a missing or malformed file at one layer never disables the other.

### Removed
- `--uninstall` flag on `hoangsa-cli install` (was a stub returning exit 4). Use `scripts/uninstall.sh` instead.

### Changed (BREAKING)
- **Rules are now purely file-driven — no built-in defaults are applied implicitly.** A project with no `.hoangsa/rules.json` (or an empty one) enforces nothing. Stateful rules (`require-memory-impact`, `require-detect-changes`, `no-git-add-ignored`) now fire only when explicitly listed **and** enabled in the merged config; the previous behaviour of implicitly enabling a stateful rule whose id was absent from `rules.json` is gone. Run `hoangsa-cli rule init` (or add the rules) to opt back in.
- **Internal `thoth-*` crates renamed to `hoangsa-memory-*`.** The public
  surface (binaries `hoangsa-memory`, `hoangsa-memory-mcp`; install dir
  `~/.hoangsa/memory/`; MCP tool names `mcp__hoangsa-memory__memory_*`)
  was already on the new name — this pass aligns the Rust workspace:
  `thoth-core` → `hoangsa-memory-core`, `thoth-parse/store/graph/retrieve/
  mcp` → `hoangsa-memory-{parse,store,graph,retrieve,mcp}`, `thoth-memory`
  → `hoangsa-memory-policy`, `thoth-cli` → `hoangsa-memory`. Internal
  only — no user-facing CLI or MCP tool name changed.
- **`.thothignore` renamed to `.memoryignore`.** Installer seed, helper
  names, and status JSON fields renamed accordingly. Existing projects
  should delete `.thothignore` and re-run `hoangsa-cli install --local`
  to get the new file, or rename manually.
- **MCP private RPC method `thoth.call` removed.** Use `hoangsa-memory.call`
  (already supported). MCP prompt names renamed:
  `thoth.reflect/nudge/grounding_check` → `memory_reflect/memory_nudge/
  memory_grounding_check` (snake_case to match tool names).
- **Preference key `thoth_strict` renamed to `memory_strict`.** The CLI
  now migrates existing `.hoangsa/config.json` files on read — you can
  also edit the key manually.

- **Node/npm packaging removed.** The `hoangsa-cc` npm package, the six `@hoangsa/cli-*` platform packages, `bin/install` (Node), `package.json`, and `pnpm-lock.yaml` are gone. Installation is exclusively the native `curl | sh` installer that downloads pre-built binaries from GitHub Releases. Existing `npx hoangsa-cc` invocations stop working — switch to the curl one-liner in the README.
- Release workflow rewritten to native-only: one `build` matrix job per supported triple (`linux-{x64,arm64,x64-musl}`, `darwin-{x64,arm64}`) plus an `assemble-release` job that tarballs binaries + templates and uploads them to the GitHub Release. The `publish` (npm) job was deleted. Windows is no longer produced because the installer does not support it.
- `--global` install mode no longer writes to the current working directory. Previously `.mcp.json`, `.hoangsa/rules.json`, and `.thothignore` were written to `cwd` even in global mode; now they are never created by `--global`. Global MCP registration now lives in `~/.claude.json`.
- `hoangsa-memory` and `hoangsa-memory-mcp` binaries are now installed to `~/.hoangsa/bin/` regardless of `--global` or `--local`.
- `--task-manager` is now a flag (was an interactive prompt only).
- `templates/workflows/update.md` rewritten to drive updates through the native installer (GitHub Releases API + `install.sh`) instead of `npm view` / `npx hoangsa-cc`.

### Fixed
- Drift bugs in the previous Node installer where `--local` tried to build memory binaries from source.
- `verify` integration assertions were grep-ing templates for the retired substring `"thoth"` / `"THOTH"`; now checks for `"hoangsa-memory"` / `"memory_"`.

### Known follow-ups
- ChromaDB collection names `thoth_code` and `thoth_archive` and the
  SQLite schema-version stamp table `_thoth_meta` are **not** renamed
  yet — changing them strands existing users' embeddings and history.
  A dedicated migration path is needed. Until then these legacy names
  remain on disk and in code.
- `install::cleanup_thoth_keys` is retained to strip `thoth*` top-level
  keys and hook entries from pre-rename Claude Code settings.
