#!/bin/sh
# hoangsa local installer — build from source, install all four binaries
# into `~/.hoangsa/bin/`, then run `hoangsa-cli install` with
# HOANGSA_TEMPLATES_DIR pointing at the repo's templates/ tree so the CLI
# copies the same template set that the release tarball would.
#
# To uninstall, use scripts/uninstall.sh.
#
# Usage:
#   scripts/install-local.sh [--global|--local] [--dry-run]
#                            [--skip-build] [-- extra args forwarded to CLI]
#
# Environment variables:
#   HOANGSA_INSTALL_DIR  Install root for all binaries (default: $HOME/.hoangsa)
#   HOANGSA_CLI_DIR      Install root for hoangsa-cli / hsp (default: $HOANGSA_INSTALL_DIR/bin)
#   HOANGSA_NO_PATH_EDIT If "1", skip shell rc file edit (manual export only)
#   CARGO_PROFILE        release|debug (default: release)

set -eu

REPO_ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$REPO_ROOT"

# Source shared UI lib (info/warn/err/section/render_summary/…).
. "$REPO_ROOT/scripts/lib/ui.sh"

CARGO_PROFILE="${CARGO_PROFILE:-release}"
HOANGSA_INSTALL_DIR="${HOANGSA_INSTALL_DIR:-$HOME/.hoangsa}"
HOANGSA_CLI_DIR="${HOANGSA_CLI_DIR:-$HOANGSA_INSTALL_DIR/bin}"
HOANGSA_NO_PATH_EDIT="${HOANGSA_NO_PATH_EDIT:-}"
SKIP_BUILD=0
DRY_RUN=0
IS_GLOBAL=0
PASSTHROUGH=""
HAS_MODE_FLAG=0
SKIP_EMBED=0

append_arg() {
    quoted=$(printf "%s" "$1" | sed "s/'/'\\\\''/g")
    if [ -z "$PASSTHROUGH" ]; then
        PASSTHROUGH="'$quoted'"
    else
        PASSTHROUGH="$PASSTHROUGH '$quoted'"
    fi
}

for arg in "$@"; do
    case "$arg" in
        --skip-build) SKIP_BUILD=1 ;;
        --dry-run)    DRY_RUN=1;    append_arg "$arg" ;;
        --global) IS_GLOBAL=1; HAS_MODE_FLAG=1; append_arg "$arg" ;;
        --local)  HAS_MODE_FLAG=1; append_arg "$arg" ;;
        --no-embed) SKIP_EMBED=1 ;;
        -h|--help)
            sed -n '2,15p' "$0"
            exit 0
            ;;
        *) append_arg "$arg" ;;
    esac
done

if [ "$HAS_MODE_FLAG" -eq 0 ]; then
    PASSTHROUGH="'--local' $PASSTHROUGH"
fi

# info / warn / err are provided by lib/ui.sh; keep die() local because
# install-local.sh uses the `die "msg"` single-arg signature.
die()  { err "$*"; exit 1; }

# --- Claude config dir picker -----------------------------------------------
#
# Claude Code honors `CLAUDE_CONFIG_DIR` so users can run alternate profiles
# via shell aliases (e.g. `zclaude='CLAUDE_CONFIG_DIR=~/.zclaude claude'`).
# Without the same awareness in the installer, `--global` writes land in
# `~/.claude/` but a zclaude session reads from `~/.zclaude/`, making the
# hoangsa skills and `hoangsa-memory` MCP invisible inside that session.
#
# Strategy:
#   * If `CLAUDE_CONFIG_DIR` is already set, honor it silently — the user
#     opted in explicitly and we must not second-guess.
#   * Else glob `$HOME/.claude*` for dirs that look like Claude config dirs
#     (contain settings.json / projects/ / history.jsonl). Only the default
#     `~/.claude` → no prompt. Two or more → interactive menu with a
#     "custom path" escape hatch. Non-TTY defaults to the first candidate
#     and logs a hint for how to override.

# Heuristic: directory `$1` looks like a Claude Code config dir.
is_claude_config_dir() {
    [ -d "$1" ] || return 1
    [ -f "$1/settings.json" ] \
        || [ -d "$1/projects" ] \
        || [ -f "$1/history.jsonl" ] \
        || [ -f "$1/.claude.json" ]
}

