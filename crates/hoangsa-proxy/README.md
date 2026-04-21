# hoangsa-proxy (`hsp`)

CLI output compressor for Claude Code. Wraps dev commands and trims their
output **before Claude reads it** — target is 60–90% token reduction on the
noisiest commands (cargo builds, git log, npm install, API JSON responses).

- **Single Rust binary.** No runtime, no daemon. Cold-start p95 ≈ **2ms**.
- **Lossless by opt-in.** `--strict` mode drops only whitespace, ANSI, and
  provably-redundant lines; never caps your output.
- **Exit code passthrough.** `grep` exit 1 stays 1; `cargo test` exit 101
  stays 101. Parallel tool calls never get cancelled because of us.
- **Adaptive machine-parsable report** on stderr — Claude reads it directly,
  one regex `^\[hsp( \w+)?\]` grabs every event.

---

## Install

```sh
cargo install --path crates/hoangsa-proxy
hsp init          # global hook: ~/.claude/settings.json
hsp init -p       # per-project: ./.claude/settings.local.json
hsp doctor        # verify everything is wired up
```

`hsp doctor` prints a self-check; if any line ends in `status=fail`, the
exit code is 1 and the message tells you what to fix.

---

## Quick start

Once the PreToolUse hook is installed, Claude Code's Bash tool calls are
rewritten transparently. You can also invoke `hsp` directly:

```sh
# Default mode — auto-trim with handler-specific rules
hsp git log -20
hsp cargo test
hsp curl https://api.github.com/repos/anthropics/anthropic-sdk-python

# See the untouched output (bypasses filter + color strip)
hsp run --raw cargo test

# Lossless mode — strip ANSI + dedupe only; never cap lines
hsp run --strict git log

# Trace which handler fired
hsp run --trace cargo test
```

---

## Modes

| Mode | Trim style | Use when |
|------|-----------|----------|
| **default** | Handler-specific (head/tail/sandwich/dedupe) | Normal agent work. 60–90% reduction. |
| `--strict` | Lossless only (ANSI strip, exact dedupe, JSON compact, blank collapse) | Audit-grade tasks: security grep, full commit list, schema inference. 20–40% reduction. |
| `--raw` | No filter, no strip | Command substitution (`$(hsp …)`), piping to another parser, debugging. |

`--strict` can also be set via `HSP_STRICT=1` env or `strict = true` in
`hsp.toml`.

---

## Built-in handlers

| Command | What it does |
|---------|--------------|
| `git log/diff/status/blame/show` | Head-cap log/blame; sandwich diff/show; drop `(use "git add …")` prose |
| `cargo build/check/test/clippy/run` | Drop `Compiling/Checking/Downloaded` progress; keep errors, warnings, `Finished`, test output, panics |
| `ls/cat/grep/rg/find` | Sandwich > N lines; consecutive-dup collapse on grep |
| `npm/pnpm/yarn` | Drop notices, funding, deprecation, spinner lines |
| `pip/pip3` | Drop `Requirement already satisfied`, `Collecting`, `Downloading` |
| `curl` | Compact pretty JSON responses (lossless) |

All handlers respect **user scope flags**: if the LLM wrote `--reverse`,
`-n 500`, `--max-count`, `-v`, `--verbose`, `--porcelain`, `-A`,
`--message-format`, etc., the filter steps aside and passes the child
output through unchanged. Your scope intent always wins.

---

## Config (`hsp.toml`)

Drop `.hoangsa-proxy/config.toml` into your project (or
`~/.config/hoangsa-proxy/config.toml` for a global default):

```toml
[runtime]
strict = true                     # lossless-only by default
max_output_mb = 50                # per-stream hard cap (1..=1024)

[handlers]
disabled = ["cargo", "find"]      # skip these built-ins
```

Precedence (low → high): **defaults < global config < project config <
env < CLI flags.** Broken TOML never fails the proxy — you'll see
`[hsp warn] event=config_parse_error …` on stderr and the broken layer is
skipped.

---

