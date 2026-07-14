# HOANGSA Menu — Contract

> **Boot:** Read `$HOANGSA_ROOT/workflows/common.md` first — universal rules, contract format, CLI reference, self-verification template.

## Mission

Take the user from an idea to validated, user-approved `DESIGN-SPEC.md` + `TEST-SPEC.md`, ready for `/hoangsa:prepare`. You are the design lead: surface real decisions with real trade-offs (≥3 options where the choice matters, one question at a time), and let the user have the final say — but don't manufacture ceremony for tasks that carry no decisions.

## Deliverables

`$SESSION_DIR/`: `CONTEXT.md`, `RESEARCH.md`, `DESIGN-SPEC.md`, `TEST-SPEC.md` — saved, committed (`menu(<scope>): complete spec for <component>`), state → `design`.

## Hard gates

| # | Gate | Check |
|---|------|-------|
| 1 | Spec valid | `validate spec "$SESSION_DIR/DESIGN-SPEC.md"` |
| 2 | Tests valid | `validate tests "$SESSION_DIR/TEST-SPEC.md"` (enforces Edge Cases non-empty; `surface: ui\|api\|cli` ⇒ `## E2E Tests`; `ui` ⇒ `## Visual Verification`) |
| 3 | Cross-check | TEST-SPEC `component` == DESIGN-SPEC `component`; every `[REQ-xx]` in TEST-SPEC exists in DESIGN-SPEC |
| 4 | User approval | user approved both documents (review loop below — revisable indefinitely) |

## Express lane — triage BEFORE any ceremony

When the request is plainly trivial — single known file, no new user-facing surface, no design decision worth discussing (config tweak, copy change, obvious small bugfix, dependency bump) — offer:

Use AskUserQuestion:
  question: "Task này nhỏ — đi express lane?"
  header: "Express"
  options:
    - label: "Express (Recommended)", description: "Bỏ vòng hỏi đáp: spec tối giản + plan 1-2 task, gates giữ nguyên — ra cook nhanh nhất"
    - label: "Full menu", description: "Đi đủ discussion → design spec → test spec — cho task cần bàn"
  multiSelect: false

**Express path** (all gates still apply — express cuts questions, never quality):
1. Create session (§Session creation below) + git context.
2. Write a minimal DESIGN-SPEC (frontmatter + Overview with 1–3 REQs + Acceptance Criteria) and TEST-SPEC (frontmatter with correct `category`/`surface`; Unit Tests and/or E2E per surface; `## Edge Cases` with ≥1 real row or explicit waiver; `## Visual Verification` when `surface: ui`) — Gates 1–3 must pass.
3. Write `plan.json` directly (schema: `prepare.md §plan.json schema` — `type`, `ui`, embedded `test_cases`/`edge_cases` included) — must pass `validate plan --tests "$SESSION_DIR/TEST-SPEC.md"` + `dag check`.
4. ONE combined confirmation (spec + plan on one screen) → on approval chain straight to `/hoangsa:cook`, skipping `/hoangsa:prepare`.

If express work reveals hidden complexity (multi-file, real design choices) → stop, tell the user, switch to Full menu keeping what's already written. When in doubt, Full menu — but let history vote: `stats summary` (workspace task records) showing similar recent tasks landing small and clean with zero fix rounds is a point for express.

---

## Session creation (invoked from Step 3c — or express-lane step 1)

After Step 3a (task type) and Step 3c (description) are done, derive the slug
yourself — 2-4 key words from the description, hyphenated, lowercase (e.g.
"Thêm authentication cho API" → `api-authentication`). The user never types it.

```bash
# SLUG is auto-derived from user's description — NEVER ask them to type it
SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session init "$TASK_TYPE" "$SLUG")
# → { "id": "feat/api-authentication", "type": "feat", "name": "api-authentication", "dir": "..." }
```

Extract `SESSION_ID`, `SESSION_DIR`, and `SESSION_TYPE` from JSON output.

