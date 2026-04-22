# hoangsa installer UI library — POSIX sh, sourced by install.sh /
# install-local.sh / uninstall.sh. At release time this file is inlined
# into dist/install.sh by .github/workflows/release.yml so the curl|sh
# endpoint stays a single self-contained file.
#
# Components (in dependency order, no forward refs):
#   [C1] Capabilities — detect TTY + env gates, set _UI_COLOR/_UI_UNICODE/_UI_QUIET
#   [C2] Palette      — assign c_*/ic_* from flags (called from ui_init)
#   [C3] Banner       — ui_banner "<version>"
#   [C4] Steps        — step_info / step_ok / step_fail / step_skip
#   [C5] Messages     — info / warn / err / section / rule
#   [C6] JSON         — json_get / render_summary / render_dry_run / render_generic
#
# Env knobs (all optional):
#   NO_COLOR              disable ANSI colors (spec: https://no-color.org/)
#   HOANGSA_PLAIN=1       disable ANSI colors AND unicode icons (ASCII only)
#   HOANGSA_QUIET=1       suppress the banner (steps still render)
#   HOANGSA_FORCE_COLOR=1 force ANSI on non-TTY (useful in CI logs)

# ── [C1] Capabilities ──────────────────────────────────────────────
_UI_COLOR=0; _UI_UNICODE=0; _UI_QUIET=0
ui_init() {
    _UI_COLOR=0; _UI_UNICODE=0; _UI_QUIET=0
    [ "${HOANGSA_QUIET:-}" = "1" ] && _UI_QUIET=1
    if [ "${HOANGSA_FORCE_COLOR:-}" = "1" ]; then
        _UI_COLOR=1
    elif [ -n "${NO_COLOR:-}" ] || [ "${HOANGSA_PLAIN:-}" = "1" ]; then
        _UI_COLOR=0
    elif [ -t 1 ]; then
        _UI_COLOR=1
    fi
    if [ "${HOANGSA_PLAIN:-}" = "1" ]; then
        _UI_UNICODE=0
    else
        case "${LC_ALL:-${LC_CTYPE:-${LANG:-}}}" in
            *UTF-8*|*utf-8*|*UTF8*|*utf8*) _UI_UNICODE=1 ;;
        esac
        [ "${HOANGSA_FORCE_COLOR:-}" = "1" ] && _UI_UNICODE=1
    fi
    ui_palette
}

# ── [C2] Palette ───────────────────────────────────────────────────
c_reset=""; c_dim=""; c_red=""; c_green=""; c_yellow=""; c_cyan=""; c_bold=""
ic_ok="+"; ic_fail="x"; ic_warn="!"; ic_skip="-"; ic_arrow=">"
ui_palette() {
    if [ "$_UI_COLOR" = 1 ]; then
        c_reset=$(printf '\033[0m')
        c_dim=$(printf '\033[2m')
        c_red=$(printf '\033[31m')
        c_green=$(printf '\033[32m')
        c_yellow=$(printf '\033[33m')
        c_cyan=$(printf '\033[36m')
        c_bold=$(printf '\033[1m')
    else
        c_reset=""; c_dim=""; c_red=""; c_green=""; c_yellow=""; c_cyan=""; c_bold=""
    fi
    if [ "$_UI_UNICODE" = 1 ]; then
        ic_ok="✓"; ic_fail="✗"; ic_warn="⚠"; ic_skip="•"; ic_arrow="→"
    else
        ic_ok="+"; ic_fail="x"; ic_warn="!"; ic_skip="-"; ic_arrow=">"
    fi
}

# ── [C3] Banner ────────────────────────────────────────────────────
ui_banner() {
    [ "$_UI_QUIET" = 1 ] && return 0
    _ver="${1:-}"; _sub="${2:-claude code harness toolkit}"
    printf '\n'
    if [ "$_UI_UNICODE" = 1 ]; then
        printf '%s  ██╗  ██╗ ██████╗  █████╗ ███╗   ██╗ ██████╗ ███████╗ █████╗ %s\n' "$c_cyan" "$c_reset"
        printf '%s  ██║  ██║██╔═══██╗██╔══██╗████╗  ██║██╔════╝ ██╔════╝██╔══██╗%s\n' "$c_cyan" "$c_reset"
        printf '%s  ███████║██║   ██║███████║██╔██╗ ██║██║  ███╗███████╗███████║%s\n' "$c_cyan" "$c_reset"
        printf '%s  ██╔══██║██║   ██║██╔══██║██║╚██╗██║██║   ██║╚════██║██╔══██║%s\n' "$c_cyan" "$c_reset"
        printf '%s  ██║  ██║╚██████╔╝██║  ██║██║ ╚████║╚██████╔╝███████║██║  ██║%s\n' "$c_cyan" "$c_reset"
        printf '%s  ╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═══╝ ╚═════╝ ╚══════╝╚═╝  ╚═╝%s\n' "$c_cyan" "$c_reset"
    else
        cat <<'__HOANGSA_ASCII__'
   _  _  ___    _    _  _  ___  ___    _
  | || |/ _ \  /_\  | \| |/ __|/ __|  /_\
  | __ | (_) |/ _ \ | .' | (_ |\__ \ / _ \
  |_||_|\___//_/ \_\|_|\_|\___||___//_/ \_\
__HOANGSA_ASCII__
    fi
    if [ -n "$_ver" ]; then
        printf '  %shoangsa %s%s%s %s— %s%s\n\n' \
            "$c_dim" "$c_bold" "$_ver" "$c_reset" "$c_dim" "$_sub" "$c_reset"
    else
        printf '  %s%s%s\n\n' "$c_dim" "$_sub" "$c_reset"
    fi
}

