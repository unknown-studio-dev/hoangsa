# HOANGSA

> A context engineering system for Claude Code

![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)
![Claude Code](https://img.shields.io/badge/Claude_Code-compatible-blueviolet.svg)
![Built with Rust](https://img.shields.io/badge/Built_with-Rust-orange.svg)

---

HOANGSA is a context engineering system for [Claude Code](https://docs.anthropic.com/en/docs/claude-code) that solves a fundamental problem: Claude's output quality degrades as the context window fills up. The fix is structural — HOANGSA splits work into discrete tasks, each running in a fresh context window with only the files it actually needs. The orchestrator never writes code; it dispatches workers with bounded context and assembles results.

---

## Installation

HOANGSA ships four binaries: three CLIs you invoke directly
(`hoangsa-cli`, `hoangsa-memory`, `hsp`) plus one MCP server
(`hoangsa-memory-mcp`) that Claude Code spawns on your behalf.

### Supported platforms

| Triple | Status | Notes |
|--------|--------|-------|
| `darwin-arm64` | ✅ Supported | Apple Silicon (M1 / M2 / M3 / M4) |
| `linux-x64` | ✅ Supported | glibc-based distros (Ubuntu, Debian, Fedora, RHEL, …) |
| `linux-arm64` | ✅ Supported | glibc-based distros |
| `linux-*` (musl / Alpine) | ❌ Not supported | ONNX Runtime binaries link glibc — build from source |
| Windows | ❌ Not yet | Use WSL2 (Ubuntu) |

Prerequisites on the target machine: `curl` or `wget`, `tar`, and one of
`sha256sum` / `shasum`. No Node, no Python, no Docker, no `cargo` needed
for the release tarball.

For contributors building from source you additionally need **Rust 1.91+**
(`rustup toolchain install stable`) and a C toolchain (`build-essential`
on Debian/Ubuntu, Xcode Command Line Tools on macOS).

---

### Option A — release installer (recommended)

One command pulls the latest release, verifies the SHA-256 checksum,
drops all four binaries into `~/.hoangsa/bin/`, registers the
`hoangsa-memory` MCP server in Claude Code, and pre-downloads the
fastembed ONNX weights.

```sh
curl -fsSL https://github.com/pirumu/hoangsa/releases/latest/download/install.sh | sh
```

**Flags** (pass after `sh -s --`):

| Flag | Effect |
|------|--------|
| `--global` | Install globally for this user (default) — writes to the resolved Claude config dir |
| `--local` | Install for the current project only — writes to `./.claude/` |
| `--no-embed` | Skip pre-downloading the `multilingual-e5-small` weights (~118 MB). They will fetch lazily on first `index` / `query` / `archive ingest`. Useful on bandwidth-constrained links. |
| `--dry-run` | Print actions without writing files — good for auditing |
| `--help` | Show the installer help |

**Examples:**

```sh
# Global install (default)
curl -fsSL https://github.com/pirumu/hoangsa/releases/latest/download/install.sh | sh

# Project-local install
curl -fsSL https://github.com/pirumu/hoangsa/releases/latest/download/install.sh | sh -s -- --local

# Dry-run to see what would happen
curl -fsSL https://github.com/pirumu/hoangsa/releases/latest/download/install.sh | sh -s -- --dry-run

# Pin a specific version
HOANGSA_VERSION=v0.2.2 curl -fsSL https://github.com/pirumu/hoangsa/releases/download/v0.2.2/install.sh | sh
```

**Environment overrides:**

| Variable | Default | Purpose |
|----------|---------|---------|
| `HOANGSA_VERSION` | `latest` | Release tag to install (e.g. `v0.2.2`) |
| `HOANGSA_REPO` | `pirumu/hoangsa` | GitHub repo slug to download from |
| `HOANGSA_INSTALL_DIR` | `$HOME/.hoangsa` | Root for all binaries and cache |
| `HOANGSA_CLI_DIR` | `$HOANGSA_INSTALL_DIR/bin` | Override just the CLI bin dir |
| `HOANGSA_NO_PATH_EDIT` | — | Set to `1` to skip `~/.zshrc` / `~/.bashrc` edits (export PATH manually) |
| `CLAUDE_CONFIG_DIR` | auto-detected | Pin a specific Claude profile (`~/.claude`, `~/.zclaude`, …) |

The installer honours `CLAUDE_CONFIG_DIR` if it is already set, and
otherwise detects multiple Claude profile dirs (`~/.claude`,
`~/.zclaude`, …). With more than one it prompts; pass the env var
explicitly in non-TTY installs.

---

### Option B — build from source (contributors)

Clone the repo and run the local install helper — builds the workspace
in release mode, installs the four binaries, and wires Claude Code the
same way the release installer does:

```sh
git clone https://github.com/pirumu/hoangsa.git
cd hoangsa
scripts/install-local.sh --global     # or --local for per-project
```

Flags: `--global` / `--local`, `--dry-run`, `--no-embed`, `--skip-build`
(re-run the post-build steps without recompiling).

**Just one CLI?** Each binary can be installed standalone via `cargo`:

```sh
cargo install --path crates/hoangsa-cli       # installs `hoangsa-cli`
cargo install --path crates/hoangsa-memory    # installs `hoangsa-memory`
cargo install --path crates/hoangsa-memory-mcp # installs `hoangsa-memory-mcp`
cargo install --path crates/hoangsa-proxy     # installs `hsp`
```

This drops binaries into `~/.cargo/bin/`. Note that `cargo install`
alone **does not** register MCP servers, copy templates, or wire Claude
Code hooks — run `hoangsa-cli install --global` afterwards to finish
setup.

---

### Post-install: PATH and verification

The installer appends a managed block to the first existing rc file it
finds (`~/.zshrc` → `~/.bashrc`) containing:

```sh
# hoangsa:managed start
export PATH="$HOME/.hoangsa/bin:$PATH"
# hoangsa:managed end
```

**If the block was skipped** (non-TTY install, declined prompt,
`HOANGSA_NO_PATH_EDIT=1`, no rc file found), add it yourself:

```sh
echo 'export PATH="$HOME/.hoangsa/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

**Verify the install:**

```sh
hoangsa-cli --version          # e.g. hoangsa-cli 0.2.2
hoangsa-memory --version       # e.g. hoangsa-memory 0.2.2
hoangsa-memory-mcp --version   # e.g. hoangsa-memory-mcp 0.2.2
hsp --version                  # e.g. hsp 0.2.2
```

Then run the per-CLI self-checks:

```sh
hsp doctor                     # verifies hsp hooks + handlers
hoangsa-memory memory show     # prints MEMORY.md + LESSONS.md for cwd
```

---

### Per-CLI install notes

#### `hoangsa-cli` — HOANGSA orchestrator

Drives the `/hoangsa:*` slash commands, owns the rule engine, wires
Claude Code hooks, manages project preferences.

- **Binary location:** `~/.hoangsa/bin/hoangsa-cli`
- **Per-project config:** `.hoangsa/config.json` (created by
  `hoangsa-cli install --local` or `/hoangsa:init`)
- **Templates:** staged to `~/.hoangsa/templates/` on global install
- **First-run setup:** `/hoangsa:init` from inside Claude Code, or
  `hoangsa-cli install --local` from the shell

No separate install step — bundled with the release installer.

#### `hoangsa-memory` — long-term memory + code intelligence

Indexes your source tree, serves recall queries, runs blast-radius
analysis, and manages the verbatim conversation archive.

- **Binary:** `~/.hoangsa/bin/hoangsa-memory`
- **Companion daemon:** `~/.hoangsa/bin/hoangsa-memory-mcp` (spawned by
  Claude Code via MCP — you never start it manually)
- **Per-project data:** `.hoangsa/memory/` (or `~/.hoangsa/memory/projects/<slug>/`)
- **Shared model cache:** `~/.hoangsa/cache/fastembed/`
  (~4xx MB on disk once warm)
- **First run:** `hoangsa-memory init && hoangsa-memory index .`

If you passed `--no-embed` and want to prefetch weights later:

```sh
hoangsa-memory prefetch-embed
```

#### `hsp` — CLI output compressor

Wraps Claude Code's Bash tool calls and trims verbose output
(cargo/npm/git log/curl JSON) before the model reads it. 60–90% token
savings on the noisiest commands.

- **Binary:** `~/.hoangsa/bin/hsp`
- **Global hook:** `hsp init` — writes PreToolUse hook into
  `~/.claude/settings.json`
- **Per-project hook:** `hsp init -p` — writes into
  `./.claude/settings.local.json`
- **Self-check:** `hsp doctor`

Full reference: [`crates/hoangsa-proxy/README.md`](crates/hoangsa-proxy/README.md).

---

### Uninstall

From a checkout of the repo:

```sh
scripts/uninstall.sh --global          # remove global install
scripts/uninstall.sh --local           # remove project-local install
scripts/uninstall.sh --global --purge  # also delete ~/.hoangsa entirely
```

Without `--purge`, the uninstaller leaves your memory data, fastembed
cache, and staged templates under `~/.hoangsa/` in place — so you can
reinstall without re-indexing or re-downloading model weights.

To remove a single `cargo install`-ed binary:

```sh
cargo uninstall hoangsa-cli
cargo uninstall hoangsa-memory
cargo uninstall hoangsa-memory-mcp
cargo uninstall hoangsa-proxy       # binary name is `hsp`
```

---

### Troubleshooting

| Symptom | Fix |
|---------|-----|
| `command not found: hoangsa-cli` after install | PATH not updated in current shell. `source ~/.zshrc` or open a new terminal. |
| `musl libc detected` on Alpine | Release tarballs are glibc-only. Use `scripts/install-local.sh` from a checkout on Alpine — requires `rustc` + `cargo`. |
| MCP tools missing in Claude Code | `CLAUDE_CONFIG_DIR` mismatch. Set it explicitly before install: `CLAUDE_CONFIG_DIR=~/.zclaude curl -fsSL …\| sh`. |
| `vector_store failed to start` on first `index` | fastembed weights missing or corrupted. Run `hoangsa-memory prefetch-embed` or delete `~/.hoangsa/cache/fastembed/` and retry. |
| Installer stalls on prompt under `curl\|sh` | stdin is piped. Pass flags explicitly: `sh -s -- --global` and set `HOANGSA_NO_PATH_EDIT=1`. |
| `GitHub API rate limit exceeded` | Pin a tag: `HOANGSA_VERSION=v0.2.2 curl …` — skips the `/releases/latest` API call. |

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

- **Auto-installed** by the installer: binaries land in `~/.hoangsa/bin/` and the MCP server is registered in your project's `.mcp.json`.
- **State** per project lives under `~/.hoangsa/memory/projects/<slug>/` (MEMORY.md, LESSONS.md, USER.md + index).
- **Hooks** installed into Claude Code settings: pre-edit rule enforcement, pre-edit lesson recall, post-tool event logging, and PreCompact / SessionEnd archive ingest for conversation recall.
- **Archive search** (full conversation history) uses the in-process fastembed vector store — no sidecar required. The installer pre-downloads the `multilingual-e5-small` weights (~4xx MB) into `~/.hoangsa/cache/fastembed/`; pass `--no-embed` to skip and fetch lazily on first use.

Manual reindex: `/hoangsa:index` or `~/.hoangsa/bin/hoangsa-memory --json index .`

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