Initialize state for this session:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" state init "$SESSION_DIR"
# → creates state.json in SESSION_DIR with status: "pending"
```

### 1c. Git context check

Apply the shared git-context module from `git-context.md`:

1. Run Part A (detect branching context) — detect base branch, current branch, dirty state, naming conventions
2. Run Part B (git state check) — handle dirty state, create/checkout branch for new task
3. Run Part D (stash recovery) — notify if stashed work exists for this task

The expected branch is derived from `SESSION_ID` (e.g., `feat/api-authentication`). If the user has uncommitted changes from another task, handle before switching.

---

## Step 2: Load saved preferences + auto-detect

Load project-level preferences from config.json. These persist across sessions — only ask what's missing.

```bash
PREFS=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get .)
# → { "lang": "vi", "spec_lang": "vi", "tech_stack": ["typescript"], ... }
```

Parse the returned JSON to extract: `lang`, `spec_lang`, `tech_stack`, `interaction_level`, `review_style`.

**Apply `interaction_level` throughout this workflow:**
- `"detailed"` → ask more deep-dive questions (3-5), show full option trade-offs, include architecture reasoning in specs
- `"concise"` → ask fewer deep-dive questions (1-2), shorter option descriptions, skip obvious explanations
- `null` → default to `"detailed"`

### 2a. Auto-detect tech stack (if not saved or empty)

If `tech_stack` is empty (`[]` or `null`):

```bash
# Auto-detect from manifest files
DETECTED_STACKS=""
[ -f "package.json" ] && DETECTED_STACKS="$DETECTED_STACKS typescript"
[ -f "tsconfig.json" ] && DETECTED_STACKS="$DETECTED_STACKS typescript"
[ -f "Cargo.toml" ] && DETECTED_STACKS="$DETECTED_STACKS rust"
[ -f "pyproject.toml" ] || [ -f "requirements.txt" ] || [ -f "setup.py" ] && DETECTED_STACKS="$DETECTED_STACKS python"
[ -f "go.mod" ] && DETECTED_STACKS="$DETECTED_STACKS go"
[ -f "pom.xml" ] || [ -f "build.gradle" ] && DETECTED_STACKS="$DETECTED_STACKS java"
echo "$DETECTED_STACKS"
```

Deduplicate and present:

```
Detected tech stack: [TypeScript, Python]
  - package.json → TypeScript/Node
  - pyproject.toml → Python

Đúng không? [OK / Thêm / Sửa]
```

After user confirms → save immediately:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" pref set . tech_stack '["typescript","python"]'
```

### 2b. Language preferences (if not saved)

If `lang` is `null`:

Use AskUserQuestion:
  question: "Bạn muốn giao tiếp bằng ngôn ngữ nào?"
  header: "Ngôn ngữ"
  options:
    - label: "Tiếng Việt", description: "Giao tiếp bằng tiếng Việt"
    - label: "English", description: "Communicate in English"
  multiSelect: false

After user answers → save immediately:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" pref set . lang "vi"
```

If `spec_lang` is `null`:

Use AskUserQuestion:
  question: "Ngôn ngữ viết specs (DESIGN-SPEC, TEST-SPEC)?"
  header: "Spec lang"
  options:
    - label: "Cùng ngôn ngữ giao tiếp", description: "Specs viết cùng ngôn ngữ đã chọn"
    - label: "Tiếng Việt", description: "Specs luôn viết bằng tiếng Việt"
    - label: "English", description: "Specs luôn viết bằng English (phổ biến cho team quốc tế)"
  multiSelect: false

After user answers → save immediately:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" pref set . spec_lang "vi"
```

### 2c. Show saved preferences summary (if all already set)

If all preferences were already saved, show a brief confirmation:

```
⚡ Dùng cài đặt đã lưu:
   Giao tiếp: Tiếng Việt
   Specs:     Tiếng Việt
   Stack:     [TypeScript, Python]

   (Muốn thay đổi? Gõ "thay đổi cài đặt")
```

Then move directly to Step 3 — no further questions about basics.

---

## Step 2d: Brainstorm detection (auto)

Check if a brainstorm session produced a `BRAINSTORM.md` that should feed into this design.

```bash
BRAINSTORM_SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session latest)
```

If `type` is `"brainstorm"` and `files` contains `"BRAINSTORM.md"` → read it
and pre-fill: **Idea** → Step 3c description; **Chosen approach** → design
direction; **Decisions** → LOCKED decisions in DESIGN-SPEC; **Open questions**
→ Step 3d deep-dive; **Out of scope** → CONTEXT.md. Show what was found
(`🧠 Brainstorm detected: <id> — idea / approach / N decisions`), then continue
to Step 2e — the user can still override everything. Otherwise skip.

