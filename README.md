# HOANGSA

> Context engineering for coding agents — Claude Code, Codex, Cowork.

![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)
![Claude Code](https://img.shields.io/badge/Claude_Code-compatible-blueviolet.svg)
![Built with Rust](https://img.shields.io/badge/Built_with-Rust-orange.svg)

<div align="center">

### 🇻🇳 Hoàng Sa, Trường Sa là của Việt Nam 🇻🇳

🇨🇳 中文 · 西沙群岛和南沙群岛属于越南。🇻🇳<br>
🇺🇸 English · Hoàng Sa (Paracel Islands) and Trường Sa (Spratly Islands) belong to Vietnam. 🇻🇳<br>
🇯🇵 日本語 · ホアンサ（パラセル）諸島とチュオンサ（スプラトリー）諸島はベトナムの領土です。🇻🇳<br>
🇪🇸 Español · Las islas Paracel y las islas Spratly pertenecen a Vietnam. 🇻🇳<br>
🇷🇺 Русский · Острова Хоангша и Чыонгша принадлежат Вьетнаму. 🇻🇳<br>
🇫🇷 Français · Les îles Paracels et les îles Spratleys appartiennent au Vietnam. 🇻🇳<br>
🇰🇷 한국어 · 호앙사 군도와 쯔엉사 군도는 베트남의 영토입니다. 🇻🇳

</div>

---

## The problem

An agent's output degrades as its context window fills. Ask for a feature and
you get good code for the first file, plausible code for the fifth, and by the
tenth it has forgotten a type it defined itself. Bigger context windows push
the cliff back; they don't remove it.

The usual answers — "be more specific", "break it into steps" — are advice.
HOANGSA is structure.

## The idea

Split the work into tasks. Give each task its **own fresh context window**
holding only what it needs: the files it may touch, the behaviour it must
implement, the command that proves it worked. The orchestrator never writes
code — it dispatches, checks, and assembles.

Everything else follows from that:

- A task needs a **spec** precise enough to execute from, so there is a design
  phase that produces one.
- A worker with no history needs its **rules and context handed to it**, so
  prompts are assembled by a CLI, not improvised by an agent.
- A claim of "done" from a fresh worker is unverifiable, so **every deliverable
  has a command that must pass**.

That last one is load-bearing. Prompt text is a suggestion an agent can talk
itself out of; a failing exit code is not. Wherever this repo says a rule
matters, there is a `hoangsa-cli` subcommand that enforces it.

---

## The pipeline

```
brainstorm → menu → prepare → cook → taste → plate → ship
   idea      specs    plan    build   verify  commit  push
```

| Phase | Produces | Enforced by |
|-------|----------|-------------|
| **brainstorm** | `BRAINSTORM.md` — options, trade-offs, risk seeds, open questions | statused open questions |
| **menu** | `DESIGN-SPEC.md` + `TEST-SPEC.md` | `validate spec`, `validate tests` |
| **prepare** | `plan.json` — task DAG with per-task files, behaviour, acceptance | `validate plan`, `dag check` |
| **cook** | code, one atomic commit per task | per-task `acceptance`, `validate scope` |
| **taste** | verdict per task | acceptance re-run, spec-coverage review |
| **plate** | conventional commit | — |
| **ship** | push / PR after review | review gates |

Each phase is a *contract*, not a script: mission, deliverables, hard gates.
The path between them is the agent's choice; the gates are not.

### What the gates actually check

To make it concrete:

- **`validate spec`** — a code spec must carry `## Behavior / Logic` (per
  requirement: trigger, steps, error paths) and a `## Risk Sweep` covering
  eight fixed classes, including concurrency & TOCTOU. Each class is either
  *applies*, with concrete handling, or *N/A* with a reason. An open question
  with no `RESOLVED` / `DEFERRED` status fails the gate — so a question cannot
  reach the plan without having been put to you.
- **`validate plan`** — every implementation task carries a non-empty
  `behavior`, copied from the spec. The worker gets it as a contract: a step it
  drops or quietly replaces is a failure even when the tests pass.
- **`validate scope`** — a task's commit is checked against the files the plan
  gave it. Touching an undeclared file is an error, not a note.

---

## Install

```sh
curl -fsSL https://github.com/pirumu/hoangsa/releases/latest/download/install.sh | sh
```

Installs four binaries into `~/.hoangsa/bin/`, registers the memory MCP server,
and writes the Claude Code hooks. Then, inside your agent:

```
/hoangsa:init     # detect the codebase, set preferences
/hoangsa:menu     # design your first task
```

<details>
<summary><b>Platforms, flags, building from source, uninstall</b></summary>

### Supported platforms

| Triple | Status | Notes |
|--------|--------|-------|
| `darwin-arm64` | ✅ | Apple Silicon |
| `linux-x64` | ✅ | glibc distros |
| `linux-arm64` | ✅ | glibc distros |
| `linux-*` musl / Alpine | ❌ | ONNX Runtime links glibc — build from source |
| Windows | ❌ | use WSL2 |

Needs `curl` or `wget`, `tar`, and `sha256sum` / `shasum`. No Node, Python, or
Docker.

### Installer flags

Pass after `sh -s --`:

| Flag | Effect |
|------|--------|
| `--global` | install for this user (default) |
| `--local` | install for the current project only (`./.claude/`) |
| `--no-embed` | write a sticky `no-embed` marker — no model download, and no project can enable semantic retrieval |
| `--dry-run` | print actions without writing |

| Variable | Default | Purpose |
|----------|---------|---------|
| `HOANGSA_VERSION` | `latest` | release tag to install |
| `HOANGSA_INSTALL_DIR` | `~/.hoangsa` | root for binaries and cache |
| `HOANGSA_NO_PATH_EDIT` | — | `1` skips the shell rc edit |
| `CLAUDE_CONFIG_DIR` | auto-detected | pin a Claude profile (`~/.claude`, `~/.zclaude`, …) |

### From source

```sh
git clone https://github.com/pirumu/hoangsa.git && cd hoangsa
scripts/install-local.sh --global
```

Flags: `--global` / `--local`, `--dry-run`, `--skip-build`, `--no-embed`,
`--embed`. The `no-embed` marker is **sticky** — a later install without the
flag keeps it; pass `--embed` to clear it deliberately.

### PATH

The installer appends a managed block to `~/.zshrc` or `~/.bashrc`. If that was
skipped, add it yourself:

```sh
echo 'export PATH="$HOME/.hoangsa/bin:$PATH"' >> ~/.zshrc && source ~/.zshrc
```

### Update and uninstall

```sh
hoangsa-cli update --check              # current vs latest; exits 10 if newer
hoangsa-cli update                      # fetch and install it

hoangsa-cli uninstall --global --dry-run  # list what would go
hoangsa-cli uninstall --global            # keeps memory + model cache
hoangsa-cli uninstall --global --purge    # deletes ~/.hoangsa entirely
```

`scripts/uninstall.sh` does the same job from a checkout, for when the binary
is missing or will not run.

</details>

### Other harnesses

| Harness | Command | Notes |
|---------|---------|-------|
| **Claude Code** | nothing — the installer does it | |
| **Codex CLI / Desktop** | `hoangsa-cli install --global --harness codex` | then run `/hooks` once in Codex to approve them |
| **Cowork / Desktop** | `hoangsa-cli install --harness cowork` | restart the app; hooks don't apply inside the VM |
| **Plugin only** | `/plugin marketplace add unknown-studio-dev/hoangsa` | commands + agents, no binaries |

`--harness` is recorded in `.hoangsa/config.json` so model routing knows which
runtime it is resolving for. On Codex, profile tiers become *reasoning effort*
— Codex has no per-subagent model knob — and the session model is left alone.

---

## Commands

### Pipeline

| Command | Does |
|---------|------|
| `/hoangsa:brainstorm` | explore a vague idea → `BRAINSTORM.md` |
| `/hoangsa:menu` | interview → `DESIGN-SPEC.md` + `TEST-SPEC.md` |
| `/hoangsa:prepare` | specs → executable task DAG (`plan.json`) |
| `/hoangsa:cook` | execute wave by wave, fresh context per task |
| `/hoangsa:taste` | run acceptance, judge test quality, verify UI |
| `/hoangsa:qc` | spec → test cases → execute, every verdict backed by evidence |
| `/hoangsa:plate` | stage + conventional commit message |
| `/hoangsa:ship` | code + security review, then push or PR |
| `/hoangsa:fix` | hotfix — cross-layer root cause, minimal change |

### Utility

| Command | Does |
|---------|------|
| `/hoangsa:init` | detect the codebase, configure preferences |
| `/hoangsa:check` | session progress and pending tasks |
| `/hoangsa:audit` | 9-dimension codebase scan |
| `/hoangsa:research` | codebase + external research → `RESEARCH.md` |
| `/hoangsa:serve` | two-way sync with a task manager |
| `/hoangsa:rule` | project enforcement rules |
| `/hoangsa:addon` | framework-specific worker-rule addons |
| `/hoangsa:index` | rebuild the code-intelligence graph |
| `/hoangsa:update` | upgrade HOANGSA |
| `/hoangsa:help` | list everything |

---

## Memory

`hoangsa-memory` is a local MCP server giving the agent persistent memory and
code-graph awareness. Nothing leaves your machine.

**Three surfaces** — plain markdown you can read and edit:

| File | Holds |
|------|-------|
| `MEMORY.md` | project facts and invariants |
| `LESSONS.md` | action-triggered advice (`when X → do Y`) |
| `USER.md` | your cross-project workflow preferences |

**A code graph** — symbols, callers, callees, imports — behind `memory_impact`
(blast radius before you edit), `memory_symbol_context`, and
`memory_detect_changes` (did this diff touch only what it claimed?).

**Recall** fuses local sources with Reciprocal Rank Fusion — symbol lookup,
BM25, graph fan-out (depth 1 from the symbol seeds), and all three markdown
surfaces. The conversation archive is deliberately *not* in default recall;
query it explicitly with `memory_archive_search`.

### Semantic retrieval — opt-in

`[vector_store] enabled` defaults to `false`. The embedder is the heaviest
thing here: a **465 MB** model cache plus a resident ONNX session whose CPU
arena grows to 150–300 MB. Lexical, symbol, and graph retrieval cover most
queries at no idle cost, so you turn it on when you decide you want it — per
project, in `<memory root>/config.toml`:

```toml
[vector_store]
enabled = true
```

Then warm the cache once with `hoangsa-memory prefetch-embed`. A global
`~/.hoangsa/no-embed` marker overrides this to off everywhere.

ONNX's thread pool is capped at half your cores (max 4). Override with
`HOANGSA_ONNX_THREADS`; `0` restores the runtime default.

### LLM reranking — opt-in

Every stage above ranks on *form*: term overlap, identifier equality, graph
edges, rank position. None of them reads a chunk and asks whether it answers
the question, so a hit that leads on a literal string match can outrank the one
that actually explains the thing.

```toml
[rerank]
enabled = true
candidates = 24        # fused results shown to the model
timeout_secs = 20
# command = ["claude", "-p"]   # default: claude, then codex
```

It drives whatever harness CLI is already on your `PATH` — no API key. Two
guarantees, because recall sits on the hot path:

- **Fail-open** — missing binary, timeout, non-zero exit, or prose instead of
  JSON all return the fused order unchanged.
- **Reordering only** — the model cannot add, drop, or duplicate a result.
  Anything it doesn't mention keeps its fused rank at the back.

---

## Configuration

`.hoangsa/config.json`, managed by `/hoangsa:init` or `hoangsa-cli pref set`.

Top-level keys: `profile`, `harness`, `model_overrides`, `preferences`,
`codebase`, `task_manager`.

### Preferences

| Key | Values | Meaning |
|-----|--------|---------|
| `lang` / `spec_lang` | `en`, `vi` | language for output / for specs |
| `interaction_level` | `quick`, `detailed` | how much the orchestrator asks |
| `review_style` | `strict`, `balanced`, `light`, `whole_document` | review thoroughness |
| `workflow_profile` | `full`, `balanced`, `minimal` | preset for the six quality keys below |
| `quality_gate` | bool | review pass after each task |
| `simplify_pass` | bool | cleanup pass after each task |
| `test_runs` | int | how many times to repeat the suite |
| `research_mode` / `context_mode` | `full`, `inline` / `full`, `selective` | research depth / context packing |
| `memory_strict` | bool | require memory consultation before edits |
| `auto_taste` / `auto_plate` / `auto_serve` | bool | auto-chain to the next phase |

> `workflow_profile` is a **quality** preset. It is not the top-level
> `profile`, which routes models — different key, different vocabulary.

### Model routing

`profile` picks a model per role. An unknown name falls back to `balanced`.

| Role | `quality` | `balanced` | `budget` | `minimal` |
|------|-----------|------------|----------|-----------|
| researcher | opus | sonnet | haiku | haiku |
| designer | opus | opus | sonnet | sonnet |
| planner | opus | sonnet | haiku | haiku |
| orchestrator | opus | opus | haiku | sonnet |
| worker | opus | sonnet | haiku | haiku |
| reviewer | opus | sonnet | haiku | haiku |
| tester | sonnet | haiku | haiku | haiku |
| committer | sonnet | haiku | haiku | haiku |
| simplify | opus | sonnet | haiku | haiku |

`minimal` is `budget` with the orchestrator kept on sonnet — the one seat where
the cheap tier usually costs more in rework than it saves, which makes
`minimal` slightly *more* expensive than `budget` despite the name.

`fable` (Claude Fable 5, ~2× opus) belongs to no profile. Route it per role:

```json
{ "model_overrides": { "designer": "fable" } }
```

**On Codex** the tier is not a model id — Codex scales by reasoning effort, so
fable/opus → `high`, sonnet → `medium`, haiku → `low`, and the session model is
left untouched.

---

## The binaries

| Binary | Role |
|--------|------|
| `hoangsa-cli` | orchestrator — slash commands, gates, rule engine, hooks, prompt assembly |
| `hoangsa-memory` | memory + code intelligence — index, query, impact, archive |
| `hoangsa-memory-mcp` | the MCP server your agent talks to (spawned for you) |
| `hsp` | output compressor — trims cargo/npm/git/curl noise before the model reads it, 60–90% on the worst offenders. See [its README](crates/hoangsa-proxy/README.md). |

Per-project state lives in `.hoangsa/`; memory in `.hoangsa/memory/` or
`~/.hoangsa/memory/projects/<slug>/`.

---

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `command not found: hoangsa-cli` | PATH not updated in this shell — `source ~/.zshrc` or open a new terminal |
| MCP tools missing in Claude Code | `CLAUDE_CONFIG_DIR` mismatch — set it explicitly before installing |
| A `hoangsa-memory` process using a lot of CPU | it is embedding. `[vector_store] enabled = false` (the default) stops it; `HOANGSA_ONNX_THREADS` caps it. Identify it with `ps -o pid,ppid,%cpu,command -p <pid>` — a PPID of 1 means a hook spawned it detached |
| `vector_store failed to start` | delete `~/.hoangsa/cache/fastembed/`, then `hoangsa-memory prefetch-embed` |
| `validate spec` fails on an existing spec | 0.6.0 requires `## Behavior / Logic` and `## Risk Sweep` on code specs. Add them, or set `category: ops` / `content` in the frontmatter |
| `musl libc detected` on Alpine | release tarballs are glibc-only — build from source |
| Installer stalls under `curl \| sh` | stdin is piped; pass flags explicitly and set `HOANGSA_NO_PATH_EDIT=1` |

---

## Contributing

```sh
cargo test --workspace       # unit + integration
hoangsa-cli verify .         # the repo's own template/config gates
cargo clippy --workspace --all-targets
```

`verify` is the interesting one: it checks that the prompt layer and the code
still agree — that every workflow calls a CLI subcommand that exists, that the
model-profile tables in the docs match `model.rs` role by role, that the worker
skill registry in `common.md` matches its fallback copy in Rust. A
documentation table that drifts from the code fails the build.

Two house rules:

- **Do not run `cargo fmt`.** The tree is not uniformly rustfmt-formatted and
  there is a standing decision not to mass-reformat. CI format-checks *newly
  added* `.rs` files only — run `rustfmt --edition 2024` on those.
- **`plugin/` is generated** from `templates/` by `make plugin`. Edit
  `templates/`, regenerate, commit both.

---

## License

[MIT](LICENSE) — Copyright (c) 2026 Zan

**Author:** Zan — [@pirumu](https://github.com/pirumu)

---

[Tiếng Việt](README.vi.md)
