# HOANGSA Brainstorm Workflow

> **Boot:** Read `$HOANGSA_ROOT/workflows/common.md` first — universal rules + CLI reference + self-verification template.

Turn a vague idea into a validated design through collaborative dialogue, before any spec or code.

**Principles:** One question at a time. Explore before committing. YAGNI ruthlessly. User always has final say.

> **MUST complete ALL steps in order. DO NOT skip any step. DO NOT stop before Step 7.**
>
> 1. Init session → 2. Explore context → 3. Clarify intent → 4. Propose approaches → 5. Present design (incl. failure/concurrency + open-questions round) → 6. Write BRAINSTORM.md → 7. Complete + chain

---

---

## Step 1: Init session

### 1a. Get the idea (always ask — this changes every time)

Use AskUserQuestion:
  question: "Bạn muốn brainstorm ý tưởng gì?"
  header: "Ý tưởng"
  options:
    - label: "Tôi sẽ mô tả", description: "Gõ ý tưởng vào ô 'Other' bên dưới"
    - label: "Xem ví dụ trước", description: "Xem ví dụ ý tưởng trước khi tự viết"
  multiSelect: false

If user chose "Xem ví dụ trước" → show examples such as:
  - "Thêm authentication cho API"
  - "Tối ưu performance dashboard"
  - "Build CLI tool quản lý tasks"
  - "Redesign landing page"
Then ask again.

If user typed their idea → store as `IDEA`.

### 1b. Create session

Auto-derive slug from the user's idea (2-4 key words, hyphenated):

```bash
SESSION=$("$HOANGSA_BIN" session init brainstorm "$SLUG")
```

Extract `SESSION_ID`, `SESSION_DIR` from JSON output.

```bash
"$HOANGSA_BIN" state init "$SESSION_DIR"
```

---

## Step 2: Explore project context

<HARD-GATE>
Do NOT write any code, create any spec, or take any implementation action until you have presented a design and the user has approved it. This applies to EVERY idea regardless of perceived simplicity.
</HARD-GATE>

### 2a. Research detection (auto)

Check if a prior `/hoangsa:research` session produced a `RESEARCH.md` relevant to this idea.

```bash
RESEARCH_SESSION=$("$HOANGSA_BIN" session latest)
```

Parse the result — if `type` is `"docs"` and `files` contains `"RESEARCH.md"`:

1. Read `RESEARCH.md` from the research session directory
2. Extract relevant findings:
   - **Patterns & conventions** → inform approach proposals in Step 4
   - **Dependencies & limitations** → surface as constraints in Step 3
   - **Key findings** → use as grounding for design in Step 5
   - **Risks & concerns** → carry into BRAINSTORM.md Decisions/Open Questions
3. Show the user:

```
📚 Research detected: <research session id>
   Topic:    <topic>
   Sections: <N>

   Dùng research này làm context cho brainstorm.
```

4. Store as `RESEARCH_CONTEXT` for use in Steps 3-5.

**If no research session found:** Skip — codebase scan in 2b handles basic context.

### 2b. Codebase scan

**If MEMORY_AVAILABLE:**
- Run `memory_recall({query: "<IDEA>"})` to find relevant code, flows, and patterns
- For top symbols found → run `memory_symbol_context({name: "<symbol>"})` to understand structure
- Search past conversations for prior brainstorms on similar topics:
  `memory_archive_search({query: "<IDEA>"})`
  If relevant past brainstorms found → surface to user: "You discussed something similar in a past session."

**If MEMORY_NOT_INSTALLED:**
- Use Glob to find project entry points (`index.*`, `main.*`, `app.*`, `server.*`)
- Use Grep to find files referencing the idea's keywords
- Read manifest files (`package.json`, `Cargo.toml`, etc.) for tech stack

### 2c. Recent history

```bash
git log --oneline -10
```

Check recent commits to understand current development direction.

### 2d. Scope check

Before asking detailed questions, assess scope: if the idea describes multiple independent subsystems (e.g., "build a platform with chat, file storage, billing, and analytics"), flag this immediately.

If too large for a single design → help the user decompose into sub-ideas. Each sub-idea gets its own brainstorm → menu → prepare → cook cycle.