---

## Step 2e: Task link detection (auto)

Apply `task-link.md`: scan the user's input for task manager URLs (Linear,
Jira, ClickUp, GitHub, Asana). If found → fetch via MCP, download attachments
to `$SESSION_DIR/attachments/` (defer video analysis to Step 2f), set status
"In Progress" (non-blocking), save `$SESSION_DIR/EXTERNAL-TASK.md` + session
state. Pre-fill: labels → task type (still confirm in 3a); title + body →
3c description; acceptance criteria → DESIGN-SPEC. Show a short summary
(`📋 Task linked: <provider> <id> — <title>`), continue to Step 3 — the user
can override everything. No URL → skip.

---

## Step 2f: Media detection (auto)

Apply `common.md §Media detection` — findings become design context for
Steps 3–5 (layout descriptions, component structure). No media → Step 3.

---

## Step 3: Gather requirements

### 3a. Task type

Use AskUserQuestion:
  question: "Loại task bạn muốn làm?"
  header: "Task type"
  options:
    - label: "feat", description: "Tính năng mới — thêm chức năng chưa có"
    - label: "fix", description: "Sửa lỗi — fix bug hoặc behavior sai"
    - label: "refactor", description: "Tái cấu trúc — cải thiện code không đổi behavior"
    - label: "perf", description: "Tối ưu hiệu năng — cải thiện tốc độ/bộ nhớ"
    - label: "docs", description: "Tài liệu — README, API docs, comments, guides"
    - label: "ci", description: "CI/CD — pipeline, GitHub Actions, deployment config"
    - label: "infra", description: "Hạ tầng — Docker, K8s, terraform, server config"
    - label: "design", description: "UI/UX — landing page, component design, mockup"
    - label: "chore", description: "Việc lặt vặt — config, deps update, cleanup, scripts"
  multiSelect: false
  // User chọn Other nếu không khít — AI tự phân loại vào category gần nhất

After user selects, determine the **task category** — this controls how the rest of the workflow adapts:

| Category | Task types | What changes |
|----------|-----------|--------------|
| **code** | feat, fix, refactor, perf, test | Full DESIGN-SPEC with types/interfaces + TEST-SPEC with test cases |
| **ops** | ci, infra, chore, deploy | DESIGN-SPEC focuses on config/pipeline/steps + TEST-SPEC becomes VALIDATION-SPEC (smoke tests, health checks) |
| **content** | docs, design | DESIGN-SPEC becomes a lightweight PLAN-SPEC (outline, structure, deliverables) + TEST-SPEC becomes CHECKLIST (review criteria) |

If the user picks "Other" and describes something that doesn't fit neatly, use your judgment to pick the closest category. The categories are guidelines, not boxes — blend them if the task spans multiple (e.g., "add a feature with Docker setup" = code + ops).

### 3b. Task-specific stack (if multi-stack project)

If `tech_stack` has more than 1 entry, ask which stack this task targets:

Use AskUserQuestion:
  question: "Task này thuộc stack nào?"
  header: "Stack"
  options: (generated from saved tech_stack array)
    - label: "<stack 1>", description: "..."
    - label: "<stack 2>", description: "..."
    - label: "Cả hai", description: "Task ảnh hưởng cả hai stack"
  multiSelect: true

If only 1 stack → skip this question, use that stack automatically.
If task category is **content** and task doesn't touch code → skip this question entirely.

### 3c. Initial description

Use AskUserQuestion (hybrid free-form):
  question: "Mô tả task bạn muốn làm?"
  header: "Mô tả"
  options:
    - label: "Tôi sẽ mô tả", description: "Gõ mô tả chi tiết vào ô 'Other' bên dưới"
    - label: "Cho ví dụ trước", description: "Xem ví dụ mô tả task trước khi tự viết"
  multiSelect: false

If user chọn "Cho ví dụ trước" → show 2-3 example descriptions **relevant to their task type**, then ask again.
If user chọn "Tôi sẽ mô tả" or Other → use their input as the task description.

### 3d. Deep dive

