#!/bin/sh
# hoangsa installer — POSIX sh bootstrap.
#
# Usage:
#   curl -fsSL https://github.com/unknown-studio-dev/hoangsa/releases/latest/download/install.sh | sh
#   curl -fsSL https://github.com/unknown-studio-dev/hoangsa/releases/download/<tag>/install.sh | sh -s -- --local
#
# Environment variables:
#   HOANGSA_VERSION     Release tag to install (default: latest)
#   HOANGSA_REPO        GitHub repo slug (default: unknown-studio-dev/hoangsa)
#   HOANGSA_INSTALL_DIR Install root for all binaries (default: $HOME/.hoangsa)
#   HOANGSA_CLI_DIR     Install root for hoangsa-cli (default: $HOANGSA_INSTALL_DIR/bin)
#   HOANGSA_NO_PATH_EDIT If "1", skip rc file edit (reserved for T-10)
#   HOANGSA_TEST_MODE   If set, skip main block (for sourcing in tests)
#
# Exit codes:
#   0  success
#   1  install step failure
#   2  invalid argument or unsupported platform
#   3  missing prerequisite (curl/wget/tar/sha256)

set -eu

# Source the shared UI lib (info/warn/err/section/render_summary/…). The
# release workflow inlines scripts/lib/ui.sh above this line and strips the
# source statement below, so curl|sh consumers get a single self-contained
# file. Checkout users read the lib at runtime.
. "$(dirname "$0")/lib/ui.sh"

# ---------------------------------------------------------------------------
# Config / constants
# ---------------------------------------------------------------------------

HOANGSA_REPO="${HOANGSA_REPO:-unknown-studio-dev/hoangsa}"
HOANGSA_VERSION="${HOANGSA_VERSION:-latest}"
HOANGSA_INSTALL_DIR="${HOANGSA_INSTALL_DIR:-$HOME/.hoangsa}"
HOANGSA_CLI_DIR="${HOANGSA_CLI_DIR:-$HOANGSA_INSTALL_DIR/bin}"
HOANGSA_NO_PATH_EDIT="${HOANGSA_NO_PATH_EDIT:-}"

SUPPORTED_TRIPLES="darwin-arm64 linux-x64 linux-arm64 linux-x64-musl"

# ---------------------------------------------------------------------------
# Helpers (info/warn/err are provided by lib/ui.sh)
# ---------------------------------------------------------------------------

die() {
    code="$1"
    shift
    err "$*"
    exit "$code"
}

have() {
    command -v "$1" >/dev/null 2>&1
}

usage() {
    cat <<'EOF'
hoangsa installer — POSIX sh bootstrap

USAGE:
    install.sh [FLAGS] [-- passthrough args]

FLAGS (forwarded to `hoangsa-cli install`):
    --global            Install globally for the current user (default)
    --local             Install for the current project (cwd)
    --no-embed          Skip pre-downloading the fastembed model weights.
                        Weights will fetch lazily on first index/query.
    --dry-run           Print actions without writing files
    --help, -h          Show this help and exit

To uninstall, use scripts/uninstall.sh from a checkout of the repo.

ENVIRONMENT:
    HOANGSA_VERSION     Release tag (default: latest)
    HOANGSA_REPO        GitHub repo slug (default: unknown-studio-dev/hoangsa)
    HOANGSA_INSTALL_DIR Install root for all bins (default: ~/.hoangsa)
    HOANGSA_CLI_DIR     Install root for hoangsa-cli (default: $HOANGSA_INSTALL_DIR/bin)
    HOANGSA_NO_PATH_EDIT If "1", do not touch rc files (manual export only)

EXAMPLES:
    curl -fsSL https://github.com/unknown-studio-dev/hoangsa/releases/latest/download/install.sh | sh
    curl -fsSL https://github.com/unknown-studio-dev/hoangsa/releases/latest/download/install.sh | sh -s -- --local
    HOANGSA_VERSION=v0.1.5 sh install.sh --global --dry-run
EOF
}

