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
- **Node/npm packaging removed.** The `hoangsa-cc` npm package, the six `@hoangsa/cli-*` platform packages, `bin/install` (Node), `package.json`, and `pnpm-lock.yaml` are gone. Installation is exclusively the native `curl | sh` installer that downloads pre-built binaries from GitHub Releases. Existing `npx hoangsa-cc` invocations stop working — switch to the curl one-liner in the README.
- Release workflow rewritten to native-only: one `build` matrix job per supported triple (`linux-{x64,arm64,x64-musl}`, `darwin-{x64,arm64}`) plus an `assemble-release` job that tarballs binaries + templates and uploads them to the GitHub Release. The `publish` (npm) job was deleted. Windows is no longer produced because the installer does not support it.
- `--global` install mode no longer writes to the current working directory. Previously `.mcp.json`, `.hoangsa/rules.json`, and `.thothignore` were written to `cwd` even in global mode; now they are never created by `--global`. Global MCP registration now lives in `~/.claude.json`.
- `hoangsa-memory` and `hoangsa-memory-mcp` binaries are now installed to `~/.hoangsa-memory/bin/` regardless of `--global` or `--local`.
- `--task-manager` is now a flag (was an interactive prompt only).
- `templates/workflows/update.md` rewritten to drive updates through the native installer (GitHub Releases API + `install.sh`) instead of `npm view` / `npx hoangsa-cc`.

### Fixed
- Drift bugs in the previous Node installer where `--local` tried to build memory binaries from source.
