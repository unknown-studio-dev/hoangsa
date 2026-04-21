# HOANGSA

> A context engineering system for Claude Code

![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)
![Claude Code](https://img.shields.io/badge/Claude_Code-compatible-blueviolet.svg)
![Built with Rust](https://img.shields.io/badge/Built_with-Rust-orange.svg)

---

HOANGSA is a context engineering system for [Claude Code](https://docs.anthropic.com/en/docs/claude-code) that solves a fundamental problem: Claude's output quality degrades as the context window fills up. The fix is structural — HOANGSA splits work into discrete tasks, each running in a fresh context window with only the files it actually needs. The orchestrator never writes code; it dispatches workers with bounded context and assembles results.

---

## Install

```sh
curl -fsSL https://github.com/pirumu/hoangsa/releases/latest/download/install.sh | sh
```

The installer ships pre-built native binaries — no Node, no `npm`, no `cargo` required.

- `hoangsa-cli` → `~/.hoangsa/bin/`
- `hoangsa-memory`, `hoangsa-memory-mcp` → `~/.hoangsa-memory/bin/`

Supported platforms: `darwin-arm64`, `darwin-x64`, `linux-x64`, `linux-arm64`, `linux-x64-musl`.

Flags (forwarded to `hoangsa-cli install`): `--global` (default), `--local`, `--uninstall`, `--install-chroma`, `--dry-run`.

---

## Quick Start

Prerequisites: the **[Claude Code CLI](https://docs.anthropic.com/en/docs/claude-code)**.

```bash
curl -fsSL https://github.com/pirumu/hoangsa/releases/latest/download/install.sh | sh
/hoangsa:init        # Initialize project — detect codebase, set preferences
/hoangsa:menu        # Design your first task → DESIGN-SPEC + TEST-SPEC
```

After `/hoangsa:menu`, run `/hoangsa:prepare` to plan, then `/hoangsa:cook` to execute.

---

## Commands

### Core Workflow

| Command | Description |
|---------|-------------|
| `/hoangsa:brainstorm` | Explore a vague idea → BRAINSTORM.md (feeds into menu) |
| `/hoangsa:menu` | Design — interview → DESIGN-SPEC + TEST-SPEC |
| `/hoangsa:prepare` | Plan — specs → executable task DAG (`plan.json`) |
| `/hoangsa:cook` | Execute — wave-by-wave, fresh context per worker task |
| `/hoangsa:taste` | Test — run acceptance tests per task |
| `/hoangsa:plate` | Commit — stage + generate conventional commit message |
| `/hoangsa:ship` | Ship — code + security review, then push or create PR |
| `/hoangsa:serve` | Sync — bidirectional sync with connected task manager |
| `/hoangsa:fix` | Hotfix — cross-layer root cause tracing + minimal fix |
| `/hoangsa:audit` | Audit — 8-dimension codebase scan (security, debt, coverage…) |
| `/hoangsa:research` | Research — codebase analysis + external research → RESEARCH.md |

### Utility

| Command | Description |
|---------|-------------|
| `/hoangsa:rule` | Rules — add, remove, or list project enforcement rules |
| `/hoangsa:addon` | Addons — list, add, or remove framework-specific worker rule addons |
| `/hoangsa:init` | Initialize — detect codebase, configure preferences, first-time setup |
| `/hoangsa:check` | Status — show current session progress and pending tasks |
| `/hoangsa:index` | Index — rebuild hoangsa-memory code intelligence graph |
| `/hoangsa:update` | Update — upgrade HOANGSA to the latest version |
| `/hoangsa:help` | Help — show all available commands |

---

## Memory & Code Intelligence

HOANGSA ships with **hoangsa-memory**, a local MCP server that gives Claude persistent memory (facts, lessons, preferences) and code-graph awareness (impact analysis, symbol context, change detection) across sessions.

- **Auto-installed** by the installer: binaries land in `~/.hoangsa-memory/bin/` and the MCP server is registered in your project's `.mcp.json`.
- **State** per project lives under `~/.hoangsa-memory/projects/<slug>/` (MEMORY.md, LESSONS.md, USER.md + index).
- **Hooks** installed into Claude Code settings: pre-edit rule enforcement, pre-edit lesson recall, post-tool event logging, and PreCompact / SessionEnd archive ingest for conversation recall.
- **Archive search** (full conversation history) needs a chroma sidecar — optional, provision with `hoangsa-cli install --install-chroma`.

Manual reindex: `/hoangsa:index` or `~/.hoangsa-memory/bin/hoangsa-memory --json index .`

---

## Configuration

Config lives in `.hoangsa/config.json`. Manage preferences with `/hoangsa:init` or `hoangsa-cli pref set`.

### Preferences

| Key | Values | Description |
|-----|--------|-------------|
| `lang` | `en`, `vi` | Language for output |
| `spec_lang` | `en`, `vi` | Language for generated specs |
| `tech_stack` | array | Project technology stack |
| `review_style` | `strict`, `balanced`, `light`, `whole_document` | Code review thoroughness |
| `interaction_level` | `minimal`, `quick`, `standard`, `detailed` | How much the orchestrator asks |
| `auto_taste` | `true`, `false` | Auto-run tests after cook |
| `auto_plate` | `true`, `false` | Auto-commit after cook |
| `auto_serve` | `true`, `false` | Auto-sync to task manager |

### Model Profiles

Select a profile (`quality` / `balanced` / `budget`) to control the model at each of 8 roles. Switch with `/hoangsa:init` or by editing `profile` in `config.json`.

| Role | `quality` | `balanced` | `budget` |
|------|-----------|------------|----------|
| researcher | opus | sonnet | haiku |
| designer | opus | opus | sonnet |
| planner | opus | sonnet | haiku |
| orchestrator | opus | opus | haiku |
| worker | opus | sonnet | haiku |
| reviewer | opus | sonnet | haiku |
| tester | sonnet | haiku | haiku |
| committer | sonnet | haiku | haiku |

---

## License

[MIT](LICENSE) — Copyright (c) 2026 Zan

**Author:** Zan — [@pirumu](https://github.com/pirumu)

---

[Tiếng Việt](README.vi.md)