# ---------------------------------------------------------------------------
# Arg parse (flag passthrough with minimal local awareness)
# ---------------------------------------------------------------------------

PASSTHROUGH=""
HAS_MODE_FLAG=0
IS_GLOBAL=0
SKIP_EMBED=0

append_arg() {
    # Append a shell-quoted arg to PASSTHROUGH so we can re-expand with `eval`.
    # POSIX-safe single-quote escaping.
    quoted=$(printf "%s" "$1" | sed "s/'/'\\\\''/g")
    if [ -z "$PASSTHROUGH" ]; then
        PASSTHROUGH="'$quoted'"
    else
        PASSTHROUGH="$PASSTHROUGH '$quoted'"
    fi
}

for arg in "$@"; do
    case "$arg" in
        --help|-h)
            usage
            exit 0
            ;;
        --global)
            IS_GLOBAL=1
            HAS_MODE_FLAG=1
            append_arg "$arg"
            ;;
        --local)
            HAS_MODE_FLAG=1
            append_arg "$arg"
            ;;
        --no-embed)
            SKIP_EMBED=1
            ;;
        --dry-run)
            append_arg "$arg"
            ;;
        *)
            append_arg "$arg"
            ;;
    esac
done

if [ "$HAS_MODE_FLAG" -eq 0 ]; then
    # Default to --global for curl|sh ergonomics.
    IS_GLOBAL=1
    if [ -z "$PASSTHROUGH" ]; then
        PASSTHROUGH="'--global'"
    else
        PASSTHROUGH="'--global' $PASSTHROUGH"
    fi
fi

# ---------------------------------------------------------------------------
# Platform detection
# ---------------------------------------------------------------------------

detect_triple() {
    uname_s=$(uname -s 2>/dev/null || echo unknown)
    uname_m=$(uname -m 2>/dev/null || echo unknown)

    case "$uname_s" in
        Darwin)  os=darwin ;;
        Linux)   os=linux ;;
        *)       die 2 "unsupported OS: $uname_s (supported: $SUPPORTED_TRIPLES)" ;;
    esac

    case "$uname_m" in
        x86_64|amd64)   arch=x64 ;;
        arm64|aarch64)  arch=arm64 ;;
        *)              die 2 "unsupported architecture: $uname_m (supported: $SUPPORTED_TRIPLES)" ;;
    esac

    triple="$os-$arch"

    # musl detection (Linux x64 only — Alpine et al.)
    if [ "$os" = linux ] && [ "$arch" = x64 ]; then
        if have ldd && ldd --version 2>&1 | grep -qi musl; then
            triple="linux-x64-musl"
        else
            for f in /lib/ld-musl-x86_64.so.1 /lib/ld-musl-i386.so.1; do
                if [ -f "$f" ]; then
                    triple="linux-x64-musl"
                    break
                fi
            done
        fi
    fi

    # Verify triple is supported.
    ok=0
    for t in $SUPPORTED_TRIPLES; do
        if [ "$t" = "$triple" ]; then
            ok=1
            break
        fi
    done
    if [ "$ok" -ne 1 ]; then
        die 2 "unsupported platform: $triple (supported: $SUPPORTED_TRIPLES)"
    fi

    TRIPLE="$triple"
    info "detected platform: $TRIPLE"
}

# ---------------------------------------------------------------------------
# Prereq check
# ---------------------------------------------------------------------------

check_prereqs() {
    if have curl; then
        DOWNLOADER=curl
    elif have wget; then
        DOWNLOADER=wget
    else
        die 3 "neither curl nor wget found; install one and retry"
    fi

    if ! have tar; then
        die 3 "tar not found; install GNU/BSD tar and retry"
    fi

    if have sha256sum; then
        SHA256="sha256sum"
    elif have shasum; then
        SHA256="shasum -a 256"
    else
        die 3 "neither sha256sum nor shasum found; install coreutils or perl-shasum"
    fi
}