For each deep-dive question, use AskUserQuestion:
  question: "<specific question about unclear point>"
  header: "<topic, max 12 chars>"
  options: (≥2, max 4 — with trade-offs in description)
    - label: "<Option A>", description: "<rationale and trade-offs>"
    - label: "<Option B>", description: "<rationale and trade-offs>"
    - label: "<Option C>", description: "<rationale and trade-offs>"
  multiSelect: false
  // User chọn Other nếu không option nào khít

Still ask one question at a time. Adapt questions to the task type — different tasks need different angles explored:

**Code tasks:**
- `feat`: scope, edge cases, API contract, error handling, rollback
- `fix`: root cause hypothesis, affected surfaces, regression risk
- `refactor`: invariants to preserve, measuring "done", rollback
- `perf`: baseline metrics, targets, acceptable trade-offs
- `test`: coverage target, test strategy, mocking approach

**Ops tasks:**
- `ci`: trigger conditions, environments, secrets management, rollback strategy, notification
- `infra`: base image, resource limits, networking, persistence, scaling approach
- `chore`: scope of cleanup, what to preserve, breaking changes risk
- `deploy`: zero-downtime strategy, health checks, rollback plan, environment parity

**Content tasks:**
- `docs`: audience, format (markdown/docsite/inline), depth level, examples needed
- `design`: target audience, responsive needs, brand/style guide, key sections, CTA goals

**Cross-cutting (when relevant):** timeline pressure, who else is affected, existing examples/references to follow.

Match question count to complexity: 1-2 for simple tasks, 3-5 for complex ones.

If the user's choices reveal strong design preferences (e.g., "always use interfaces", "prefer functional over OOP", "no magic strings"):

```
memory_remember_preference({text: "<preference>"})
```

### 3e. Write CONTEXT.md

Save to `$SESSION_DIR/CONTEXT.md`:

```markdown
# Context: <Title>

## Task Type
<type>

## Language
<lang>

## Spec Language
<spec_lang>

## Tech Stack
<stack for this task — e.g. Python 3.12 / FastAPI / PostgreSQL>

## User Input
<Original description — verbatim>

## Discussion Log
### [Q1] <Question>
- Options: A / B / C
- Chosen: <choice>
- Reason: <rationale>

## Decisions Made
| # | Decision | Reason | Type |
|---|----------|--------|------|
| 1 | ... | ... | LOCKED |
| 2 | ... | ... | FLEXIBLE |

## Out of Scope
<What we confirmed NOT doing>
```

---

## Step 3f: Load codebase metadata from config

```bash
CONFIG=$("$HOANGSA_ROOT/bin/hoangsa-cli" config get .)
```

Extract from config and pass to research step:
- `codebase.packages` → known packages, entry points, build/test commands
- `codebase.frameworks` → detected frameworks
- `codebase.testing` → test frameworks, config files, file patterns
- `codebase.entry_points` → project entry points

This avoids re-detecting what init already discovered. Research agents should use this metadata as a starting point and only re-detect if it appears stale.

---

## Step 4: Codebase research (delegate to /hoangsa:research)

First, read the `research_mode` preference to determine how to run this step:

```bash
RESEARCH_MODE=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . research_mode | python3 -c "import sys,json; print(json.load(sys.stdin).get('value','') or 'inline')")
```

**`"full"`:** invoke /hoangsa:research with Topic = task description from
CONTEXT.md, Scope = "codebase", Mode = "auto", Session = current $SESSION_DIR
(output → `$SESSION_DIR/RESEARCH.md`), passing MEMORY_STATUS and the
`codebase` config metadata so agents don't re-detect structure. Soft timeout
120s — on timeout proceed with available context and note the gap in
DESIGN-SPEC. Wait for RESEARCH.md before proceeding.

**`"inline"` (default):** lightweight instead: (1) hoangsa-memory recall or
Grep/Glob for code relevant to the task, (2) read key files from CONTEXT.md,
(3) write a minimal `RESEARCH.md` (relevant symbols, files, patterns,
dependencies). Report: `⏭️ Full research skipped (research_mode=inline)`.

---

## Step 5: Create DESIGN-SPEC.md

Synthesize CONTEXT + RESEARCH. Write specs in the `spec_lang` from preferences.

**The spec structure adapts to the task category.** Don't force code-centric sections onto non-code tasks. The frontmatter and Overview/Requirements sections are always present; everything else varies.

### Always required:

- YAML frontmatter with all fields
- Section `## Overview` with Goal, Context, Requirements, Out of Scope
- Section `## Acceptance Criteria` — how we know it's done (format varies by category)

### Category-specific sections:

**Code tasks** (feat, fix, refactor, perf, test):
- `## Types / Data Models` — define types/schemas for the stack
- `## Interfaces / APIs` — function signatures, endpoints, contracts
- `## Implementations` — LOCKED/FLEXIBLE decisions, affected files, flow/logic
- Acceptance = **runnable test commands**

**Ops tasks** (ci, infra, chore, deploy):
- `## Configuration / Pipeline` — config files, pipeline stages, environment variables
- `## Steps / Runbook` — ordered steps to implement, with rollback at each stage
- `## Dependencies & Prerequisites` — what needs to exist before this can work
- Acceptance = **health checks, smoke tests, status commands** (e.g., `docker compose ps`, `curl -f http://localhost:8080/health`, `gh workflow view`)

**Content tasks** (docs, design):
- `## Structure / Outline` — sections, pages, or components to create
- `## Deliverables` — concrete list of files/artifacts with format and location
- `## Style & Guidelines` — tone, audience, brand, formatting rules
- Acceptance = **checklist of deliverables** (e.g., "README.md exists and covers X, Y, Z")

**Hybrid tasks** (spans multiple categories):
- Pick sections from each relevant category. A "feature + Docker setup" task gets code sections for the feature AND ops sections for Docker. Don't duplicate — merge where it makes sense.

### Acceptance criteria formats by category:

| Category | Acceptance format | Examples |
|----------|------------------|---------|
| Code | Runnable test commands | `pytest tests/test_x.py -v`, `npx jest src/x.test.ts` |
| Ops | Health/status checks | `docker compose ps`, `curl -f localhost:8080/health`, `terraform plan` |
| Content | Deliverable checklist | "README includes setup section", "API docs cover all endpoints" |
| Hybrid | Mix of the above | Test commands + health checks |

### Template (adapt sections based on category):

```markdown
---
spec_version: "1.0"
project: "<project name>"
component: "<component/module name — snake_case>"
language: "<actual tech stack — or 'N/A' for pure content tasks>"
task_type: "<feat|fix|refactor|perf|test|docs|ci|infra|design|chore>"
category: "<code|ops|content>"
status: "draft"
---

## Overview
[<task_type>]: <Short title>

### Goal
<One sentence end result>

### Context
<Current state, why this is needed>

### Requirements
- [REQ-01] <Specific, verifiable requirement>
- [REQ-02] <...>

### Out of Scope
- <From CONTEXT.md>

---

<!-- CODE CATEGORY: include these sections -->

## Types / Data Models
<Language-appropriate type definitions>

## Interfaces / APIs
<Public function signatures, class methods, REST endpoints>
<Use actual language syntax — not pseudo-code>

<!-- OPS CATEGORY: include these sections -->

## Configuration / Pipeline
<Config files, pipeline stages, env vars, secrets (reference only — never include actual secrets)>

## Steps / Runbook
1. <Step with expected outcome>
   - Rollback: <how to undo this step>
2. <Next step...>

## Dependencies & Prerequisites
<What must exist/be configured before starting>

<!-- CONTENT CATEGORY: include these sections -->

## Structure / Outline
<Sections, pages, or components — with purpose of each>

## Deliverables
| Deliverable | Format | Location | Description |
|------------|--------|----------|-------------|
| ... | .md / .html / .yml | path/to/file | What it contains |

## Style & Guidelines
<Audience, tone, formatting rules, references to follow>

<!-- ALL CATEGORIES: include these sections -->

---

## Implementations

### Design Decisions
| # | Decision | Reasoning | Type |
|---|----------|-----------|------|
| 1 | ... | ... | LOCKED |
| 2 | ... | ... | FLEXIBLE |

### Affected Files

**If hoangsa-memory available:** Use `memory_impact({target: "symbolName", direction: "upstream"})` for each symbol being modified to discover all affected files (direct callers at d=1, indirect at d=2). This prevents missing files that import or call the changed code.

| File | Action | Description | Impact |
|------|--------|-------------|--------|
| `path/to/file` | CREATE / MODIFY / DELETE | What changes | d=1 / d=2 / N/A |

---

## Open Questions
- <Unresolved, if any>

## Constraints
- <Performance, security, compatibility, deadline>

---

## Acceptance Criteria

### Per-Requirement
| Req | Verification | Expected Result |
|-----|-------------|----------------|
| REQ-01 | <command or checklist item> | <expected result> |

### Overall
<Verification sequence appropriate to the task category>
```