## Extending with Rhai

Drop a `.rhai` file into `.hoangsa-proxy/` (project) or
`~/.config/hoangsa-proxy/` (global):

```rhai
proxy::register(#{
    cmd: "git",
    subcmd: "log",
    priority: 100,
    filter: |ctx| {
        let ls = proxy::lines(ctx.stdout);
        let trimmed = proxy::head(ls, 20);
        #{ stdout: proxy::join(trimmed) }
    }
});
```

**Helpers:** `lines`, `join`, `head`, `tail`, `dedupe`, `collapse_repeats`,
`grep`, `grep_out`, `sandwich`, `summary`.

**Context fields:** `cmd`, `subcmd`, `args`, `stdout`, `stderr`, `exit`,
`cwd`, `strict`.

**Return:** `#{ stdout?, stderr?, exit? }` — any missing field falls back
to raw passthrough.

Rhai handlers beat built-ins. If a Rhai script panics or throws, we log
`[hsp] rhai runtime error …` and fall through to the built-in.

---

## The stderr report — how Claude reads us

Every proxied command can emit zero or more records on stderr, in a stable
machine-parsable form:

```
[hsp] handler=git::log before_bytes=1258291 after_bytes=204800 saved=83% exit=0 ansi_stripped=true strict=false
[hsp warn] event=soft_threshold stream=stdout threshold_bytes=10485760 raw_bytes=15728640
[hsp warn] event=hard_cap stream=stdout cap_bytes=104857600 raw_bytes=268435456
[hsp warn] event=filter_abandoned reason=output_larger_than_input
[hsp info] child_exit=101
[hsp hint] cmd='hsp run --raw git log'
```

Records are one line each, prefix + `key=value` fields. No box-drawing, no
emoji, no prose — a single regex `^\[hsp( \w+)?\]` catches them all.

Absent anything interesting (trim=0, no warn, no color strip), stderr stays
silent.

---

## Safety behaviours

| Concern | How we handle it |
|---------|------------------|
| ANSI on piped stdout (e.g. `$(hsp cat foo)` → `gh pr create --body`) | Strip automatically when stdout is not a TTY; `NO_COLOR`/`CLICOLOR=0` honoured; `--keep-color` overrides |
| Pipe/redirect corruption (`hsp grep foo x \| wc -l`) | Hook detects `\| > >> < $( \` && \|\| ; &` and skips rewrite — shell sees the raw command |
| Filter output larger than input | Abandon the filter, emit `[hsp warn] event=filter_abandoned`, passthrough raw |
| Big output (>100MB default) | Stream into a ring buffer, drop overflow, keep child draining so it doesn't block on write |
| Ctrl+C / SIGTERM mid-run | Signal forwarded to the child; no orphan processes |
| Broken Rhai / config | Warn on stderr, fall back to next layer; never block the command |

---

## Troubleshooting

```sh
hsp doctor              # full self-check
hsp list                # every registered handler + priority
hsp run --trace <cmd>   # which handler fired for this call
hsp run --raw <cmd>     # no filter, verbatim child output
```

If `hsp doctor` shows `item=hook_project installed=false`, run
`hsp init -p` to install into the current project (or `hsp init -g` for
global).

If `item=config status=warn` surfaces, run `hsp doctor | grep
config_warning` — each parse/value error is its own record.

---

## Benchmark numbers (macOS / M-series, release build)

```
hook rewrite  n=100 avg≈1.8ms p50≈1.8ms p95≈2.2ms p99≈2.6ms
run echo hi   n=100 avg≈3.5ms p50≈3.5ms p95≈4.1ms p99≈4.6ms
--version     n=100 avg≈1.8ms p50≈1.8ms p95≈2.2ms p99≈2.6ms
```

Run `./bench/bench.sh` (hyperfine) or `cargo test --release --test
bench_cold_start -- --ignored --nocapture` to reproduce on your hardware.

The sub-5ms hook rewrite is why we stayed subprocess-per-call instead of
shipping a daemon.