# ---------------------------------------------------------------------------
# Download helpers
# ---------------------------------------------------------------------------

fetch_to() {
    # fetch_to <url> <dest>
    _url="$1"
    _dest="$2"
    if [ "$DOWNLOADER" = curl ]; then
        curl -fsSL --retry 3 --retry-delay 2 -o "$_dest" "$_url"
    else
        wget -q -O "$_dest" "$_url"
    fi
}

fetch_stdout() {
    # fetch_stdout <url>
    _url="$1"
    if [ "$DOWNLOADER" = curl ]; then
        curl -fsSL --retry 3 --retry-delay 2 "$_url"
    else
        wget -q -O - "$_url"
    fi
}

# ---------------------------------------------------------------------------
# PATH rc-file edit (managed markers, TTY-gated, HOANGSA_NO_PATH_EDIT aware)
# ---------------------------------------------------------------------------

# Single source of truth for the PATH export line written into rc files and
# printed to the user as a manual fallback. Both consumers MUST go through
# this helper so the two copies cannot drift.
#
# Emits a literal `$PATH` so the user's shell (or the rc file) expands it at
# source time, not our installer. `$HOANGSA_INSTALL_DIR` and `$HOANGSA_CLI_DIR`
# ARE expanded here — we bake the absolute paths into the rc file so relocating
# the installer state does not silently break the rc snippet.
managed_export_line() {
    # shellcheck disable=SC2016  # `$PATH` intentionally literal.
    if [ "$HOANGSA_INSTALL_DIR/bin" = "$HOANGSA_CLI_DIR" ]; then
        printf 'export PATH="%s:$PATH"\n' "$HOANGSA_CLI_DIR"
    else
        printf 'export PATH="%s/bin:%s:$PATH"\n' \
            "$HOANGSA_INSTALL_DIR" "$HOANGSA_CLI_DIR"
    fi
}

# Print the manual export instructions. Used as a fallback whenever the rc
# edit is skipped for any reason (env flag, non-TTY, user declined, no rc).
print_manual_export() {
    info "add the following line to your ~/.zshrc or ~/.bashrc:"
    printf '    '
    managed_export_line
}

# Managed-block markers. Used by both the awk strip pass and the append pass
# below — keep in sync via this single constant.
HOANGSA_MARK_START='# hoangsa:managed start'
HOANGSA_MARK_END='# hoangsa:managed end'

# Rewrite the managed block inside the given rc file. Strips any existing
# block delimited by the managed markers, then appends a fresh one. BSD/GNU
# awk compatible (plain POSIX awk only — no gensub, no \b, no gawk extensions).
rewrite_managed_block() {
    _rc="$1"
    _tmp="$_rc.hoangsa.tmp.$$"

    # Ensure the file exists before awk reads it. `touch` is POSIX.
    [ -f "$_rc" ] || touch "$_rc"

    # Strip any existing managed block (inclusive of both marker lines).
    # Pass markers in as awk vars so we never duplicate the literal strings.
    awk -v s="$HOANGSA_MARK_START" -v e="$HOANGSA_MARK_END" '
        index($0, s) { flag=1; next }
        index($0, e) { flag=0; next }
        !flag        { print }
    ' "$_rc" > "$_tmp" || {
        rm -f "$_tmp"
        return 1
    }

    # Ensure trailing newline before appending the fresh block. Command
    # substitution strips trailing newlines, so a non-empty `tail -c 1`
    # means the file does NOT end in \n.
    if [ -s "$_tmp" ] && [ -n "$(tail -c 1 "$_tmp" 2>/dev/null)" ]; then
        printf '\n' >> "$_tmp"
    fi

    {
        printf '%s\n' "$HOANGSA_MARK_START"
        managed_export_line
        printf '%s\n' "$HOANGSA_MARK_END"
    } >> "$_tmp"

    mv -f "$_tmp" "$_rc"
}