# Populate $CLAUDE_CANDIDATES (newline-separated) with every discovered dir.
# Always includes `$HOME/.claude` at the head so the default is available
# even when the user has never launched Claude Code before.
detect_claude_candidates() {
    CLAUDE_CANDIDATES="$HOME/.claude"
    # Glob `.*claude*` (not just `.claude*`) so prefix-style alternate
    # profiles like `.zclaude` — used by `zclaude='CLAUDE_CONFIG_DIR=~/.zclaude claude'` —
    # are visible. The `is_claude_config_dir` filter gates out unrelated
    # dotdirs that happen to contain "claude" in their name.
    for d in "$HOME"/.*claude*; do
        [ -d "$d" ] || continue
        [ "$d" = "$HOME/.claude" ] && continue
        if is_claude_config_dir "$d"; then
            CLAUDE_CANDIDATES="$CLAUDE_CANDIDATES
$d"
        fi
    done
}

# Prompt the user to pick a candidate; sets $CLAUDE_DIR_PICK. Must only be
# called when $IS_GLOBAL=1.
pick_claude_dir() {
    # Caller-provided env wins — even over $HOME/.claude fallback. This is how
    # zclaude-style aliases propagate their profile into the installer.
    if [ -n "${CLAUDE_CONFIG_DIR:-}" ]; then
        CLAUDE_DIR_PICK="$CLAUDE_CONFIG_DIR"
        info "honoring CLAUDE_CONFIG_DIR=$CLAUDE_DIR_PICK (inherited from env)"
        return 0
    fi

    detect_claude_candidates
    _count=$(printf '%s\n' "$CLAUDE_CANDIDATES" | wc -l | tr -d ' ')

    if [ "$_count" -le 1 ]; then
        CLAUDE_DIR_PICK="$CLAUDE_CANDIDATES"
        return 0
    fi

    if [ ! -t 0 ] || [ ! -t 1 ]; then
        CLAUDE_DIR_PICK=$(printf '%s\n' "$CLAUDE_CANDIDATES" | head -n 1)
        info "multiple Claude config dirs detected but non-interactive — defaulting to $CLAUDE_DIR_PICK"
        info "override: CLAUDE_CONFIG_DIR=<dir> ./scripts/install-local.sh --global"
        return 0
    fi

    printf '\nMultiple Claude config dirs detected:\n' >&2
    _i=1
    while [ "$_i" -le "$_count" ]; do
        _d=$(printf '%s\n' "$CLAUDE_CANDIDATES" | sed -n "${_i}p")
        printf '  %d) %s\n' "$_i" "$_d" >&2
        _i=$((_i + 1))
    done
    _custom_idx=$((_count + 1))
    printf '  %d) custom path\n' "$_custom_idx" >&2
    printf 'Pick [1]: ' >&2
    _pick=""
    # shellcheck disable=SC2039  # `read -r` is POSIX
    read -r _pick || _pick=""
    [ -z "$_pick" ] && _pick=1

    if [ "$_pick" = "$_custom_idx" ]; then
        printf 'Enter path: ' >&2
        read -r CLAUDE_DIR_PICK || CLAUDE_DIR_PICK=""
        [ -n "$CLAUDE_DIR_PICK" ] || die "empty path"
        # Tilde-expand a leading `~/` — shells do this for literal tokens,
        # but `read` captures the raw string unchanged. SC2088 is a false
        # positive here: we're pattern-matching the *input* string, not
        # relying on the shell to expand `~` inside a quoted path.
        # shellcheck disable=SC2088
        case "$CLAUDE_DIR_PICK" in
            "~/"*) CLAUDE_DIR_PICK="$HOME/${CLAUDE_DIR_PICK#\~/}" ;;
            "~")   CLAUDE_DIR_PICK="$HOME" ;;
        esac
    else
        case "$_pick" in
            ''|*[!0-9]*) die "invalid selection: $_pick" ;;
        esac
        if [ "$_pick" -lt 1 ] || [ "$_pick" -gt "$_count" ]; then
            die "selection out of range: $_pick"
        fi
        CLAUDE_DIR_PICK=$(printf '%s\n' "$CLAUDE_CANDIDATES" | sed -n "${_pick}p")
    fi
    info "using Claude config dir: $CLAUDE_DIR_PICK"
}

