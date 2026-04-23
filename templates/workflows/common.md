# Common — Shared Workflow Scaffolding

Referenced by every HOANGSA workflow. Not a standalone workflow.

Workflows load this once at Step 0 (boot) so their per-file token cost drops
and the shared rules live in one canonical place.

---

## Boot preamble

Every workflow resolves its install path from the slash command, then runs
`hoangsa-cli` via `$HOANGSA_ROOT/bin/hoangsa-cli`. The heavy lifts below
are the only CLI calls a workflow needs to know about — deeper subcommands
are documented inline where they're used.

```bash
SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session latest)       # {found, id, dir, files}
STATE=$("$HOANGSA_ROOT/bin/hoangsa-cli" state get "$SESSION_DIR")
LANG_PREF=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . lang | python3 -c "import sys,json;print(json.load(sys.stdin).get('value','en'))")
"$HOANGSA_ROOT/bin/hoangsa-cli" state update "$SESSION_DIR" '{"status":"..."}'
"$HOANGSA_ROOT/bin/hoangsa-cli" pref set . <key> <value>
```

### Pre-built context bundle (optional, cheap)

If `$SESSION_DIR/ctx.md` exists, read it **once** at Step 0 and skip the
scattered `state get` / `git status` / `config get` calls that would
otherwise be repeated throughout the workflow. Populated by:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" ctx <workflow>
```

Absence is a no-op — workflows fall back to their existing boot sequence.

---

## Universal rules

These apply to **every** workflow. Do not restate them in per-workflow
Rules tables — list only workflow-specific rules there.

| Rule | Detail |
|------|--------|
| **AskUserQuestion for all interactions** | Every user-facing question uses AskUserQuestion — no plain text prompts. |
| **Save preferences on first ask** | Ask once, persist to `.hoangsa/config.json`, never ask again. |
| **Stop when user asks** | Immediately. No "one more step". |
| **Respect `$LANG_PREF`** | All user-visible text in the language configured in config preferences. |
| **Fresh context is sacred** | Never fold worker state into orchestrator context, and vice versa. |

---

## Self-verification template

Workflows that declare a checklist output this table before Step N (final
report). Replace the rows with the workflow's actual steps.

```
| Step | Status |
|------|--------|
| 0. Boot (common.md + lang + session) | DONE / SKIPPED |
| 1. <workflow step 1> | DONE / SKIPPED |
| ... | DONE / SKIPPED |
| N. <final step> | DONE / SKIPPED |
```

Any `SKIPPED` row that was not explicitly approved by the user means the
workflow is not done — go back and complete it before reporting.

---

## Shared modules

| Module | Purpose |
|--------|---------|
| `git-context.md` | Branch detection, dirty-state handling, stash recovery, post-commit PR flow. |
| `task-link.md` | External task link detection (Linear/Jira/ClickUp/…), attachment download, sync-back chain. |
| `worker-rules.md` | Worker-rules composition (middleware chain, addon priorities, gates). |

Reference them with `Read $HOANGSA_ROOT/workflows/<module>.md` at the step
that needs them — not at the top of the file.