edit_path_in_rc() {
    # Respect explicit opt-out.
    if [ "$HOANGSA_NO_PATH_EDIT" = "1" ]; then
        info "HOANGSA_NO_PATH_EDIT=1 — skipping rc file edit"
        print_manual_export
        return 0
    fi

    # Pick the first existing rc file (zsh wins over bash by convention).
    _rc=""
    if [ -f "$HOME/.zshrc" ]; then
        _rc="$HOME/.zshrc"
    elif [ -f "$HOME/.bashrc" ]; then
        _rc="$HOME/.bashrc"
    else
        warn "no ~/.zshrc or ~/.bashrc found; cannot auto-edit PATH"
        print_manual_export
        return 0
    fi

    # Non-TTY → curl|sh pipeline or other non-interactive shell. Never edit
    # rc files silently; surface the manual line instead.
    if [ ! -t 0 ] || [ ! -t 1 ]; then
        info "non-interactive shell detected — skipping rc file edit"
        print_manual_export
        return 0
    fi

    # Interactive: prompt before touching the rc file.
    printf 'Add ~/.hoangsa/bin to PATH in %s? [Y/n] ' "$_rc"
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

    if ! rewrite_managed_block "$_rc"; then
        warn "failed to update $_rc"
        print_manual_export
        return 0
    fi

    info "PATH updated in $_rc. Open a new shell or run: source $_rc"
}

# ---------------------------------------------------------------------------
# Claude config dir picker (mirrors install-local.sh)
# ---------------------------------------------------------------------------
#
# Claude Code honors `CLAUDE_CONFIG_DIR` so users can run alternate profiles
# via shell aliases (e.g. `zclaude='CLAUDE_CONFIG_DIR=~/.zclaude claude'`).
# Without the same awareness here, `--global` writes land in `~/.claude/` but
# a zclaude session reads from `~/.zclaude/`, leaving hoangsa skills and the
# `hoangsa-memory` MCP invisible inside that session.
#
# Strategy:
#   * `CLAUDE_CONFIG_DIR` already set → honor silently.
#   * Else glob `$HOME/.*claude*` for dirs that look like Claude config dirs
#     (settings.json / projects/ / history.jsonl / .claude.json present).
#     Single hit (the default) → no prompt. Multiple → interactive menu with
#     "custom path" escape hatch. Non-TTY → default to first + log override
#     hint (curl|sh runs are usually non-TTY; users can re-run with the env
#     var explicit).

is_claude_config_dir() {
    [ -d "$1" ] || return 1
    [ -f "$1/settings.json" ] \
        || [ -d "$1/projects" ] \
        || [ -f "$1/history.jsonl" ] \
        || [ -f "$1/.claude.json" ]
}

# Populate $CLAUDE_CANDIDATES (newline-separated). Always includes
# `$HOME/.claude` at the head so the default is available even when the user
# has never launched Claude Code before.
detect_claude_candidates() {
    CLAUDE_CANDIDATES="$HOME/.claude"
    # `.*claude*` (not `.claude*`) catches prefix-style profiles like
    # `.zclaude` from `zclaude='CLAUDE_CONFIG_DIR=~/.zclaude claude'`.
    for d in "$HOME"/.*claude*; do
        [ -d "$d" ] || continue
        [ "$d" = "$HOME/.claude" ] && continue
        if is_claude_config_dir "$d"; then
            CLAUDE_CANDIDATES="$CLAUDE_CANDIDATES
$d"
        fi
    done
}

# Sets $CLAUDE_DIR_PICK. Caller is responsible for `export CLAUDE_CONFIG_DIR`.
pick_claude_dir() {
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
        info "override: CLAUDE_CONFIG_DIR=<dir> sh install.sh --global"
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
        [ -n "$CLAUDE_DIR_PICK" ] || die 2 "empty path"
        # shellcheck disable=SC2088
        case "$CLAUDE_DIR_PICK" in
            "~/"*) CLAUDE_DIR_PICK="$HOME/${CLAUDE_DIR_PICK#\~/}" ;;
            "~")   CLAUDE_DIR_PICK="$HOME" ;;
        esac
    else
        case "$_pick" in
            ''|*[!0-9]*) die 2 "invalid selection: $_pick" ;;
        esac
        if [ "$_pick" -lt 1 ] || [ "$_pick" -gt "$_count" ]; then
            die 2 "selection out of range: $_pick"
        fi
        CLAUDE_DIR_PICK=$(printf '%s\n' "$CLAUDE_CANDIDATES" | sed -n "${_pick}p")
    fi
    info "using Claude config dir: $CLAUDE_DIR_PICK"
}