# --- PATH rc-file edit ($SHELL-detected; matches install.sh marker contract) -
#
# Managed-block markers MUST match scripts/install.sh (and scripts/uninstall.sh)
# so a later uninstall or re-install can strip-and-rewrite the same block.

HOANGSA_MARK_START='# hoangsa:managed start'
HOANGSA_MARK_END='# hoangsa:managed end'

managed_export_line_posix() {
    if [ "$HOANGSA_INSTALL_DIR/bin" = "$HOANGSA_CLI_DIR" ]; then
        # shellcheck disable=SC2016
        printf 'export PATH="%s:$PATH"\n' "$HOANGSA_CLI_DIR"
    else
        # shellcheck disable=SC2016
        printf 'export PATH="%s/bin:%s:$PATH"\n' \
            "$HOANGSA_INSTALL_DIR" "$HOANGSA_CLI_DIR"
    fi
}

managed_export_line_fish() {
    # fish expands $PATH as a list; same semantics as POSIX colon-prepend.
    # `$PATH` stays literal so fish (not this shell) expands it at source time.
    if [ "$HOANGSA_INSTALL_DIR/bin" = "$HOANGSA_CLI_DIR" ]; then
        # shellcheck disable=SC2016
        printf 'set -gx PATH %s $PATH\n' "$HOANGSA_CLI_DIR"
    else
        # shellcheck disable=SC2016
        printf 'set -gx PATH %s/bin %s $PATH\n' \
            "$HOANGSA_INSTALL_DIR" "$HOANGSA_CLI_DIR"
    fi
}

# Sets RC_FILE + RC_SYNTAX based on $SHELL. Empty RC_FILE = unsupported shell.
pick_rc_file() {
    RC_FILE=""
    RC_SYNTAX=""
    _shell_name="${SHELL##*/}"
    case "$_shell_name" in
        zsh)
            RC_FILE="$HOME/.zshrc"
            RC_SYNTAX=posix
            ;;
        bash)
            # macOS login shells read .bash_profile, not .bashrc. Linux bash
            # reads .bashrc for interactive non-login shells (the common case).
            if [ "$(uname -s 2>/dev/null)" = Darwin ]; then
                RC_FILE="$HOME/.bash_profile"
            else
                RC_FILE="$HOME/.bashrc"
            fi
            RC_SYNTAX=posix
            ;;
        fish)
            RC_FILE="$HOME/.config/fish/config.fish"
            RC_SYNTAX=fish
            ;;
    esac
}

print_managed_line() {
    if [ "$RC_SYNTAX" = fish ]; then
        managed_export_line_fish
    else
        managed_export_line_posix
    fi
}

print_manual_export() {
    info "add the following line to your shell rc file:"
    printf '    '
    print_managed_line
}

# Strip any existing managed block from $1, append a fresh one.
rewrite_managed_block() {
    _rc="$1"
    _tmp="$_rc.hoangsa.tmp.$$"

    _dir=$(dirname "$_rc")
    [ -d "$_dir" ] || mkdir -p "$_dir"
    [ -f "$_rc" ] || touch "$_rc"

    awk -v s="$HOANGSA_MARK_START" -v e="$HOANGSA_MARK_END" '
        index($0, s) { flag=1; next }
        index($0, e) { flag=0; next }
        !flag        { print }
    ' "$_rc" > "$_tmp" || {
        rm -f "$_tmp"
        return 1
    }

    if [ -s "$_tmp" ] && [ -n "$(tail -c 1 "$_tmp" 2>/dev/null)" ]; then
        printf '\n' >> "$_tmp"
    fi

    {
        printf '%s\n' "$HOANGSA_MARK_START"
        print_managed_line
        printf '%s\n' "$HOANGSA_MARK_END"
    } >> "$_tmp"

    mv -f "$_tmp" "$_rc"
}