---

## Step 3: Clarify intent

Ask clarifying questions **one at a time** using AskUserQuestion. Prefer multiple-choice when possible.

### 3a. Research trigger

During clarifying questions, watch for signals that research is needed:
- User explicitly says "research", "tìm hiểu", "best practice", "how do others do this"
- The idea involves technology or patterns not in the current codebase
- The approach requires evaluating external libraries, services, or standards
- No `RESEARCH_CONTEXT` was loaded in Step 2a and the topic is non-trivial

**When triggered — run research inline, no extra user action needed:**

1. Notify user: `"🔍 Đang research <topic>..."`
2. Delegate to the research workflow internally — spawn parallel research agents (same as `/hoangsa:research`) with scope "both", mode "auto", topic derived from what triggered the research
3. Wait for research to complete → load results as `RESEARCH_CONTEXT`
4. Save RESEARCH.md to `$SESSION_DIR/RESEARCH.md`
5. Show summary of key findings to user
6. Resume clarifying questions with research findings as context

Research runs as part of the brainstorm flow — user never leaves the conversation.

### 3b. Clarifying questions

Focus on understanding:
- **Purpose** — why does this need to exist? What problem does it solve?
- **Users** — who will use this? What are their expectations?
- **Constraints** — performance, security, compatibility, timeline
- **Success criteria** — how do we know this is done and working?
- **Existing patterns** — from Step 2 + RESEARCH_CONTEXT, does the codebase or ecosystem already have patterns we should follow?

**Adapt depth to complexity:**
- Simple idea (config change, small util) → 1-2 questions
- Medium idea (new feature, refactor) → 3-4 questions
- Complex idea (new subsystem, architecture change) → 4-6 questions

### 3c. The unknown ledger

Keep a running list of everything you don't know yet. Each entry ends in exactly
one of three states:

- **Answered from context** — codebase scan (Step 2b), research, or git history. Cite where.
- **Answered by the user** — you asked.
- **Assumption** — you filled the gap yourself. Then it is *not* settled: it goes
  into the BRAINSTORM.md `## Open Questions` table with the assumption you made and
  what breaks if it's wrong, and it gets surfaced in Step 5c.

**Question budget:** the 1-6 count above bounds *exploratory* questions —
nice-to-know context. A question is **blocking** when two plausible answers lead
to two different designs (who owns the data, is it single- or multi-writer, must
it work offline, is the old format still in the wild). Blocking questions are
asked regardless of the budget; running out of budget is never a reason to guess.
After 6 exploratory questions, stop exploring — do not stop asking blocking ones.

---

## Step 4: Propose approaches

Present 2-3 different approaches with trade-offs. Use AskUserQuestion:

  question: "Bạn thích approach nào?"
  header: "Approach"
  options: (2-3 options, each with preview showing the approach)
    - label: "<Approach A> (Recommended)", description: "<rationale and trade-offs>"
    - label: "<Approach B>", description: "<rationale and trade-offs>"
    - label: "<Approach C>", description: "<rationale and trade-offs>"
  multiSelect: false

For each approach, briefly cover:
- Architecture / structure
- Key components
- Main trade-off (speed vs flexibility, simple vs powerful, etc.)

Lead with your recommended option and explain why.

If the user picks Other → incorporate their direction into the design.

If the user's choice reveals a strong design preference (e.g., "prefers composition over inheritance", "no ORMs", "always REST over GraphQL"):

```
memory_remember_preference({text: "<preference derived from user's choice>"})
```

This persists across projects so future brainstorms can surface relevant preferences.

---

## Step 5: Present design

Present the design in sections, scaled to complexity. After each major section, check with the user.

### 5a. Sections to cover (adapt per idea)

| Section | When to include | Scale |
|---------|----------------|-------|
| **Architecture** | Always | 1-3 paragraphs |
| **Components** | When >1 module involved | List with one-line descriptions |
| **Data flow** | When data moves between components | Short description or diagram |
| **API / Interface** | When there's a public interface | Key signatures, not exhaustive |
| **Error handling** | When failure modes matter | List of scenarios + strategy |
| **Failure & concurrency** | Whenever the idea writes shared state (file, DB row, cache, global, session dir) | See 5a-2 — required, not optional |
| **Testing strategy** | Always | 1-2 sentences |
| **Migration / rollout** | When changing existing behavior | Steps + rollback plan |

