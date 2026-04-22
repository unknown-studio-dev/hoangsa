#!/bin/sh
# hoangsa uninstaller — POSIX sh, removes everything install.sh / install-local.sh
# write. This is the only supported uninstall path; hoangsa-cli has no
# uninstall subcommand.
#
# What gets removed:
#   1. Binaries in $HOANGSA_CLI_DIR and $HOANGSA_INSTALL_DIR/bin
#      (hoangsa-cli, hsp, hoangsa-memory, hoangsa-memory-mcp)
#   2. Templates tracked by ~/.hoangsa/manifest.json under the Claude config dir
#      (skills/, commands/, agents/, hoangsa/ entries only — never unknown files)
#   3. HOANGSA-managed hook entries in settings.json (objects with
#      _hoangsa_managed: true) — requires jq; warns-and-skips if absent
#   4. hoangsa-memory entry from mcpServers in ~/.claude.json or <cwd>/.mcp.json
#   5. Managed PATH block in ~/.zshrc / .bashrc / .bash_profile / config.fish
#      (between `# hoangsa:managed start` / `# hoangsa:managed end` markers)
#   6. fastembed model cache at $HOANGSA_INSTALL_DIR/cache/fastembed
#      (global mode only; ~118 MB of ONNX weights the installer downloaded)
#
# What gets preserved by default (user data):
#   * ~/.hoangsa/memory/     — long-term memory store
#   * ~/.hoangsa/share/      — packaged templates cache
#   * <project>/.hoangsa/    — per-project sessions / config
#   Pass --purge to remove ~/.hoangsa entirely (per-project dirs always stay).
#
# Usage:
#   scripts/uninstall.sh --global [--dry-run] [--purge]
#   scripts/uninstall.sh --local  [--dry-run]
#
# Exit codes:
#   0  success (including dry-run)
#   1  runtime failure
#   2  invalid argument

set -eu

# Source shared UI lib (info/warn/err/section/render_*/…). Uninstall runs
# only from a checkout, so runtime sourcing is always possible.
. "$(dirname "$0")/lib/ui.sh"

HOANGSA_INSTALL_DIR="${HOANGSA_INSTALL_DIR:-$HOME/.hoangsa}"
HOANGSA_CLI_DIR="${HOANGSA_CLI_DIR:-$HOANGSA_INSTALL_DIR/bin}"
HOANGSA_NO_PATH_EDIT="${HOANGSA_NO_PATH_EDIT:-}"

MODE=""
DRY_RUN=0
PURGE=0

# info / warn provided by lib/ui.sh. die() stays local because uninstall.sh
# uses the `die "msg" [code]` 2-arg signature (exit code second).
die() { err "$1"; exit "${2:-1}"; }

usage() {
    cat <<'EOF'
hoangsa uninstaller

USAGE:
    scripts/uninstall.sh --global [--dry-run] [--purge]
    scripts/uninstall.sh --local  [--dry-run]

FLAGS:
    --global     Remove global install (Claude config dir + ~/.hoangsa bins)
    --local      Remove project install (./.claude/ + ./.mcp.json entry)
    --dry-run    Print what would be removed; touch nothing
    --purge      (global only) Also remove ~/.hoangsa entirely, including
                 memory/ and share/. Per-project .hoangsa/ dirs are never
                 touched by --purge.
    --help, -h   Show this help

ENVIRONMENT:
    HOANGSA_INSTALL_DIR   Install root (default: ~/.hoangsa)
    HOANGSA_CLI_DIR       CLI bin dir  (default: $HOANGSA_INSTALL_DIR/bin)
    HOANGSA_NO_PATH_EDIT  If "1", skip rc file edits

WARNING: --purge is destructive. It deletes the entire memory store under
~/.hoangsa/memory. Make sure you've backed up anything you need first.
EOF
}