**Important:** Delete irrelevant sections entirely — never leave them blank (a docs task has no "Types / Data Models"; a CI task has no "Interfaces / APIs").

**After drafting:** Show the full document to user.

Then check `review_style` preference:

```bash
REVIEW=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . review_style)
```

### If `review_style` is "whole_document" or null (default):

Use AskUserQuestion:
  question: "DESIGN-SPEC có OK không?"
  header: "Review"
  options:
    - label: "OK", description: "Spec ổn, tiếp tục sang TEST-SPEC"
    - label: "Cần sửa", description: "Có điểm cần chỉnh — ghi chi tiết vào Other"
    - label: "Viết lại", description: "Viết lại toàn bộ spec"
  multiSelect: false

If "OK" → proceed to Step 6.
If "Cần sửa" → apply fixes from Other, re-show, re-ask.
If "Viết lại" → rewrite, re-show, re-ask.

### If `review_style` is "section_by_section":

For each section in [Overview, Types/Data Models, Interfaces/APIs, Implementations, Acceptance Criteria]:
  Use AskUserQuestion:
    question: "Section '<section name>' có OK không?"
    header: "<section>"  (max 12 chars)
    options:
      - label: "OK", description: "Section này ổn, tiếp tục sang section tiếp theo"
      - label: "Cần sửa", description: "Có điểm cần chỉnh — ghi chú chi tiết vào ô Other"
      - label: "Viết lại", description: "Viết lại toàn bộ section này từ đầu"
    multiSelect: false

  If "OK" → next section
  If "Cần sửa" → read user notes from Other, apply fixes, re-ask same section
  If "Viết lại" → rewrite entire section, re-ask same section

### First-time review_style setup:

If `review_style` is `null` → after the first review round, save based on behavior:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" pref set . review_style "whole_document"
```

Architectural choices (interfaces, dependencies) belong in the DESIGN-SPEC's Architecture / Interfaces sections — that document is the canonical record.

---

## Step 6: Create TEST-SPEC.md (or VALIDATION-SPEC / CHECKLIST)

The verification document adapts to the task category. The goal is always the same — define how we know the task is done — but the format changes.

### 6a. Strategy selection (adapts by category)

**Code tasks** → ask test strategy:

Use AskUserQuestion with preview:
  question: "Chọn test strategy cho task này?"
  header: "Strategy"
  options:
    - label: "Unit-heavy"
      description: "Tập trung unit test, mock dependencies — nhanh, isolate tốt"
    - label: "Integration"
      description: "Test luồng thực với real dependencies — chậm hơn nhưng sát thực tế"
    - label: "Mixed (Recommended)"
      description: "Unit cho logic phức tạp, integration cho luồng chính — cân bằng tốt nhất"
  multiSelect: false

**Code tasks — determine `surface` (E2E is NOT a strategy option; it's mandatory for user-facing work):**

Infer from the DESIGN-SPEC Interfaces/APIs section: does the task expose anything a user or external client touches directly — a UI screen/component, an HTTP endpoint, a CLI command? If the answer is obvious (e.g., the task IS a UI feature), set `surface` yourself and report it. Only ask when genuinely ambiguous:

Use AskUserQuestion:
  question: "Task này expose surface nào cho user/client chạm trực tiếp?"
  header: "Surface"
  options:
    - label: "UI"
      description: "Screen/component người dùng nhìn thấy — bắt buộc ## E2E Tests + ## Visual Verification"
    - label: "API"
      description: "HTTP endpoint client gọi trực tiếp — bắt buộc ## E2E Tests"
    - label: "CLI"
      description: "Command người dùng chạy trực tiếp — bắt buộc ## E2E Tests"
    - label: "Internal"
      description: "Chỉ logic nội bộ / library — unit + integration là đủ"
  multiSelect: false

Enforcement (`hoangsa-cli validate tests` fails the spec otherwise):
`ui|api|cli` ⇒ `## E2E Tests` with runnable commands; `ui` ⇒ additionally a
`## Visual Verification` table with ≥1 row.

