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

## Contract format

Core workflows (menu, prepare, cook, taste, fix) are **contracts, not
scripts**: Mission → Deliverables → Hard gates → Judgment notes → Escalation.
Anything under "Suggested flow" is guidance — choose the shortest path that
produces the deliverables. **Gates are the law**: a deliverable is done only
when its gate command passes; never claim completion past a failing gate.
User-interaction beats (AskUserQuestion blocks) and artifact templates are
part of the contract — don't skip or reshape them.

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
| `worker-rules/base.md` | Base worker rules; composed with addons by `hoangsa-cli rules compose` (used inside `hoangsa-cli envelope`). |

Reference them with `Read $HOANGSA_ROOT/workflows/<module>.md` at the step
that needs them — not at the top of the file.

---

## Worker skill registry

`hoangsa-cli envelope` inserts this block into every worker prompt (edit here, not in Rust):

```
Available skills — read the full SKILL.md only if relevant to your task:
- git-flow: Git branching, task switching, PR creation → ${CLAUDE_PLUGIN_ROOT}/skills/git-flow/SKILL.md
- visual-debug: Screenshot/video analysis for visual bugs → ${CLAUDE_PLUGIN_ROOT}/skills/visual-debug/SKILL.md
- fe-testing: FE verification loop — criteria, test layers, run-and-observe, mutation check → ${CLAUDE_PLUGIN_ROOT}/skills/fe-testing/SKILL.md

To use a skill: read_file("<path>") to get full instructions, then follow them.
Do NOT read skills unless your task specifically requires them.
```

---

## Media detection (menu Step 2f / fix Step 1b)

Scan two sources: (1) file paths or pasted screenshots/videos in the user's
input, (2) task-link attachments in `$SESSION_DIR/attachments/` (`ls` it if it
exists). Images: `.png .jpg .jpeg .webp .gif` — read natively, note paths as
visual context. Videos: `.mp4 .mov .webm .avi .mkv` — invoke the
`visual-debug` skill:

```bash
hoangsa-cli media check-ffmpeg
# Always quote the path; reject paths with shell metacharacters
hoangsa-cli media analyze "$VIDEO_PATH" --output-dir "/tmp/hoangsa-media-$(date +%s)"
```

Read the output `montage.png` (annotated frame grid) and `diff-montage.png`
(red overlay of frame changes); fold findings into the workflow's context.
No media → skip.

---

## Phase chaining (`chain_mode` preference)

When a workflow auto-chains to the next phase (cook → taste, taste → plate):

- `inline` (default) — continue in the current context. Cheapest for short
  sessions; a long design conversation rides along into every later phase.
- `fresh` — spawn the next phase as a fresh-context subagent whose prompt is
  just: "Read `$HOANGSA_ROOT/workflows/<phase>.md` and execute it for session
  `$SESSION_DIR`", then relay its report. The session dir (artifacts +
  state.json) is the complete handoff — same principle as fresh-context
  workers, applied at phase level. Use for long sessions where accumulated
  context taxes every subsequent phase.

`pref get . chain_mode` — unset means `inline`.

---

## Lesson injection (workflows that spawn workers)

`hoangsa-cli envelope` already injects LESSONS.md entries keyword-matched to
the task. For workers spawned WITHOUT envelope (analyzers, research agents),
optionally recall relevant lessons yourself:
`memory_recall({query: "<task summary>", scope: "curated", top_k: 5, log_event: false})`
and include matching lessons verbatim; no matches → omit.