for arg in "$@"; do
    case "$arg" in
        --global)   [ -z "$MODE" ] || die "--global and --local are mutually exclusive" 2; MODE=global ;;
        --local)    [ -z "$MODE" ] || die "--global and --local are mutually exclusive" 2; MODE=local ;;
        --dry-run)  DRY_RUN=1 ;;
        --purge)    PURGE=1 ;;
        --help|-h)  usage; exit 0 ;;
        *)          die "unknown flag: $arg (try --help)" 2 ;;
    esac
done

[ -n "$MODE" ] || die "must specify --global or --local (try --help)" 2
[ "$PURGE" -eq 1 ] && [ "$MODE" != "global" ] && die "--purge requires --global" 2

# --- step helpers -----------------------------------------------------------
#
# Two kinds of destructive action: rm and file-edit. Both route through
# `act` so --dry-run has one gate for everything.

act() {
    if [ "$DRY_RUN" -eq 1 ]; then
        info "dry-run: would: $*"
        return 0
    fi
    info "$*"
    "$@"
}

rm_if_exists() {
    _p="$1"
    if [ -e "$_p" ] || [ -L "$_p" ]; then
        act rm -f "$_p"
    fi
}

rmdir_if_empty() {
    _d="$1"
    if [ -d "$_d" ] && [ -z "$(ls -A "$_d" 2>/dev/null)" ]; then
        act rmdir "$_d"
    fi
}

# --- Claude config dir picker (mirrors install.sh) --------------------------

is_claude_config_dir() {
    [ -d "$1" ] || return 1
    [ -f "$1/settings.json" ] \
        || [ -d "$1/projects" ] \
        || [ -f "$1/history.jsonl" ] \
        || [ -f "$1/.claude.json" ]
}

detect_claude_candidates() {
    CLAUDE_CANDIDATES="$HOME/.claude"
    for d in "$HOME"/.*claude*; do
        [ -d "$d" ] || continue
        [ "$d" = "$HOME/.claude" ] && continue
        if is_claude_config_dir "$d"; then
            CLAUDE_CANDIDATES="$CLAUDE_CANDIDATES
$d"
        fi
    done
}

pick_claude_dir() {
    if [ -n "${CLAUDE_CONFIG_DIR:-}" ]; then
        CLAUDE_DIR_PICK="$CLAUDE_CONFIG_DIR"
        info "honoring CLAUDE_CONFIG_DIR=$CLAUDE_DIR_PICK"
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
        info "override: CLAUDE_CONFIG_DIR=<dir> $0 --global"
        return 0
    fi

    printf '\nMultiple Claude config dirs detected:\n' >&2
    _i=1
    while [ "$_i" -le "$_count" ]; do
        _d=$(printf '%s\n' "$CLAUDE_CANDIDATES" | sed -n "${_i}p")
        printf '  %d) %s\n' "$_i" "$_d" >&2
        _i=$((_i + 1))
    done
    printf 'Pick [1]: ' >&2
    _pick=""
    # shellcheck disable=SC2039
    read -r _pick || _pick=""
    [ -z "$_pick" ] && _pick=1
    case "$_pick" in
        ''|*[!0-9]*) die "invalid selection: $_pick" 2 ;;
    esac
    if [ "$_pick" -lt 1 ] || [ "$_pick" -gt "$_count" ]; then
        die "selection out of range: $_pick" 2
    fi
    CLAUDE_DIR_PICK=$(printf '%s\n' "$CLAUDE_CANDIDATES" | sed -n "${_pick}p")
    info "using Claude config dir: $CLAUDE_DIR_PICK"
}

# --- 1. binaries ------------------------------------------------------------

remove_binaries() {
    info "removing binaries"
    # CLI-tier bins (hoangsa-cli, hsp) live in $HOANGSA_CLI_DIR; memory bins
    # live in $HOANGSA_INSTALL_DIR/bin. On default layout these are the same dir.
    rm_if_exists "$HOANGSA_CLI_DIR/hoangsa-cli"
    rm_if_exists "$HOANGSA_CLI_DIR/hsp"
    rm_if_exists "$HOANGSA_INSTALL_DIR/bin/hoangsa-memory"
    rm_if_exists "$HOANGSA_INSTALL_DIR/bin/hoangsa-memory-mcp"
}

