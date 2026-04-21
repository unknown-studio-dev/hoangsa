# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Zero-dep `curl | sh` installer as a per-tag GitHub Release asset. See README for the one-liner.
- New Rust subcommand `hoangsa-cli install [--global|--local] [--uninstall] [--install-chroma] [--dry-run]` owning all install logic.
- CI smoke tests on alpine, ubuntu, and macOS for the install pipeline.

### Changed (BREAKING)
- `--global` install mode no longer writes to the current working directory. Previously `.mcp.json`, `.hoangsa/rules.json`, and `.thothignore` were written to `cwd` even in global mode; now they are never created by `--global`. Global MCP registration now lives in `~/.claude.json`.
- `hoangsa-memory` and `hoangsa-memory-mcp` binaries are now installed to `~/.hoangsa-memory/bin/` regardless of `--global` or `--local`.
- `bin/install` (Node) is now a thin shim that execs `hoangsa-cli install` — all install logic lives in Rust.
- `--task-manager` is now a flag (was an interactive prompt only). The interactive prompt is preserved for TTY `npx hoangsa-cc` but can be bypassed.

### Fixed
- Drift bugs in the previous Node installer where `--local` tried to build memory binaries from source.