# ── [C4] Steps ─────────────────────────────────────────────────────
step_info() { printf '  %s%s%s %s\n' "$c_cyan" "$ic_arrow" "$c_reset" "$*"; }
step_ok() {
    _lbl="$1"; _note="${2:-}"
    if [ -n "$_note" ]; then
        printf '  %s%s%s %s  %s%s%s\n' "$c_green" "$ic_ok" "$c_reset" "$_lbl" "$c_dim" "$_note" "$c_reset"
    else
        printf '  %s%s%s %s\n' "$c_green" "$ic_ok" "$c_reset" "$_lbl"
    fi
}
step_fail() {
    _lbl="$1"; _note="${2:-}"
    if [ -n "$_note" ]; then
        printf '  %s%s%s %s  %s%s%s\n' "$c_red" "$ic_fail" "$c_reset" "$_lbl" "$c_red" "$_note" "$c_reset" >&2
    else
        printf '  %s%s%s %s\n' "$c_red" "$ic_fail" "$c_reset" "$_lbl" >&2
    fi
}
step_skip() {
    _lbl="$1"; _note="${2:-}"
    if [ -n "$_note" ]; then
        printf '  %s%s %s  %s(%s)%s\n' "$c_dim" "$ic_skip" "$_lbl" "$c_dim" "$_note" "$c_reset"
    else
        printf '  %s%s %s%s\n' "$c_dim" "$ic_skip" "$_lbl" "$c_reset"
    fi
}

# ── [C5] Messages ──────────────────────────────────────────────────
info() { printf '  %s%s%s %s\n' "$c_cyan" "$ic_arrow" "$c_reset" "$*"; }
warn() { printf '  %s%s warning:%s %s\n' "$c_yellow" "$ic_warn" "$c_reset" "$*" >&2; }
err()  { printf '  %s%s%s error:%s %s\n' "$c_red" "$c_bold$ic_fail" "$c_reset" "$c_reset" "$*" >&2; }
section() {
    _title="$*"
    if [ "$_UI_UNICODE" = 1 ]; then
        printf '\n  %s── %s%s%s %s────────────────────────────%s\n' \
            "$c_cyan" "$c_bold" "$_title" "$c_reset" "$c_cyan" "$c_reset"
    else
        printf '\n  %s-- %s%s%s %s----------------------------%s\n' \
            "$c_cyan" "$c_bold" "$_title" "$c_reset" "$c_cyan" "$c_reset"
    fi
}
rule() {
    if [ "$_UI_UNICODE" = 1 ]; then
        printf '  %s──────────────────────────────────%s\n' "$c_dim" "$c_reset"
    else
        printf '  %s----------------------------------%s\n' "$c_dim" "$c_reset"
    fi
}

# ── [C6] JSON ──────────────────────────────────────────────────────
#
# json_get: extract a scalar field from JSON. Accepts jq-style paths.
# With jq present: any path (`.a.b.c`). Awk fallback: only top-level
# flat keys (`.key`). Missing paths / parse errors → empty string.
json_get() {
    _path="$1"; _json="$2"
    if command -v jq >/dev/null 2>&1; then
        printf '%s' "$_json" | jq -r "$_path // empty" 2>/dev/null
        return
    fi
    _key="${_path#.}"
    case "$_key" in
        *.*|*\[*) return ;;  # awk fallback is flat-only
    esac
    printf '%s' "$_json" | awk -v k="\"$_key\"" '
        { buf = buf $0 "\n" }
        END {
            n = index(buf, k)
            if (n == 0) exit
            rest = substr(buf, n + length(k))
            sub(/^[[:space:]]*:[[:space:]]*/, "", rest)
            if (substr(rest, 1, 1) == "\"") {
                rest = substr(rest, 2)
                end = index(rest, "\"")
                while (end > 1 && substr(rest, end-1, 1) == "\\") {
                    nxt = index(substr(rest, end+1), "\"")
                    if (nxt == 0) break
                    end = end + nxt
                }
                print substr(rest, 1, end-1)
            } else {
                if (match(rest, /[,}\n]/)) print substr(rest, 1, RSTART-1)
                else print rest
            }
        }
    '
}