# --- fastembed model cache --------------------------------------------------
#
# The installer pre-downloads `multilingual-e5-small` (~118 MB of ONNX
# weights) into $HOANGSA_INSTALL_DIR/cache/fastembed. We remove it here
# so an uninstall doesn't leave orphan GB-scale junk behind. FASTEMBED_CACHE_DIR
# is honored only if the user set it for *this* shell; we never
# speculatively recurse into unknown paths.

remove_fastembed_cache() {
    _cache="${FASTEMBED_CACHE_DIR:-$HOANGSA_INSTALL_DIR/cache/fastembed}"
    if [ ! -d "$_cache" ]; then
        info "no fastembed cache at $_cache"
        return 0
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
        info "dry-run: would rm -rf $_cache"
        return 0
    fi
    info "removing fastembed cache $_cache"
    rm -rf "$_cache"
    # Clean up the now-likely-empty $HOANGSA_INSTALL_DIR/cache parent.
    rmdir_if_empty "$HOANGSA_INSTALL_DIR/cache"
}

# --- 2. templates via manifest ---------------------------------------------
#
# $HOANGSA_INSTALL_DIR/manifest.json is always read from the global install
# root (even for --local) because that's where the CLI's
# `default_manifest_path` writes it (install.rs:510). The manifest's
# `files` map holds forward-slash paths relative to the Claude config dir
# (global) or project .claude dir (local).

MANIFEST_PATH="$HOANGSA_INSTALL_DIR/manifest.json"