# ---------------------------------------------------------------------------
# Resolve tag (latest -> vX.Y.Z)
# ---------------------------------------------------------------------------

resolve_tag() {
    if [ "$HOANGSA_VERSION" != latest ] && [ -n "$HOANGSA_VERSION" ]; then
        TAG="$HOANGSA_VERSION"
        return 0
    fi

    info "resolving latest release tag from github.com/$HOANGSA_REPO"
    _api="https://api.github.com/repos/$HOANGSA_REPO/releases/latest"

    # Surface network / HTTP failures explicitly instead of masking them with
    # `|| true` (which let grep pick a bogus tag out of rate-limit JSON or an
    # HTML error page). On failure we abort with a clear error.
    _json=$(fetch_stdout "$_api") \
        || die 1 "fetching release metadata from $_api failed (network or GitHub rate limit)"

    if [ -z "$_json" ]; then
        die 1 "failed to fetch release metadata from $_api (empty response)"
    fi

    # Rate-limit bodies are HTTP 200 from curl's POV (or 403 with a JSON body
    # from wget's); detect the marker string explicitly to give a useful
    # error instead of "could not parse tag_name".
    if printf '%s' "$_json" | grep -q '"message"[[:space:]]*:[[:space:]]*"API rate limit exceeded'; then
        die 1 "GitHub API rate limit exceeded for $_api — set HOANGSA_VERSION=<tag> to skip resolution, or retry later / authenticate"
    fi

    # Parse tag_name without jq. Keep it strict: the first tag_name line wins.
    TAG=$(printf '%s\n' "$_json" \
        | grep -E '"tag_name"[[:space:]]*:' \
        | head -n 1 \
        | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')

    if [ -z "$TAG" ]; then
        die 1 "could not parse tag_name from release metadata at $_api"
    fi
}

# ---------------------------------------------------------------------------
# Vector store bootstrap
# ---------------------------------------------------------------------------
#
# As of Phase 2 of `fix/memory-4bugs` the semantic vector store runs
# in-process via `fastembed`. There is no Python sidecar to provision.
#
# We pre-download the `multilingual-e5-small` weights (~118 MB) into
# `$HOANGSA_INSTALL_DIR/cache/fastembed` so the first real `index` /
# `query` / `archive ingest` call doesn't stall 30–60 s on a
# HuggingFace fetch. Failure is non-fatal — the weights will be fetched
# lazily on first use instead.
prefetch_embed_model() {
    _bin="$HOANGSA_INSTALL_DIR/bin/hoangsa-memory"
    if [ ! -x "$_bin" ]; then
        info "vector store: hoangsa-memory not found at $_bin — skipping prefetch"
        return 0
    fi
    info "pre-downloading fastembed model (~4xx MB) into $HOANGSA_INSTALL_DIR/cache/fastembed"
    HOANGSA_INSTALL_DIR="$HOANGSA_INSTALL_DIR" "$_bin" prefetch-embed \
        || info "prefetch failed — weights will download on first use"
}

# ---------------------------------------------------------------------------
# Main install flow
# ---------------------------------------------------------------------------

main() {
    ui_banner "${HOANGSA_VERSION}"
    section "detect"
    detect_triple
    check_prereqs
    resolve_tag
    info "installing tag: $TAG"

    TMP=$(mktemp -d 2>/dev/null || mktemp -d -t hoangsa-install)
    if [ -z "$TMP" ] || [ ! -d "$TMP" ]; then
        die 1 "failed to create temp directory"
    fi
    # shellcheck disable=SC2064
    trap "rm -rf \"$TMP\"" EXIT INT TERM

    TARBALL_NAME="hoangsa-$TRIPLE.tar.gz"
    BASE_URL="https://github.com/$HOANGSA_REPO/releases/download/$TAG"
    TARBALL_URL="$BASE_URL/$TARBALL_NAME"
    SUMS_URL="$BASE_URL/SHA256SUMS"

    section "download"
    info "downloading $TARBALL_NAME"
    fetch_to "$TARBALL_URL" "$TMP/$TARBALL_NAME" \
        || die 1 "failed to download $TARBALL_URL"

    info "downloading SHA256SUMS"
    fetch_to "$SUMS_URL" "$TMP/SHA256SUMS" \
        || die 1 "failed to download $SUMS_URL"

    info "verifying SHA256"
    # Extract the expected hash from the SHA256SUMS file. Format: "<hex>  <name>".
    expected=$(grep -E "[[:space:]]+\*?$TARBALL_NAME\$" "$TMP/SHA256SUMS" \
        | head -n 1 \
        | awk '{print $1}')
    if [ -z "$expected" ]; then
        die 1 "no SHA256 entry for $TARBALL_NAME in SHA256SUMS"
    fi
    actual=$(cd "$TMP" && $SHA256 "$TARBALL_NAME" | awk '{print $1}')
    if [ "$expected" != "$actual" ]; then
        die 1 "checksum mismatch for $TARBALL_NAME (expected $expected, got $actual)"
    fi
    info "checksum OK"

    info "extracting tarball"
    EXTRACT_DIR="$TMP/extract"
    mkdir -p "$EXTRACT_DIR"
    tar -xzf "$TMP/$TARBALL_NAME" -C "$EXTRACT_DIR" \
        || die 1 "tar extraction failed"

    # Expected layout: hoangsa-<triple>/bin/{hoangsa-cli,hoangsa-memory,hoangsa-memory-mcp}
    # plus templates/ VERSION LICENSE. The top-level directory name is fixed.
    PKG_DIR="$EXTRACT_DIR/hoangsa-$TRIPLE"
    if [ ! -d "$PKG_DIR" ]; then
        # Fall back: pick the first directory inside.
        PKG_DIR=$(find "$EXTRACT_DIR" -mindepth 1 -maxdepth 1 -type d | head -n 1)
    fi
    if [ -z "$PKG_DIR" ] || [ ! -d "$PKG_DIR/bin" ]; then
        die 1 "extracted tarball missing expected bin/ directory"
    fi

    # Install destinations — memory bins are per-user shared, CLI goes to its own dir.
    section "install bins"
    mkdir -p "$HOANGSA_INSTALL_DIR/bin" "$HOANGSA_CLI_DIR"

    CLI_SRC="$PKG_DIR/bin/hoangsa-cli"
    if [ ! -f "$CLI_SRC" ]; then
        die 1 "hoangsa-cli not found in tarball at $CLI_SRC"
    fi

    # Atomic install: copy to a sibling tmp file in the target dir, chmod, then rename.
    install_bin() {
        _src="$1"
        _dst="$2"
        if [ ! -f "$_src" ]; then
            warn "missing binary $_src (skipping)"
            return 0
        fi
        info "installing $(basename "$_src") -> $_dst"
        _tmp="$_dst.new.$$"
        cp "$_src" "$_tmp"
        chmod 0755 "$_tmp"
        mv -f "$_tmp" "$_dst"
    }

    install_bin "$CLI_SRC" "$HOANGSA_CLI_DIR/hoangsa-cli"
    install_bin "$PKG_DIR/bin/hoangsa-memory" "$HOANGSA_INSTALL_DIR/bin/hoangsa-memory"
    install_bin "$PKG_DIR/bin/hoangsa-memory-mcp" "$HOANGSA_INSTALL_DIR/bin/hoangsa-memory-mcp"

    # PATH rc-file append with managed markers + TTY gating. Skipped only when
    # BOTH the memory-bin dir AND the CLI dir are already on $PATH — otherwise
    # either `hoangsa-memory*` or `hoangsa-cli` stays unreachable from new
    # shells. (Bug H: previously only the memory-bin dir was checked, so a
    # second install with the memory dir already on PATH left the CLI dir
    # unlinked.)
    _need_path_edit=1
    case ":$PATH:" in
        *":$HOANGSA_INSTALL_DIR/bin:"*)
            case ":$PATH:" in
                *":$HOANGSA_CLI_DIR:"*) _need_path_edit=0 ;;
            esac
            ;;
    esac
    if [ "$_need_path_edit" -eq 1 ]; then
        edit_path_in_rc
    fi

    # Pick the Claude config dir to install into and propagate to the CLI.
    # Only relevant for --global; --local writes everything under cwd/.claude/
    # and never touches a Claude profile dir.
    if [ "$IS_GLOBAL" -eq 1 ]; then
        pick_claude_dir
        CLAUDE_CONFIG_DIR="$CLAUDE_DIR_PICK"
        export CLAUDE_CONFIG_DIR
    fi

    # Stage templates in a persistent directory OUTSIDE $TMP so the CLI can
    # read them after our $TMP cleanup. The memory bins were already installed
    # to their final home above, so only templates need staging.
    #
    # Cleanup is the CLI's responsibility. On systems with `/tmp` cleanup
    # (macOS, systemd-tmpfiles on Linux) a leaked staging dir is benign.
    section "staging"
    STAGING=$(mktemp -d "${TMPDIR:-/tmp}/hoangsa-staging.XXXXXX" 2>/dev/null \
        || mktemp -d -t hoangsa-staging)
    if [ -z "$STAGING" ] || [ ! -d "$STAGING" ]; then
        die 1 "failed to create staging directory"
    fi

    if [ -d "$PKG_DIR/templates" ]; then
        mv "$PKG_DIR/templates" "$STAGING/templates" \
            || die 1 "failed to stage templates into $STAGING/templates"
        HOANGSA_TEMPLATES_DIR="$STAGING/templates"
        export HOANGSA_TEMPLATES_DIR
        info "staged templates -> $STAGING/templates"
    fi

    # Pre-download the fastembed model weights unless --no-embed was passed.
    # Runs before the CLI hand-off so the CLI inherits a warm cache.
    if [ "$SKIP_EMBED" -eq 0 ]; then
        prefetch_embed_model
    else
        info "--no-embed — skipping fastembed model pre-download"
    fi

    # Clear the cleanup trap now that we're done with $TMP. We drop the trap
    # only after consuming everything we needed from it; the tarball +
    # checksums are no longer referenced.
    rm -rf "$TMP"
    trap - EXIT INT TERM

    # Hand off to the CLI for the real install work. Unlike the old `exec`
    # path, we capture stdout so the final JSON summary can be rendered as
    # pretty UI via render_summary / render_dry_run. Stderr stays attached to
    # the user's terminal so CLI diagnostics stream in real time.
    HOANGSA_CLI="$HOANGSA_CLI_DIR/hoangsa-cli"
    section "cli"
    info "running: $HOANGSA_CLI install $PASSTHROUGH"

    _cli_out=""
    _cli_exit=0
    # shellcheck disable=SC2086
    _cli_out=$(eval "\"$HOANGSA_CLI\"" install $PASSTHROUGH) || _cli_exit=$?

    # Dispatch on --dry-run because the preview JSON has a different shape
    # (status=preview, actions[]) than the final summary.
    case " $PASSTHROUGH " in
        *\'--dry-run\'*) render_dry_run "$_cli_out" ;;
        *)               render_summary "$_cli_out" ;;
    esac
    exit "$_cli_exit"
}

# Allow sourcing for tests without running main.
if [ -z "${HOANGSA_TEST_MODE:-}" ]; then
    main
fi