# render_summary: pretty-print the hoangsa-cli final install JSON.
render_summary() {
    _json="$1"
    if [ -z "$_json" ]; then
        warn "no CLI output captured"
        return
    fi
    section "install summary"
    _status=$(json_get '.status' "$_json")
    _mode=$(json_get '.mode' "$_json")
    _copied=$(json_get '.copied' "$_json")
    _skipped=$(json_get '.skipped' "$_json")
    _backups=$(json_get '.backups' "$_json")
    _mcp=$(json_get '.mcp_target' "$_json")
    _manifest=$(json_get '.manifest' "$_json")
    _settings=$(json_get '.settings' "$_json")
    _hooks=$(json_get '.hooks_added' "$_json")
    _stline=$(json_get '.statusline_set' "$_json")

    case "$_status" in
        ok)
            printf '  %s%s%s status:     %s%s%s\n' \
                "$c_green" "$ic_ok" "$c_reset" "$c_green" "$_status" "$c_reset" ;;
        partial)
            printf '  %s%s%s status:     %s%s%s\n' \
                "$c_yellow" "$ic_warn" "$c_reset" "$c_yellow" "$_status" "$c_reset" ;;
        '')
            render_generic "$_json"
            return ;;
        *)
            printf '  %s%s%s status:     %s\n' "$c_red" "$ic_fail" "$c_reset" "$_status" ;;
    esac
    [ -n "$_mode" ]     && printf '    %smode:%s       %s\n' "$c_dim" "$c_reset" "$_mode"
    [ -n "$_copied" ]   && printf '    %sfiles:%s      %s copied, %s skipped, %s backups\n' \
        "$c_dim" "$c_reset" "$_copied" "${_skipped:-0}" "${_backups:-0}"
    [ -n "$_manifest" ] && printf '    %smanifest:%s   %s\n' "$c_dim" "$c_reset" "$_manifest"
    [ -n "$_settings" ] && printf '    %ssettings:%s   %s\n' "$c_dim" "$c_reset" "$_settings"
    [ -n "$_mcp" ]      && printf '    %smcp:%s        %s\n' "$c_dim" "$c_reset" "$_mcp"
    [ -n "$_hooks" ]    && printf '    %shooks added:%s %s\n' "$c_dim" "$c_reset" "$_hooks"
    [ -n "$_stline" ]   && printf '    %sstatusline:%s  %s\n' "$c_dim" "$c_reset" "$_stline"

    if command -v jq >/dev/null 2>&1; then
        _warns=$(printf '%s' "$_json" | jq -r '.warnings[]?' 2>/dev/null)
        if [ -n "$_warns" ]; then
            printf '\n  %swarnings:%s\n' "$c_yellow" "$c_reset"
            printf '%s\n' "$_warns" | while IFS= read -r _w; do
                [ -z "$_w" ] && continue
                printf '    %s%s%s %s\n' "$c_yellow" "$ic_warn" "$c_reset" "$_w"
            done
        fi
    fi
    printf '\n'
}

# render_dry_run: pretty-print the hoangsa-cli --dry-run JSON preview.
render_dry_run() {
    _json="$1"
    if [ -z "$_json" ]; then
        warn "no CLI output captured"
        return
    fi
    section "dry-run preview"
    _mode=$(json_get '.mode' "$_json")
    printf '  %smode:%s %s\n' "$c_dim" "$c_reset" "$_mode"

    if command -v jq >/dev/null 2>&1; then
        _count=$(printf '%s' "$_json" | jq -r '.actions | length' 2>/dev/null)
        if [ -z "$_count" ] || [ "$_count" = "0" ]; then
            printf '\n  %sno actions planned%s\n' "$c_dim" "$c_reset"
        else
            printf '\n  %s%s action(s) planned:%s\n' "$c_bold" "$_count" "$c_reset"
            printf '%s\n' "$_json" | jq -r '
                .actions[]
                | "    \(.action // "?")  \(.src // "")\(if .dst then "  → \(.dst)" else "" end)"
            ' 2>/dev/null
        fi
        _warns=$(printf '%s' "$_json" | jq -r '.warnings[]?' 2>/dev/null)
        if [ -n "$_warns" ]; then
            printf '\n  %swarnings:%s\n' "$c_yellow" "$c_reset"
            printf '%s\n' "$_warns" | while IFS= read -r _w; do
                [ -z "$_w" ] && continue
                printf '    %s%s%s %s\n' "$c_yellow" "$ic_warn" "$c_reset" "$_w"
            done
        fi
    else
        printf '\n  %s(jq unavailable — showing raw JSON)%s\n' "$c_dim" "$c_reset"
        printf '%s\n' "$_json"
    fi
    printf '\n'
}

# render_generic: fallback when JSON is empty/malformed/unknown shape.
render_generic() {
    _json="$1"
    section "raw output"
    printf '%s\n\n' "$_json"
}

# Auto-init at source time. Scripts may call ui_init again after mutating env.
ui_init