**Ops tasks** → ask validation approach:

Use AskUserQuestion:
  question: "Cách validate task ops này?"
  header: "Validation"
  options:
    - label: "Smoke test"
      description: "Chạy thử pipeline/container, kiểm tra status — nhanh gọn"
    - label: "Full validation"
      description: "Smoke test + test rollback + test edge cases (network fail, timeout)"
    - label: "Dry-run only"
      description: "Chỉ dry-run, không apply thật — cho thay đổi infra nhạy cảm"
  multiSelect: false

**Content tasks** → skip strategy question, go straight to checklist.

### 6b. Write the verification document

Write in `spec_lang`. The filename is always `TEST-SPEC.md` for engine compatibility, but the content adapts.

**For code tasks:**

```markdown
---
tests_version: "1.0"
spec_ref: "<component>-spec-v1.0"
component: "<MUST MATCH DESIGN-SPEC.md>"
category: "code"
strategy: "<unit|integration|property|mixed>"
surface: "<ui|api|cli|internal>"
language: "<same as DESIGN-SPEC>"
---

## Unit Tests

### Test: <descriptive_test_name>
- **Covers**: [REQ-01]
- **Input**: <concrete values>
- **Setup**: <mocks/fixtures if needed>
- **Expected**: <exact output>
- **Verify**: `<runnable test command>`

## Integration Tests

### Test: <descriptive_test_name>
- **Covers**: [REQ-02]
- **Setup**: <environment, fixtures>
- **Steps**:
  1. <step>
  2. <step>
- **Expected**: <outcome>
- **Verify**: `<runnable test command>`

## E2E Tests
<!-- REQUIRED when surface: ui|api|cli. Drive the WHOLE flow like a real user/client
     — through the UI, the HTTP endpoint, or the CLI binary — not through internal APIs. -->

### Test: <flow_name>
- **Covers**: [REQ-xx, ...]
- **Entry point**: <URL / screen / CLI invocation>
- **Steps**: <user-observable actions — click, type, call endpoint>
- **Expected**: <observable outcome — visible text, HTTP status + body, exit code>
- **Verify**: `<runnable command — playwright / maestro / curl script / CLI>`

## Edge Cases
<!-- MUST be non-empty (`validate tests` fails an empty table). Pull every boundary,
     error path, and weird input from the Step 3d deep-dive. A REQ with truly no
     edge case gets a waiver row: | None for REQ-xx | — | — | <reason> | -->
| Case | Input | Expected | Covers |
|------|-------|----------|--------|

## Visual Verification
<!-- REQUIRED when surface: ui (≥1 row). Each state gets verified against the REAL
     running app (fe-testing flow 5) — screenshots are the evidence. Delete for non-UI. -->
| Screen / Component | States to verify | How |
|--------------------|------------------|-----|
| <name> | empty / loading / error / success / disabled / long-text overflow / responsive | run app + screenshot each state |

## Test Data / Fixtures
<Mock data, factories, sample inputs>

## Coverage Target
- Target: ≥ <X>%
- Critical paths: 100%
```

**For ops tasks:**

```markdown
---
tests_version: "1.0"
spec_ref: "<component>-spec-v1.0"
component: "<MUST MATCH DESIGN-SPEC.md>"
category: "ops"
strategy: "<smoke|full|dry-run>"
language: "<tools used — e.g. docker, terraform, github-actions>"
---

## Pre-flight Checks
- [ ] <prerequisite is met — e.g., Docker daemon running>
- [ ] <dependency exists — e.g., `.env` file present>

## Smoke Tests

### Check: <descriptive_name>
- **Covers**: [REQ-01]
- **Command**: `<runnable command>`
- **Expected**: <exit code 0, specific output, status>
- **Timeout**: <max wait time>

## Rollback Verification
### Rollback: <scenario>
- **Trigger**: <what goes wrong>
- **Steps**: <rollback commands>
- **Verify**: `<command to confirm rollback worked>`

## Edge Cases
| Scenario | How to simulate | Expected behavior | Covers |
|----------|----------------|-------------------|--------|
| Network failure | `docker network disconnect ...` | Graceful retry | REQ-03 |
```