### 5a-2. Failure & concurrency (risk seeds)

The design is not presentable until you've said something concrete about how it
breaks. Cover, in one or two lines each — these become the **Risk Seeds** that
seed the menu-phase Risk Sweep:

- **Shared state** — what does this read and then write? Who else touches it?
- **Concurrency / TOCTOU** — for every check-then-act (does the file exist? is the
  row free? is the lock held?), what happens if another actor changes it between
  the check and the act? Name the guard: lock, transaction, atomic temp+rename,
  CAS, unique index, single-writer.
- **Retry / idempotency** — the operation runs twice: same result, or duplicate?
- **Partial failure** — it dies halfway: what's left behind, who cleans it up?

If the honest answer is "no shared state, pure computation" — say that, once.
Unknowns here are blocking questions (Step 3c), not assumptions.

### 5b. Section review

After presenting all sections, use AskUserQuestion:

  question: "Design có OK không?"
  header: "Review"
  options:
    - label: "OK — viết lên file", description: "Design ổn, lưu thành BRAINSTORM.md"
    - label: "Cần sửa", description: "Có điểm cần chỉnh — ghi chi tiết vào Other"
    - label: "Quay lại approach", description: "Muốn chọn approach khác"
  multiSelect: false

If "OK" → proceed to Step 5c.
If "Cần sửa" → apply fixes, re-present affected sections, re-ask.
If "Quay lại approach" → go back to Step 4.

### 5c. Open-questions round (blocking)

Before writing the file, walk the unknown ledger (3c) and pull out every entry
still marked *assumption*, plus anything in the design you wrote as "TBD",
"probably", or "for now". Nothing left → say `✅ Không còn open question` and go
to Step 6.

Otherwise ask — the whole point of brainstorming is that these are cheap here and
expensive in cook. One AskUserQuestion per question, blocking ones first:

Use AskUserQuestion:
  question: "<the open question, concretely>"
  header: "<topic, max 12 chars>"
  options:
    - label: "<Answer A>", description: "<what it makes true — and the cost>"
    - label: "<Answer B>", description: "<...>"
    - label: "Để menu quyết", description: "Chuyển sang menu với giả định: <assumption>. Sai thì: <impact>"
  multiSelect: false

Every question gets a row in `## Open Questions` with status `RESOLVED` (answer
recorded) or `DEFERRED` (assumption + impact recorded, menu Step 2d picks it up).
A question that was never asked has no valid status — writing the file without
this round is a skipped step, not a shortcut.

---

## Step 6: Write BRAINSTORM.md

Save to `$SESSION_DIR/BRAINSTORM.md`:

```markdown
---
brainstorm_version: "1.0"
idea: "<original idea from user>"
approach: "<chosen approach name>"
research_ref: "<path to RESEARCH.md if used, or null>"
status: "approved"
---

# Brainstorm: <Title>

## Idea
<Original idea — verbatim from user>

## Context
<What exists in the codebase today that's relevant>
<Recent development direction from git history>

## Research Summary
<If RESEARCH_CONTEXT was used — key findings that informed this design>
<If no research — omit this section entirely>

## Chosen Approach
### <Approach name>
<Description of the chosen approach>

### Why this approach
<Rationale — why this over the alternatives>

### Alternatives considered
| Approach | Pros | Cons | Why not |
|----------|------|------|---------|
| <Alt A> | ... | ... | ... |
| <Alt B> | ... | ... | ... |

## Design

### Architecture
<Architecture description>

### Components
<Component list with responsibilities>

### Data Flow
<How data moves through the system>

### Interfaces
<Key public interfaces / API surface>

### Error Handling
<Failure scenarios and strategies>

### Failure & Concurrency
<From Step 5a-2 — shared state, TOCTOU windows + guards, retry/idempotency, partial failure>

### Testing Strategy
<How to verify this works>

## Decisions
| # | Decision | Reasoning | Type |
|---|----------|-----------|------|
| 1 | ... | ... | LOCKED |
| 2 | ... | ... | FLEXIBLE |

## Risk Seeds
<!-- Feeds the menu-phase Risk Sweep (menu Step 3d-2). One line per risk you already
     know about, with the class it belongs to. Unknown ≠ absent: write what you'd have
     to check. Omit only for pure-computation ideas with no shared state. -->
| Risk class | What we know so far |
|------------|---------------------|
| Concurrency & TOCTOU | ... |
| Idempotency & retry | ... |
| Partial failure | ... |
| Backward compat / migration | ... |

## Open Questions
<!-- Every row was surfaced to the user in Step 5c. RESOLVED = answered here.
     DEFERRED = the user chose to decide in menu; record the assumption we proceed
     under and what it costs if wrong. No row may be statusless — an unasked
     question is not an open question, it's a guess. -->
| Question | Status | Answer / Assumption | Impact if wrong |
|----------|--------|---------------------|-----------------|
| <question> | RESOLVED / DEFERRED | ... | ... |

## Out of Scope
<What we explicitly decided NOT to do>
```