remove_templates() {
    _dst_root="$1"
    if [ ! -f "$MANIFEST_PATH" ]; then
        warn "no manifest at $MANIFEST_PATH — skipping template removal (templates may have to be removed by hand)"
        return 0
    fi

    if ! command -v jq >/dev/null 2>&1; then
        warn "jq not found — cannot parse manifest; skipping template removal"
        return 0
    fi

    info "removing templates tracked in $MANIFEST_PATH"
    # Collect files + parent dirs. Parents handled after files so empty-dir
    # cleanup cascades. `jq -r` emits one path per line.
    _files=$(jq -r '.files | keys[]' "$MANIFEST_PATH" 2>/dev/null || true)
    if [ -z "$_files" ]; then
        info "manifest has no tracked files"
        return 0
    fi

    # Delete tracked files. Skip paths that escape $_dst_root — defensive
    # against a tampered manifest.
    printf '%s\n' "$_files" | while IFS= read -r _rel; do
        [ -z "$_rel" ] && continue
        case "$_rel" in
            /*|*..*) warn "skipping suspicious manifest entry: $_rel"; continue ;;
        esac
        rm_if_exists "$_dst_root/$_rel"
    done

    # Prune any directories the manifest's files lived in, deepest first.
    # `sort -r` on paths puts deeper paths before their parents.
    _dirs=$(printf '%s\n' "$_files" \
        | awk -F/ 'NF>1 { for (i=NF-1; i>=1; i--) { p=$1; for (j=2; j<=i; j++) p=p"/"$j; print p } }' \
        | sort -u \
        | sort -r)
    printf '%s\n' "$_dirs" | while IFS= read -r _rel; do
        [ -z "$_rel" ] && continue
        rmdir_if_empty "$_dst_root/$_rel"
    done

    rm_if_exists "$MANIFEST_PATH"
}

# --- 3. settings.json hooks -------------------------------------------------
#
# Strip every entry with "_hoangsa_managed: true" from every hook array,
# drop empty arrays, drop the "hooks" key if it's empty. Requires jq
# because round-tripping settings.json safely in awk/sed is a losing battle
# (order-preservation, escaping, trailing commas, etc.).

strip_hooks_jq='
  def prune_event: map(select((.["'"_hoangsa_managed"'"] // false) != true));
  if .hooks then
    .hooks |= with_entries(.value |= prune_event)
          |  with_entries(select(.value | length > 0))
  else . end
  | if (.hooks // {}) == {} then del(.hooks) else . end
'

strip_managed_hooks() {
    _settings="$1"
    if [ ! -f "$_settings" ]; then
        info "no settings.json at $_settings — nothing to strip"
        return 0
    fi
    if ! command -v jq >/dev/null 2>&1; then
        warn "jq not found — cannot strip managed hooks from $_settings (edit by hand or install jq)"
        return 0
    fi

    # Shape check first — install.sh treats non-object as empty, so we do the
    # same rather than crash the whole uninstall on a weird settings.json.
    if ! jq -e 'type == "object"' "$_settings" >/dev/null 2>&1; then
        warn "$_settings is not a JSON object — leaving alone"
        return 0
    fi

    if [ "$DRY_RUN" -eq 1 ]; then
        _before=$(jq '[.. | objects | select(.["'"_hoangsa_managed"'"] == true)] | length' "$_settings" 2>/dev/null || echo 0)
        info "dry-run: would strip $_before managed hook(s) from $_settings"
        return 0
    fi

    _tmp="$_settings.hoangsa.tmp.$$"
    if jq "$strip_hooks_jq" "$_settings" > "$_tmp" 2>/dev/null; then
        # Preserve the 2-space pretty format install.sh writes; jq's default
        # is 2 spaces already, just ensure trailing newline.
        printf '\n' >> "$_tmp"
        info "stripping managed hooks from $_settings"
        mv -f "$_tmp" "$_settings"
    else
        rm -f "$_tmp"
        warn "jq failed to rewrite $_settings — leaving original untouched"
    fi
}

# --- 4. MCP registration ---------------------------------------------------

strip_mcp_entry() {
    _mcp="$1"
    if [ ! -f "$_mcp" ]; then
        info "no MCP config at $_mcp — nothing to strip"
        return 0
    fi
    if ! command -v jq >/dev/null 2>&1; then
        warn "jq not found — cannot remove hoangsa-memory from $_mcp (edit by hand or install jq)"
        return 0
    fi
    if ! jq -e '.mcpServers["hoangsa-memory"]' "$_mcp" >/dev/null 2>&1; then
        info "no hoangsa-memory entry in $_mcp"
        return 0
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
        info "dry-run: would remove mcpServers.hoangsa-memory from $_mcp"
        return 0
    fi

    _tmp="$_mcp.hoangsa.tmp.$$"
    if jq 'del(.mcpServers["hoangsa-memory"])
           | if (.mcpServers // {}) == {} then del(.mcpServers) else . end' \
           "$_mcp" > "$_tmp" 2>/dev/null; then
        printf '\n' >> "$_tmp"
        info "removing hoangsa-memory from $_mcp"
        mv -f "$_tmp" "$_mcp"
    else
        rm -f "$_tmp"
        warn "jq failed to rewrite $_mcp — leaving original untouched"
    fi
}

# --- 5. rc file PATH block --------------------------------------------------

HOANGSA_MARK_START='# hoangsa:managed start'
HOANGSA_MARK_END='# hoangsa:managed end'

strip_managed_block() {
    _rc="$1"
    [ -f "$_rc" ] || return 0
    if ! grep -qF "$HOANGSA_MARK_START" "$_rc" 2>/dev/null; then
        return 0
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
        info "dry-run: would strip managed PATH block from $_rc"
        return 0
    fi

    _tmp="$_rc.hoangsa.tmp.$$"
    if awk -v s="$HOANGSA_MARK_START" -v e="$HOANGSA_MARK_END" '
        index($0, s) { flag=1; next }
        index($0, e) { flag=0; next }
        !flag        { print }
    ' "$_rc" > "$_tmp"; then
        info "stripping managed PATH block from $_rc"
        mv -f "$_tmp" "$_rc"
    else
        rm -f "$_tmp"
        warn "failed to rewrite $_rc — leaving original untouched"
    fi
}

strip_all_rc_files() {
    if [ "$HOANGSA_NO_PATH_EDIT" = "1" ]; then
        info "HOANGSA_NO_PATH_EDIT=1 — skipping rc file edits"
        return 0
    fi
    for _rc in \
        "$HOME/.zshrc" \
        "$HOME/.bashrc" \
        "$HOME/.bash_profile" \
        "$HOME/.config/fish/config.fish"
    do
        strip_managed_block "$_rc"
    done
}

# --- 6. purge (optional) ----------------------------------------------------
#
# Only touches $HOANGSA_INSTALL_DIR (typically ~/.hoangsa). NEVER touches
# per-project .hoangsa/ dirs — those live in user repos and carry session
# state + config that is independent of the global install.

purge_install_dir() {
    if [ ! -d "$HOANGSA_INSTALL_DIR" ]; then
        info "$HOANGSA_INSTALL_DIR already gone"
        return 0
    fi
    warn "--purge will delete $HOANGSA_INSTALL_DIR including memory/ and share/"
    if [ "$DRY_RUN" -eq 1 ]; then
        info "dry-run: would rm -rf $HOANGSA_INSTALL_DIR"
        return 0
    fi
    # Interactive confirm — --purge is destructive and irreversible. In
    # non-TTY contexts we still proceed (the user passed --purge explicitly
    # and may be scripting), but TTY users get one chance to back out.
    if [ -t 0 ] && [ -t 1 ]; then
        printf 'Delete %s? [y/N] ' "$HOANGSA_INSTALL_DIR"
        REPLY=""
        # shellcheck disable=SC2039
        read -r REPLY || REPLY=""
        case "$REPLY" in
            y*|Y*) ;;
            *) info "aborted purge (user declined)"; return 0 ;;
        esac
    fi
    act rm -rf "$HOANGSA_INSTALL_DIR"
}

# --- main -------------------------------------------------------------------

main() {
    ui_banner "uninstaller"
    info "mode: $MODE"
    [ "$DRY_RUN" -eq 1 ] && warn "DRY-RUN: no files will be changed"

    if [ "$MODE" = "global" ]; then
        section "target"
        pick_claude_dir
        _dst="$CLAUDE_DIR_PICK"
        _settings="$_dst/settings.json"
        _mcp="${CLAUDE_CONFIG_DIR:+$CLAUDE_CONFIG_DIR/.claude.json}"
        [ -z "$_mcp" ] && _mcp="$HOME/.claude.json"
    else
        _dst="$PWD/.claude"
        _settings="$_dst/settings.json"
        _mcp="$PWD/.mcp.json"
    fi

    section "templates"
    remove_templates "$_dst"
    section "settings"
    strip_managed_hooks "$_settings"
    section "mcp"
    strip_mcp_entry "$_mcp"
    section "binaries"
    remove_binaries
    if [ "$MODE" = "global" ]; then
        section "fastembed cache"
        remove_fastembed_cache
        section "PATH"
        strip_all_rc_files
    fi

    # Clean up now-empty bin dirs. $HOANGSA_CLI_DIR and $HOANGSA_INSTALL_DIR/bin
    # may be the same path — `rmdir_if_empty` is a no-op if already gone.
    rmdir_if_empty "$HOANGSA_CLI_DIR"
    rmdir_if_empty "$HOANGSA_INSTALL_DIR/bin"

    if [ "$PURGE" -eq 1 ]; then
        section "purge"
        purge_install_dir
    fi

    section "done"
    info "uninstall complete"
    if [ "$DRY_RUN" -eq 0 ] && [ "$MODE" = "global" ]; then
        info "open a new shell (or source your rc file) so PATH changes take effect"
    fi
}

main