**For content tasks:**

```markdown
---
tests_version: "1.0"
spec_ref: "<component>-spec-v1.0"
component: "<MUST MATCH DESIGN-SPEC.md>"
category: "content"
strategy: "checklist"
language: "N/A"
---

## Deliverable Checklist

### [REQ-01] <deliverable name>
- [ ] File exists at expected path
- [ ] Covers required topics: <list>
- [ ] Follows style guide / formatting rules
- [ ] <Specific quality criterion>

## Review Criteria
| Criterion | How to verify | Covers |
|-----------|--------------|--------|
| Accuracy | Cross-check with source/docs | REQ-01 |
| Completeness | All sections from outline present | REQ-02 |
| Audience-appropriate | No jargon for end-user docs | REQ-03 |

## Content Quality Gates
- [ ] Spell-check passes
- [ ] Links are valid (if applicable)
- [ ] Screenshots/diagrams up to date (if applicable)
```

**For hybrid tasks:** Combine sections from relevant categories. A "feature + Docker" task gets Unit/Integration Tests AND Smoke Tests.

**After drafting:** Show the full document to user.

Review using the same `review_style` as Step 5 (whole_document or section_by_section), applied to the relevant sections for the task category.

---

## Step 7: Validate

```bash
SPEC_RESULT=$("$HOANGSA_ROOT/bin/hoangsa-cli" validate spec \
  "$SESSION_DIR/DESIGN-SPEC.md")
echo $SPEC_RESULT

TEST_RESULT=$("$HOANGSA_ROOT/bin/hoangsa-cli" validate tests \
  "$SESSION_DIR/TEST-SPEC.md")
echo $TEST_RESULT
```

If errors → fix and re-validate before proceeding.

Manual cross-check:
- `component` in TEST-SPEC == `component` in DESIGN-SPEC ✓
- All `[REQ-xx]` in TEST-SPEC exist in DESIGN-SPEC ✓

---

## Step 8: Complete

Save all files to `$SESSION_DIR/`:
- `CONTEXT.md`
- `RESEARCH.md`
- `DESIGN-SPEC.md`
- `TEST-SPEC.md`

Update state to reflect design is complete:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" state update "$SESSION_ID" '{"status":"design"}'
```

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" commit \
  "menu(<scope>): complete spec for <component>" \
  --files \
    $SESSION_DIR/CONTEXT.md \
    $SESSION_DIR/RESEARCH.md \
    $SESSION_DIR/DESIGN-SPEC.md \
    $SESSION_DIR/TEST-SPEC.md
```

Report:
```
✅ Design complete!
   Session:      <session-id>
   Stack:        <tech stack>
   Task type:    <type>
   Requirements: <N>
   Test cases:   <N>

   Next: /hoangsa:prepare
```

---

## Self-verification checklist

Before Step 8, emit the `common.md` self-verification table with rows:

```
| 0. Setup (lang + hoangsa-memory) | ... |
| 1. Init session | ... |
| 2. Gather input | ... |
| 3. Create CONTEXT.md | ... |
| 4. Research | ... |
| 5. DESIGN-SPEC.md | ... |
| 6. TEST-SPEC.md | ... |
| 7. Validate specs | ... |
| 8. Save + commit | ... |
```

---

## Rules

Universal rules live in `common.md §Universal rules`. Menu-specific additions:

| Rule | Detail |
|------|--------|
| **DON'T skip discussion** | Ask before deciding — except on the express lane, where there's nothing to decide |
| **Express cuts questions, not gates** | Express specs/plans pass the same validate commands |
| **≥3 options** | For every important decision |
| **Language-agnostic** | Use the actual stack's syntax |
| **Acceptance = command** | Runnable, not prose |
| **Edge cases mandatory** | Code TEST-SPEC needs a non-empty Edge Cases table — explicit waiver rows only, never silent omission |
| **ui/api/cli ⇒ E2E** | `surface: ui|api|cli` requires a runnable `## E2E Tests` section |
| **ui ⇒ Visual Verification** | `surface: ui` additionally requires a `## Visual Verification` table |
| **Loop until approved** | User can revise indefinitely |
| **Validate before done** | Run hoangsa-cli before step 8 |
| **Auto-detect before asking** | Detect tech stack from manifests, don't ask what's detectable |