Omit sections that are not applicable. A simple idea should produce a short document.

### 6b. Self-review

After writing, scan the document for:
1. **Placeholders** — any "TBD", "TODO", incomplete sections? Fix them.
2. **Contradictions** — do any sections conflict? Resolve.
3. **Scope creep** — did we add things the user didn't ask for? Remove.
4. **Ambiguity** — could any part be interpreted two ways? Clarify.
5. **Statusless open questions** — any row without RESOLVED/DEFERRED means Step 5c
   was skipped. Go back and ask; do not backfill a status yourself.

Fix issues inline. No need to re-present to user.

Append locked architectural decisions to the DESIGN-SPEC's Architecture section during the menu workflow — BRAINSTORM.md is the source of truth for design rationale.

---

## Step 7: Complete + chain

```bash
"$HOANGSA_BIN" state update "$SESSION_ID" '{"status":"brainstorm"}'
```

```bash
"$HOANGSA_BIN" commit \
  "brainstorm(<scope>): design for <title>" \
  --files "$SESSION_DIR/BRAINSTORM.md"
```

Report:

```
✅ Brainstorm complete!
   Session:   <session-id>
   Idea:      <idea summary>
   Approach:  <chosen approach>
   Output:    <path to BRAINSTORM.md>

Next steps:
   - /hoangsa:menu  — turn this design into DESIGN-SPEC + TEST-SPEC
     (menu will auto-detect BRAINSTORM.md and use it as context)
```

---

## Self-verification checklist

Before Step 7, emit the `common.md` self-verification table with rows:

```
| 0. Setup (lang + hoangsa-memory) | ... |
| 1. Init session + idea | ... |
| 2. Explore context | ... |
| 3. Clarify intent (unknown ledger) | ... |
| 4. Propose approaches | ... |
| 5. Present design (incl. failure & concurrency) | ... |
| 5c. Open-questions round (asked, not assumed) | ... |
| 6. Write BRAINSTORM.md | ... |
| 7. Complete + chain | ... |
```

---

## Rules

Universal rules live in `common.md §Universal rules`. Brainstorm-specific additions:

| Rule | Detail |
|------|--------|
| **One question at a time** | Don't overwhelm — AskUserQuestion, one per message |
| **YAGNI ruthlessly** | Remove features user didn't ask for |
| **≥2 approaches** | Always propose alternatives before settling |
| **No code before approval** | HARD-GATE — design first, implement never (that's menu/cook's job) |
| **Scale to complexity** | Simple idea = short doc, complex = detailed doc |
| **Max 6 exploratory questions** | The budget bounds exploration only — blocking questions (two answers ⇒ two designs) are asked regardless |
| **No silent assumptions** | An assumption is a DEFERRED open question with an impact line, surfaced in Step 5c — never an unwritten guess |
| **Name the failure modes** | Shared state, TOCTOU, retry, partial failure — answered in 5a-2, carried out as Risk Seeds |
| **Chain to menu** | Terminal state is suggesting `/hoangsa:menu` |
| **Structured output** | BRAINSTORM.md has frontmatter + sections that menu can parse |
