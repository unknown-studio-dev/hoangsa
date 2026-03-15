# HOANGSA

> A context engineering system for Claude Code — split work into bounded tasks, each with a fresh context window.

![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)
![npm version](https://img.shields.io/npm/v/hoangsa-cc.svg)
![Claude Code](https://img.shields.io/badge/Claude_Code-compatible-blueviolet.svg)
![Built with Rust](https://img.shields.io/badge/Built_with-Rust-orange.svg)
![Node.js](https://img.shields.io/badge/Node.js-18+-green.svg)

---

## What is HOANGSA?

HOANGSA is a context engineering system for [Claude Code](https://docs.anthropic.com/en/docs/claude-code). It solves a fundamental problem: **Claude's output quality degrades as the context window fills up.**

The fix is structural. HOANGSA splits work into discrete tasks. Each task runs in a fresh context window with only the files it actually needs. The result is consistent, high-quality output across arbitrarily large projects.

The core pipeline:

| Phase | Command | Output |
|-------|---------|--------|
| Design | `/hoangsa:menu` | DESIGN-SPEC + TEST-SPEC |
| Plan | `/hoangsa:prepare` | Executable task DAG (`plan.json`) |
| Execute | `/hoangsa:cook` | Working code, wave by wave |
| Test | `/hoangsa:taste` | Acceptance test results |
| Commit | `/hoangsa:plate` | Conventional commit |
| Review | `/hoangsa:ship` | Code + security review, push/PR |

The orchestrator never writes code. It dispatches workers, each with a bounded context, and assembles results.

---

## Features

**Context Engineering** — Each worker task runs in a fresh context window (200k tokens). The plan's `context_pointers` tell each worker exactly which files to read — no more, no less.

**Spec-Driven Development** — Every feature starts with a DESIGN-SPEC and TEST-SPEC. Workers implement against specs, not vague instructions. Adaptive spec format for different task types (code, ops, infra, docs).

**DAG-Based Execution** — Tasks organized as a directed acyclic graph. Independent tasks execute in parallel waves, dependent tasks execute sequentially. No unnecessary serialization.

**3-Tier Verification** — Each task goes through static analysis, behavioral tests (x3), and semantic review against spec before proceeding.

**Cross-Layer Bug Tracing** — `/hoangsa:fix` traces bugs across FE/BE/API/DB boundaries to find the real root cause before touching any code.

**Pre-Ship Review Gates** — `/hoangsa:ship` runs code quality and security reviews in parallel, blocks on critical issues, and handles push or PR creation.

**8-Dimension Codebase Audit** — `/hoangsa:audit` scans for code smells, security vulnerabilities, performance bottlenecks, tech debt, test coverage gaps, dependency risks, architectural violations, and documentation gaps.

**Task Manager Integration** — Bidirectional sync with ClickUp and Asana. Pull task details as context, push status/comments/reports back after work completes.

**GitNexus Code Intelligence** — Built-in call graph analysis. Impact analysis before edits, safe renames across the codebase, and full execution flow tracing.

**Visual Debugging** — Analyze screenshots and screen recordings. Extract frames from video, generate montages, and overlay diffs to spot visual regressions.

**Git Flow Management** — Built-in skill for task branching: start, switch, park, resume, finish, cleanup, sync. Auto-detects branching strategy and naming conventions.

**Framework-Specific Worker Rules** — 15 framework addons (React, Next.js, Vue, Svelte, Angular, Express, NestJS, Go, Rust, Python, Java, Swift, Flutter, TypeScript, JavaScript) tune worker behavior per tech stack.

**Multi-Profile Model Selection** — Switch between quality, balanced, and budget model profiles to match task requirements and cost constraints.

---

## Quick Start

```bash
npx hoangsa-cc          # Install HOANGSA into your Claude Code environment
/hoangsa:init           # Initialize project — detect codebase, set preferences
/hoangsa:menu           # Design your first task
```

After `/hoangsa:menu` completes, follow with `/hoangsa:prepare` to generate a plan, then `/hoangsa:cook` to execute it.

---

## Installation

Prerequisites: **Node.js 18+** and the **[Claude Code CLI](https://docs.anthropic.com/en/docs/claude-code)**

```bash
# Interactive — asks whether to install globally or locally
npx hoangsa-cc

# Install to ~/.claude/ — available in all projects
npx hoangsa-cc --global

# Install to .claude/ — this project only
npx hoangsa-cc --local

# Remove HOANGSA
npx hoangsa-cc --uninstall

# Install to a custom config directory
npx hoangsa-cc --config-dir <path>
```

| Flag | Short | Description |
|------|-------|-------------|
| `--global` | `-g` | Install to `~/.claude/` (all projects) |
| `--local` | `-l` | Install to `.claude/` (this project only) |
| `--uninstall` | `-u` | Remove HOANGSA |
| `--config-dir` | | Use a custom config directory path |

The installer also sets up:
- Lifecycle hooks (statusline, context monitor, update checker, GitNexus tracker)
- GitNexus MCP server for code intelligence
- Task manager MCP integration (if configured)
- Quality gate skills (silent-failure-hunter, pr-test-analyzer, comment-analyzer, type-design-analyzer)

---

## Workflow

```
idea  →  /menu      Design    →  DESIGN-SPEC + TEST-SPEC
      →  /prepare   Plan      →  Executable task DAG (plan.json)
      →  /cook      Execute   →  Wave-by-wave, fresh context per task
      →  /taste     Test      →  Acceptance tests per task
      →  /plate     Commit    →  Conventional commit message
      →  /ship      Review    →  Code + security gates, push/PR
      →  /serve     Sync      →  Bidirectional task manager sync
```

**Design (`/menu`)** — Interview the user about requirements. Produce a structured DESIGN-SPEC with interfaces and acceptance criteria, plus a TEST-SPEC with test cases and coverage targets.

**Plan (`/prepare`)** — Parse the specs and generate `plan.json`: a DAG of tasks, each with an assigned worker, bounded file list (`context_pointers`), and explicit dependency edges.

**Execute (`/cook`)** — Walk the DAG wave by wave. Dispatch each worker with its context. Independent tasks in the same wave run in parallel. Each completed task goes through an auto-simplify pass before advancing.

**Test (`/taste`)** — Run the acceptance tests defined in TEST-SPEC. Report pass/fail per task. Block the pipeline on failures, delegate fixes to `/hoangsa:fix`.

**Commit (`/plate`)** — Stage changes and generate a conventional commit message from the completed work.

**Review (`/ship`)** — Launch parallel code quality and security review agents. Block on critical/high issues. User decides: fix, override, or cancel. On pass, push and/or create PR with review summary.

**Sync (`/serve`)** — Push status updates, comments, and artifacts back to the linked task manager.

---

## Commands

### Core Workflow

| Command | Description |
|---------|-------------|
| `/hoangsa:menu` | Design — from idea to DESIGN-SPEC + TEST-SPEC |
| `/hoangsa:prepare` | Plan — convert specs to an executable task DAG |
| `/hoangsa:cook` | Execute — wave-by-wave with fresh context per task |
| `/hoangsa:taste` | Test — run acceptance tests per task |
| `/hoangsa:plate` | Commit — generate and apply a conventional commit message |
| `/hoangsa:ship` | Ship — code + security review, then push or create PR |
| `/hoangsa:serve` | Sync — bidirectional sync with connected task manager |

### Specialized

| Command | Description |
|---------|-------------|
| `/hoangsa:fix` | Hotfix — cross-layer root cause tracing + minimal targeted fix |
| `/hoangsa:audit` | Audit — 8-dimension codebase scan (security, debt, coverage, etc.) |
| `/hoangsa:research` | Research — codebase analysis combined with external research |

### Utility

| Command | Description |
|---------|-------------|
| `/hoangsa:init` | Initialize — detect codebase, configure preferences, first-time setup |
| `/hoangsa:check` | Status — show current session progress and pending tasks |
| `/hoangsa:index` | Index — rebuild GitNexus code intelligence graph |
| `/hoangsa:update` | Update — upgrade HOANGSA to the latest version |
| `/hoangsa:help` | Help — show all available commands |

---

## Skills

HOANGSA includes built-in skills that extend Claude Code's capabilities:

### Git Flow

Task-oriented git workflow management. Start a task branch, park work-in-progress, switch between tasks, and finish with push + PR — all with dirty-state guards and auto-detection of your branching strategy.

Flows: `start` | `switch` | `park` | `resume` | `finish` | `cleanup` | `sync`

### Visual Debug

Analyze screenshots and screen recordings to debug visual issues. Extracts frames from video files, generates montage grids for overview, and creates diff overlays to highlight changes between frames.

Supports: `.png`, `.jpg`, `.webp`, `.gif`, `.mp4`, `.mov`, `.webm`, `.avi`, `.mkv`

---

## Configuration

HOANGSA stores project configuration in `.hoangsa/config.json`.

```json
{
  "lang": "en",
  "spec_lang": "en",
  "tech_stack": ["typescript", "react", "postgres"],
  "review_style": "strict",
  "model_profile": "balanced",
  "task_manager": {
    "provider": "clickup",
    "token": "<your-token>"
  }
}
```

### Preferences

| Key | Values | Description |
|-----|--------|-------------|
| `lang` | `en`, `vi` | Language for orchestrator output |
| `spec_lang` | `en`, `vi` | Language for generated specs |
| `tech_stack` | array | Project technology stack (used to select worker rule addons) |
| `review_style` | `strict`, `balanced`, `light` | Code review thoroughness |
| `interaction_level` | `minimal`, `standard`, `detailed` | How much the orchestrator asks |

### Model Profiles

Select a profile to control the model used at each role:

| Profile | Worker | Designer | Reviewer |
|---------|--------|----------|----------|
| `quality` | claude-opus | claude-opus | claude-opus |
| `balanced` | claude-sonnet | claude-opus | claude-sonnet |
| `budget` | claude-haiku | claude-sonnet | claude-haiku |

Switch profiles with `/hoangsa:init` or by editing `model_profile` in `config.json`.

### Task Manager Integration

| Provider | How to connect |
|----------|---------------|
| ClickUp | Paste a ClickUp task URL |
| Asana | Paste an Asana task URL |

HOANGSA fetches task details as additional context and writes results back on `/hoangsa:serve`.

---

## Architecture

### Project Structure

```
hoangsa/
├── cli/                        # Rust CLI (hoangsa-cli)
│   └── src/
│       ├── cmd/                # Command modules
│       │   ├── commit.rs       # Atomic commit
│       │   ├── config.rs       # Config read/write
│       │   ├── context.rs      # Context pointer resolution
│       │   ├── dag.rs          # DAG traversal and wave scheduling
│       │   ├── hook.rs         # Lifecycle hooks (statusline, context-monitor, tracker)
│       │   ├── media.rs        # Video/image probing, frame extraction, montage
│       │   ├── memory.rs       # Session memory
│       │   ├── model.rs        # Model profile & role resolution
│       │   ├── pref.rs         # User preferences
│       │   ├── session.rs      # Session create/resume/list
│       │   ├── state.rs        # Task state machine
│       │   ├── validate.rs     # Plan/spec validation
│       │   └── verify.rs       # Installation verification
│       ├── helpers.rs          # Shared utilities
│       └── main.rs
├── templates/
│   ├── commands/hoangsa/       # 15 slash command definitions
│   ├── workflows/              # Workflow implementations
│   │   ├── menu.md             # Design workflow
│   │   ├── prepare.md          # Planning workflow
│   │   ├── cook.md             # Execution workflow
│   │   ├── taste.md            # Test workflow
│   │   ├── plate.md            # Commit workflow
│   │   ├── ship.md             # Review & ship workflow
│   │   ├── fix.md              # Hotfix workflow
│   │   ├── audit.md            # Audit workflow
│   │   ├── research.md         # Research workflow
│   │   ├── serve.md            # Task manager sync
│   │   ├── init.md             # Project setup
│   │   ├── update.md           # Update workflow
│   │   ├── git-context.md      # Shared: git state detection
│   │   ├── task-link.md        # Shared: task URL parsing
│   │   └── worker-rules/       # Worker behavior rules
│   │       ├── base.md         # Common patterns
│   │       └── addons/         # 15 framework-specific addons
│   └── skills/                 # Skill definitions
│       └── hoangsa/
│           ├── git-flow/       # Git workflow management
│           └── visual-debug/   # Screenshot & video analysis
├── bin/
│   └── install                 # Node.js installer script
├── npm/                        # Platform-specific binary packages
│   ├── cli-darwin-arm64/
│   ├── cli-darwin-x64/
│   ├── cli-linux-arm64/
│   ├── cli-linux-x64/
│   ├── cli-linux-x64-musl/
│   └── cli-windows-x64/
├── package.json
└── .hoangsa/                   # Project-local config and sessions
    ├── config.json
    └── sessions/               # Session artifacts (plan.json, specs, logs)
```

### Tech Stack

| Layer | Technology | Purpose |
|-------|-----------|---------|
| CLI | Rust | Session management, DAG traversal, state machine, validation, media analysis, hooks |
| Installer | Node.js | Package distribution, slash command registration, hook setup |
| Code Intelligence | GitNexus MCP | Call graph, impact analysis, safe rename, execution flow tracing |
| AI Runtime | Claude Code | Orchestrator + worker execution |

### Hooks

HOANGSA installs lifecycle hooks into Claude Code:

| Hook | Event | Purpose |
|------|-------|---------|
| Statusline | `SessionStart` | Display session info, token usage, project context |
| Context Monitor | `PostToolUse` | Track context window usage, warn on high utilization |
| GitNexus Tracker | `PostToolUse` | Track file modifications for index freshness |
| Update Checker | `SessionStart` | Notify when a new HOANGSA version is available |

### Worker Rules & Framework Addons

Workers receive framework-specific guidance based on your `tech_stack` configuration. Available addons:

Angular, Express.js, Flutter, Go, Java, JavaScript, NestJS, Next.js, Python, React, Rust, Svelte, Swift, TypeScript, Vue

### How to Contribute

1. Fork the repository at https://github.com/pirumu/hoangsa
2. Run `npm run build` to compile the Rust CLI (`cargo build --release` inside `cli/`)
3. Run `npm test` to verify the installation
4. Slash command definitions live in `templates/commands/hoangsa/` — each is a Markdown file with YAML frontmatter
5. Workflow logic lives in `templates/workflows/` — plain Markdown instructions for the AI
6. Worker rule addons live in `templates/workflows/worker-rules/addons/`

---

## Supported Integrations

### Task Managers

- ClickUp
- Asana

### Code Intelligence

- GitNexus MCP (call graphs, impact analysis, execution flow tracing, safe rename)

### Quality Gate Skills

Optionally installed during setup:

- **silent-failure-hunter** — Identifies swallowed errors and inadequate error handling
- **pr-test-analyzer** — Analyzes test coverage quality and completeness
- **comment-analyzer** — Checks comment accuracy and documentation gaps
- **type-design-analyzer** — Reviews type design for encapsulation and invariants

### Language & Framework Support

HOANGSA is language-agnostic. The worker-rules system has addons for:

- JavaScript / TypeScript (React, Next.js, Vue, Svelte, Angular, Express, NestJS)
- Rust
- Python (FastAPI, Django)
- Go
- Java / Kotlin (Spring)
- Swift / Flutter
- And more via the base rules

---

## License

[MIT](LICENSE) — Copyright (c) 2026 Zan

---

## Author

**Zan** — [@pirumu](https://github.com/pirumu)

---

[Tiếng Việt](README.vi.md)
