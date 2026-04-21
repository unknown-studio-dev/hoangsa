#!/bin/sh
# hoangsa installer — POSIX sh bootstrap.
#
# Usage:
#   curl -fsSL https://github.com/pirumu/hoangsa/releases/latest/download/install.sh | sh
#   curl -fsSL https://github.com/pirumu/hoangsa/releases/download/<tag>/install.sh | sh -s -- --local
#
# Environment variables:
#   HOANGSA_VERSION     Release tag to install (default: latest)
#   HOANGSA_REPO        GitHub repo slug (default: pirumu/hoangsa)
#   HOANGSA_INSTALL_DIR Install root for memory binaries (default: $HOME/.hoangsa-memory)
#   HOANGSA_CLI_DIR     Install root for hoangsa-cli (default: $HOME/.hoangsa/bin)
#   HOANGSA_NO_PATH_EDIT If "1", skip rc file edit (reserved for T-10)
#   HOANGSA_TEST_MODE   If set, skip main block (for sourcing in tests)
#
# Exit codes:
#   0  success
#   1  install step failure
#   2  invalid argument or unsupported platform
#   3  missing prerequisite (curl/wget/tar/sha256)

set -eu

# ---------------------------------------------------------------------------
# Config / constants
# ---------------------------------------------------------------------------

HOANGSA_REPO="${HOANGSA_REPO:-pirumu/hoangsa}"
HOANGSA_VERSION="${HOANGSA_VERSION:-latest}"
HOANGSA_INSTALL_DIR="${HOANGSA_INSTALL_DIR:-$HOME/.hoangsa-memory}"
HOANGSA_CLI_DIR="${HOANGSA_CLI_DIR:-$HOME/.hoangsa/bin}"
HOANGSA_NO_PATH_EDIT="${HOANGSA_NO_PATH_EDIT:-}"

SUPPORTED_TRIPLES="darwin-arm64 darwin-x64 linux-x64 linux-arm64 linux-x64-musl"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

info() {
    printf '==> %s\n' "$*"
}

warn() {
    printf 'warning: %s\n' "$*" >&2
}

err() {
    printf 'error: %s\n' "$*" >&2
}

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
    --uninstall         Remove a previous install (combine with --global/--local)
    --install-chroma    Provision the chroma sidecar venv only
    --dry-run           Print actions without writing files
    --help, -h          Show this help and exit

ENVIRONMENT:
    HOANGSA_VERSION     Release tag (default: latest)
    HOANGSA_REPO        GitHub repo slug (default: pirumu/hoangsa)
    HOANGSA_INSTALL_DIR Install root for memory bins (default: ~/.hoangsa-memory)
    HOANGSA_CLI_DIR     Install root for hoangsa-cli (default: ~/.hoangsa/bin)
    HOANGSA_NO_PATH_EDIT If "1", do not touch rc files (manual export only)

EXAMPLES:
    curl -fsSL https://github.com/pirumu/hoangsa/releases/latest/download/install.sh | sh
    curl -fsSL https://github.com/pirumu/hoangsa/releases/latest/download/install.sh | sh -s -- --local
    HOANGSA_VERSION=v0.1.5 sh install.sh --global --dry-run
EOF
}

# ---------------------------------------------------------------------------
# Arg parse (flag passthrough with minimal local awareness)
# ---------------------------------------------------------------------------

PASSTHROUGH=""
HAS_MODE_FLAG=0

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
        --global|--local)
            HAS_MODE_FLAG=1
            append_arg "$arg"
            ;;
        --uninstall|--install-chroma|--dry-run)
            append_arg "$arg"
            ;;
        *)
            append_arg "$arg"
            ;;
    esac
done

if [ "$HAS_MODE_FLAG" -eq 0 ]; then
    # Default to --global for curl|sh ergonomics.
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
# Resolve tag (latest -> vX.Y.Z)
# ---------------------------------------------------------------------------

resolve_tag() {
    if [ "$HOANGSA_VERSION" != latest ] && [ -n "$HOANGSA_VERSION" ]; then
        TAG="$HOANGSA_VERSION"
        return 0
    fi

    info "resolving latest release tag from github.com/$HOANGSA_REPO"
    _api="https://api.github.com/repos/$HOANGSA_REPO/releases/latest"
    _json=$(fetch_stdout "$_api" || true)
    if [ -z "$_json" ]; then
        die 1 "failed to fetch release metadata from $_api"
    fi

    # Parse tag_name without jq. Keep it strict: the first tag_name line wins.
    TAG=$(printf '%s\n' "$_json" \
        | grep -E '"tag_name"[[:space:]]*:' \
        | head -n 1 \
        | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')

    if [ -z "$TAG" ]; then
        die 1 "could not parse tag_name from release metadata"
    fi
}

# ---------------------------------------------------------------------------
# Main install flow
# ---------------------------------------------------------------------------

main() {
    info "hoangsa installer starting"
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
    info "installing binaries"
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
        _tmp="$_dst.new.$$"
        cp "$_src" "$_tmp"
        chmod 0755 "$_tmp"
        mv -f "$_tmp" "$_dst"
    }

    install_bin "$CLI_SRC" "$HOANGSA_CLI_DIR/hoangsa-cli"
    install_bin "$PKG_DIR/bin/hoangsa-memory" "$HOANGSA_INSTALL_DIR/bin/hoangsa-memory"
    install_bin "$PKG_DIR/bin/hoangsa-memory-mcp" "$HOANGSA_INSTALL_DIR/bin/hoangsa-memory-mcp"

    # Stage templates alongside the CLI so `hoangsa-cli install` can find them
    # via its $EXE_DIR/../templates/ convention. We leave them in the temp pkg
    # dir and point HOANGSA_TEMPLATES_DIR at it so the Rust subcommand can pick
    # them up without requiring a separate hand-off.
    if [ -d "$PKG_DIR/templates" ]; then
        HOANGSA_TEMPLATES_DIR="$PKG_DIR/templates"
        export HOANGSA_TEMPLATES_DIR
    fi

    # PATH rc-file edit + TTY gating is T-10's job. For now, emit the manual
    # export line so the user sees it on first install regardless of TTY.
    case ":$PATH:" in
        *":$HOANGSA_INSTALL_DIR/bin:"*) ;;
        *)
            info "add the following line to your shell rc file:"
            printf '    export PATH="%s/bin:%s:$PATH"\n' \
                "$HOANGSA_INSTALL_DIR" "$HOANGSA_CLI_DIR"
            ;;
    esac

    # Hand off to the CLI for the real install work. Use eval to re-expand the
    # quoted PASSTHROUGH string built during arg parse.
    HOANGSA_CLI="$HOANGSA_CLI_DIR/hoangsa-cli"
    info "executing: $HOANGSA_CLI install $PASSTHROUGH"
    # shellcheck disable=SC2086
    eval exec "\"$HOANGSA_CLI\"" install $PASSTHROUGH
}

# Allow sourcing for tests without running main.
if [ -z "${HOANGSA_TEST_MODE:-}" ]; then
    main
fi