edit_path_in_rc() {
    # Resolve shell first so the manual-export fallback prints the right
    # syntax for the user's shell regardless of which gate we exit through.
    # Falls back to POSIX syntax for unsupported shells.
    pick_rc_file
    [ -n "$RC_SYNTAX" ] || RC_SYNTAX=posix

    # Already reachable? (Both dirs must be on PATH — otherwise either the
    # memory bins or the CLI stays unreachable from fresh shells.)
    case ":$PATH:" in
        *":$HOANGSA_INSTALL_DIR/bin:"*)
            case ":$PATH:" in
                *":$HOANGSA_CLI_DIR:"*)
                    info "PATH already contains install dirs — no rc edit needed"
                    return 0
                    ;;
            esac
            ;;
    esac

    if [ "$HOANGSA_NO_PATH_EDIT" = "1" ]; then
        info "HOANGSA_NO_PATH_EDIT=1 — skipping rc file edit"
        print_manual_export
        return 0
    fi

    if [ -z "$RC_FILE" ]; then
        info "unsupported shell (\$SHELL=${SHELL:-unset}) — skipping rc file edit"
        print_manual_export
        return 0
    fi

    if [ "$DRY_RUN" -eq 1 ]; then
        info "dry-run: would update managed block in $RC_FILE"
        print_manual_export
        return 0
    fi

    if [ ! -t 0 ] || [ ! -t 1 ]; then
        info "non-interactive shell — skipping rc file edit"
        print_manual_export
        return 0
    fi

    printf 'Add hoangsa bin dir to PATH in %s? [Y/n] ' "$RC_FILE"
    REPLY=""
    # shellcheck disable=SC2039  # `read -r` is POSIX
    read -r REPLY || REPLY=""
    case "$REPLY" in
        n*|N*)
            info "skipped rc file edit (user declined)"
            print_manual_export
            return 0
            ;;
    esac

    if ! rewrite_managed_block "$RC_FILE"; then
        info "failed to update $RC_FILE"
        print_manual_export
        return 0
    fi

    info "PATH updated in $RC_FILE. Open a new shell or run: source $RC_FILE"
}

CARGO_PKGS="-p hoangsa-cli -p hoangsa-memory -p hoangsa-memory-mcp -p hoangsa-proxy"
REQUIRED_BINS="hoangsa-cli hoangsa-memory hoangsa-memory-mcp hsp"

# --- Build ------------------------------------------------------------------

if [ "$CARGO_PROFILE" = "release" ]; then
    BIN_DIR="$REPO_ROOT/target/release"
    CARGO_FLAGS="--release"
else
    BIN_DIR="$REPO_ROOT/target/debug"
    CARGO_FLAGS=""
fi

ui_banner "local-dev"

section "build"
if [ "$SKIP_BUILD" -eq 0 ]; then
    info "building binaries (profile: $CARGO_PROFILE)"
    # shellcheck disable=SC2086
    cargo build $CARGO_FLAGS $CARGO_PKGS
else
    info "skipping build; using $BIN_DIR"
fi

for b in $REQUIRED_BINS; do
    [ -x "$BIN_DIR/$b" ] || die "missing binary: $BIN_DIR/$b (drop --skip-build?)"
done

# --- Install binaries -------------------------------------------------------
#
# All four binaries land here so `install-local.sh` mirrors the layout the
# tarball path produces: CLI-tier bins (hoangsa-cli, hsp) go to
# $HOANGSA_CLI_DIR; memory bins (hoangsa-memory, hoangsa-memory-mcp) go to
# $HOANGSA_INSTALL_DIR/bin. On the default layout these two dirs are the
# same. Doing the copies here (instead of routing memory bins through the
# CLI's relocate step) means the user sees every install action as one
# contiguous block in the output.

install_bin() {
    _name="$1"
    _dst_dir="$2"
    _dst="$_dst_dir/$_name"
    _src="$BIN_DIR/$_name"
    if [ "$DRY_RUN" -eq 1 ]; then
        info "dry-run: would install $_src -> $_dst"
        return 0
    fi
    info "installing $_name -> $_dst"
    mkdir -p "$_dst_dir"
    _tmp="$_dst.new.$$"
    cp "$_src" "$_tmp"
    chmod 0755 "$_tmp"
    mv -f "$_tmp" "$_dst"
}

