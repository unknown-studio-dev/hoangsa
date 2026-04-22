#!/bin/sh
# hoangsa local installer — build from source and run `hoangsa-cli install`
# with the same HOANGSA_TEMPLATES_DIR / HOANGSA_STAGING_DIR handoff that
# scripts/install.sh performs after downloading a release tarball. Also
# installs the `hsp` proxy binary, which the CLI install flow does not own.
#
# Usage:
#   scripts/install-local.sh [--global|--local] [--dry-run] [--uninstall]
#                            [--skip-build] [-- extra args forwarded to CLI]
#
# Environment variables:
#   HOANGSA_INSTALL_DIR  Install root for all binaries (default: $HOME/.hoangsa)
#   HOANGSA_CLI_DIR      Install root for hoangsa-cli / hsp (default: $HOANGSA_INSTALL_DIR/bin)
#   CARGO_PROFILE        release|debug (default: release)

set -eu

REPO_ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$REPO_ROOT"

CARGO_PROFILE="${CARGO_PROFILE:-release}"
HOANGSA_INSTALL_DIR="${HOANGSA_INSTALL_DIR:-$HOME/.hoangsa}"
HOANGSA_CLI_DIR="${HOANGSA_CLI_DIR:-$HOANGSA_INSTALL_DIR/bin}"
SKIP_BUILD=0
DRY_RUN=0
UNINSTALL=0
PASSTHROUGH=""
HAS_MODE_FLAG=0

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
        --uninstall)  UNINSTALL=1;  append_arg "$arg" ;;
        --global|--local) HAS_MODE_FLAG=1; append_arg "$arg" ;;
        --install-chroma) append_arg "$arg" ;;
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

info() { printf '==> %s\n' "$*"; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

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

# Skip the build on pure --uninstall — no binaries are needed to delete files.
if [ "$SKIP_BUILD" -eq 0 ] && [ "$UNINSTALL" -eq 0 ]; then
    info "building binaries (profile: $CARGO_PROFILE)"
    # shellcheck disable=SC2086
    cargo build $CARGO_FLAGS $CARGO_PKGS
elif [ "$UNINSTALL" -eq 0 ]; then
    info "skipping build; using $BIN_DIR"
fi

if [ "$UNINSTALL" -eq 0 ]; then
    for b in $REQUIRED_BINS; do
        [ -x "$BIN_DIR/$b" ] || die "missing binary: $BIN_DIR/$b (drop --skip-build?)"
    done
fi

# --- Install / uninstall CLI-tier binaries (hoangsa-cli, hsp) ---------------
#
# `hoangsa-cli install` itself owns templates + memory bins but does NOT copy
# its own binary or hsp. We manage both here so a user running
# `install-local.sh` ends up with every launcher reachable via
# `~/.hoangsa/bin/` (matching the tarball layout from `scripts/install.sh`).
# We touch HOANGSA_CLI_DIR *before* exec'ing the CLI so the CLI's own writes
# to the same dir don't race with ours.

install_cli_bin() {
    _name="$1"
    _dst="$HOANGSA_CLI_DIR/$_name"
    _src="$BIN_DIR/$_name"
    if [ "$UNINSTALL" -eq 1 ]; then
        if [ "$DRY_RUN" -eq 1 ]; then
            info "dry-run: would remove $_dst"
        elif [ -e "$_dst" ]; then
            info "removing $_dst"
            rm -f "$_dst"
        fi
        return 0
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
        info "dry-run: would install $_src -> $_dst"
        return 0
    fi
    info "installing $_name -> $_dst"
    mkdir -p "$HOANGSA_CLI_DIR"
    _tmp="$_dst.new.$$"
    cp "$_src" "$_tmp"
    chmod 0755 "$_tmp"
    mv -f "$_tmp" "$_dst"
}

install_cli_bin hoangsa-cli
install_cli_bin hsp

# --- Stage templates + memory bins (mirrors install.sh layout) --------------

if [ "$UNINSTALL" -eq 0 ]; then
    STAGING=$(mktemp -d "${TMPDIR:-/tmp}/hoangsa-local.XXXXXX")
    trap 'rm -rf "$STAGING"' EXIT INT TERM

    info "staging into $STAGING"
    cp -R "$REPO_ROOT/templates" "$STAGING/templates"
    mkdir -p "$STAGING/bin"
    cp "$BIN_DIR/hoangsa-memory"     "$STAGING/bin/"
    cp "$BIN_DIR/hoangsa-memory-mcp" "$STAGING/bin/"

    HOANGSA_TEMPLATES_DIR="$STAGING/templates"
    HOANGSA_STAGING_DIR="$STAGING"
    export HOANGSA_TEMPLATES_DIR HOANGSA_STAGING_DIR

    # Drop trap before exec — the CLI owns $STAGING from here on (it moves bins
    # out of staging/bin into the install dirs, then cleans up). If we keep the
    # trap, the shell's EXIT handler would yank $STAGING before the CLI reads it.
    trap - EXIT INT TERM
fi

# --- Hand off to the CLI ----------------------------------------------------

CLI="$BIN_DIR/hoangsa-cli"
# On --uninstall we still need the CLI binary (installed or from target/).
# Fall back to an installed hoangsa-cli if we skipped the build for uninstall.
if [ ! -x "$CLI" ]; then
    if command -v hoangsa-cli >/dev/null 2>&1; then
        CLI=$(command -v hoangsa-cli)
    else
        die "hoangsa-cli not found (build first or install via --global / --local)"
    fi
fi

info "running: $CLI install $PASSTHROUGH"
# shellcheck disable=SC2086
eval exec "\"$CLI\"" install $PASSTHROUGH
