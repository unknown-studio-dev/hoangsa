# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Zero-dep `curl | sh` installer as a per-tag GitHub Release asset. See README for the one-liner.
- New Rust subcommand `hoangsa-cli install [--global|--local] [--install-chroma] [--dry-run]` owning all install logic.
- CI smoke tests on alpine, ubuntu, and macOS for the install pipeline.
- `scripts/uninstall.sh [--global|--local] [--dry-run] [--purge]` — standalone POSIX-sh uninstaller that removes binaries, manifest-tracked templates, managed hook entries, the `hoangsa-memory` MCP registration, and the managed PATH block.

### Removed
- `--uninstall` flag on `hoangsa-cli install` (was a stub returning exit 4). Use `scripts/uninstall.sh` instead.

### Changed (BREAKING)
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