section "install bins"
install_bin hoangsa-cli        "$HOANGSA_CLI_DIR"
install_bin hsp                "$HOANGSA_CLI_DIR"
install_bin hoangsa-memory     "$HOANGSA_INSTALL_DIR/bin"
install_bin hoangsa-memory-mcp "$HOANGSA_INSTALL_DIR/bin"

# --- Pick target Claude config dir + PATH rc-file edit ----------------------
#
# Global installs drop binaries into $HOANGSA_CLI_DIR (= ~/.hoangsa/bin by
# default). That dir is not on most users' PATH, so without this step a
# successful install still leaves `hoangsa-cli` / `hsp` unreachable from the
# shell. Local installs bypass this — project-scoped consumers don't need PATH.

if [ "$IS_GLOBAL" -eq 1 ]; then
    pick_claude_dir
    # Export so the CLI (and its `claude_config_dir()` helper) writes to the
    # chosen profile instead of defaulting to $HOME/.claude.
    CLAUDE_CONFIG_DIR="$CLAUDE_DIR_PICK"
    export CLAUDE_CONFIG_DIR
    edit_path_in_rc
fi

# --- Stage templates for the CLI --------------------------------------------
#
# `hoangsa-cli install` copies templates from $HOANGSA_TEMPLATES_DIR into the
# Claude config dir. Memory bins are already installed above, so staging is
# templates-only (no $HOANGSA_STAGING_DIR needed).

section "staging"
STAGING=$(mktemp -d "${TMPDIR:-/tmp}/hoangsa-local.XXXXXX")
trap 'rm -rf "$STAGING"' EXIT INT TERM

info "staging templates into $STAGING"
cp -R "$REPO_ROOT/templates" "$STAGING/templates"

HOANGSA_TEMPLATES_DIR="$STAGING/templates"
export HOANGSA_TEMPLATES_DIR

# Drop trap before the CLI runs — it reads $HOANGSA_TEMPLATES_DIR after we
# return control, and we don't want EXIT to yank the dir mid-read.
trap - EXIT INT TERM

# --- Vector store ----------------------------------------------------------
#
# The fastembed-powered vector store runs entirely in-process. We
# pre-download the `multilingual-e5-small` weights (~118 MB) into
# `$HOANGSA_INSTALL_DIR/cache/fastembed` so the first real `index` /
# `query` / `archive ingest` call doesn't stall 30–60 s on a
# HuggingFace fetch. Failure is non-fatal — the weights will be fetched
# lazily on first use instead. Skipped when `--no-embed` is passed.
prefetch_embed_model() {
    _bin="$BIN_DIR/hoangsa-memory"
    if [ "$DRY_RUN" -eq 1 ]; then
        info "dry-run: would run $_bin prefetch-embed"
        return 0
    fi
    if [ ! -x "$_bin" ]; then
        info "skipping prefetch: $_bin not found"
        return 0
    fi
    info "pre-downloading fastembed model (~4xx MB)"
    HOANGSA_INSTALL_DIR="$HOANGSA_INSTALL_DIR" "$_bin" prefetch-embed \
        || info "prefetch failed — weights will download on first use"
}

section "vector store"
if [ "$SKIP_EMBED" -eq 0 ]; then
    prefetch_embed_model
else
    info "--no-embed — skipping fastembed model pre-download"
fi

# --- Hand off to the CLI ----------------------------------------------------

CLI="$BIN_DIR/hoangsa-cli"
if [ ! -x "$CLI" ]; then
    die "hoangsa-cli not found at $CLI (build first or drop --skip-build)"
fi

section "cli"
info "running: $CLI install $PASSTHROUGH"

# Capture stdout so render_summary / render_dry_run can pretty-print the
# CLI's JSON output. Stderr stays attached so diagnostics stream live.
_cli_out=""
_cli_exit=0
# shellcheck disable=SC2086
_cli_out=$(eval "\"$CLI\"" install $PASSTHROUGH) || _cli_exit=$?

if [ "$DRY_RUN" -eq 1 ]; then
    render_dry_run "$_cli_out"
else
    render_summary "$_cli_out"
fi
exit "$_cli_exit"
