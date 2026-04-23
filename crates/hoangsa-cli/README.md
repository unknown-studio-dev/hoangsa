# hoangsa-cli

The orchestrator CLI that drives the [HOANGSA](../../README.md) context
engineering system for Claude Code. It owns install/uninstall, the rule
engine, preferences, session state, plan DAG validation, and the hook
endpoints Claude Code calls on every tool use.

`hoangsa-cli` is a pure CLI — it never generates code. Workers (Claude
subagents) do the writing; the CLI dispatches, validates, and keeps state
consistent.

---

## What it does

| Command | Purpose |
|---------|---------|
| `hoangsa-cli install --global \| --local` | First-time setup: stage templates, register MCP, seed rules, wire Claude Code hooks |
| `hoangsa-cli session init \| status \| …` | HOANGSA session lifecycle |
| `hoangsa-cli rule list \| add \| remove \| sync` | Project rule engine (CLAUDE.md guards) |
| `hoangsa-cli pref get \| set` | Project preferences (`.hoangsa/config.json`) |
| `hoangsa-cli addon list \| add \| remove` | Framework-specific worker rule addons |
| `hoangsa-cli dag check \| waves` | Validate `plan.json` DAG + compute wave schedule |
| `hoangsa-cli validate plan \| spec \| tests` | Schema + invariant checks for specs and plans |
| `hoangsa-cli hook pre-tool \| post-tool` | PreToolUse / PostToolUse hook endpoints for Claude Code |
| `hoangsa-cli statusline` | Claude Code status line renderer |

Run `hoangsa-cli --help` or `hoangsa-cli <cmd> --help` for the full list.

---

## Install

**End users:** do not install this crate directly — use the release
installer at the repo root. It bundles `hoangsa-cli` together with the
other three binaries:

```sh
curl -fsSL https://github.com/pirumu/hoangsa/releases/latest/download/install.sh | sh
```

See [the root README install section](../../README.md#installation) for
flags (`--global`, `--local`, `--no-embed`, `--dry-run`) and environment
overrides.

**Contributors** building from a checkout:

```sh
cargo install --path crates/hoangsa-cli
# or build + install everything via the helper script
scripts/install-local.sh --global
```

---

## Features

- `default` — core CLI, no optional deps.
- `media` — enables the `media` subcommand (PNG/JPEG annotation for visual
  review via `image` + `imageproc` + `ab_glyph`). Pulls heavy image deps,
  so off by default.

Enable with `cargo build --features media`.

---

## Layout

```
crates/hoangsa-cli/
├── src/
│   ├── main.rs         # flag parsing + dispatch table
│   ├── cmd/            # one module per subcommand
│   └── helpers.rs      # cwd + path resolution
└── tests/              # integration tests (budget, pref)
```

Each `src/cmd/<name>.rs` maps 1:1 to a top-level subcommand. Hooks live in
`cmd/hook.rs` and are invoked by Claude Code's PreToolUse / PostToolUse
machinery — the rule engine (`cmd/rule.rs`) decides what to allow, block,
or nudge.

---

## License

MIT OR Apache-2.0.
