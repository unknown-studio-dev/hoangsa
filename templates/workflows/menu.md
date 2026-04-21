# HOANGSA Menu Workflow

You are the design lead. Mission: take user from vague idea → DESIGN-SPEC + TEST-SPEC, ready for planning.

**Principles:** Don't skip discussion. Ask one question at a time, not a dump list. User always has final say. ≥3 options for every important decision. Use AskUserQuestion for all interactions.

> **MUST complete ALL steps in order. DO NOT skip any step. DO NOT stop before Step 8.**
>
> 1. Init session → 2. Gather input → 3. Create context → 4. Research → 5. Design spec → 6. Test spec → 7. Validate → 8. Complete (save + commit)

---

---

## Step 1: Init session

**Do NOT create the session yet.** Session creation happens automatically in Step 3c after the task type and description are collected. Continue to Step 2.

---

## Step 1b: Create session (called from Step 3c)

After Step 3a (task type) and Step 3c (description) are done, auto-extract a slug from the user's description and create the session. The user never types the session name — you derive it.

**How to derive the slug:** Take the user's task description, extract 2-4 key words that capture the essence, join with hyphens. Examples:
- "Thêm authentication cho API" → `api-authentication`
- "Fix lỗi null pointer trong UserService" → `userservice-null-pointer`
- "Refactor session naming to be descriptive" → `session-naming`

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

Parse the result — if `type` is `"brainstorm"` and `files` contains `"BRAINSTORM.md"`:

1. Read `BRAINSTORM.md` from the brainstorm session directory
2. Extract from the brainstorm:
   - **Idea** → use as initial description in Step 3c (pre-fill, user can override)
   - **Chosen approach** → carry forward as design direction
   - **Decisions** → import LOCKED decisions into DESIGN-SPEC later
   - **Open questions** → surface during Step 3d deep-dive
   - **Out of scope** → carry into CONTEXT.md
3. Show the user what was found:

```
🧠 Brainstorm detected: <brainstorm session id>
   Idea:      <idea summary>
   Approach:  <chosen approach>
   Decisions: <N> locked, <N> flexible

   Dùng brainstorm này làm context cho design.
```

4. Continue to Step 2e with pre-filled context — the user can still override everything.

**If no brainstorm session or latest session is not a brainstorm:** Skip this step.

---

## Step 2e: Task link detection (auto)

Apply the shared task-link detection from `task-link.md`.

Before gathering requirements, scan the user's input for task manager URLs (Linear, Jira, ClickUp, GitHub, Asana).

**If a task URL is detected:**

1. Fetch task details via MCP (see `task-link.md` for detection logic and URL patterns)
2. Fetch and process attachments (see `task-link.md` Step 3b) — download to `$SESSION_DIR/attachments/`, classify by type. **Do NOT process videos here** — video analysis is deferred to Step 2f (media detection) which handles both user-provided and task-link media in one pass.
3. Set task status to "In Progress" (non-blocking, best-effort)
4. Save to `$SESSION_DIR/EXTERNAL-TASK.md` + store reference in session state
5. Auto-extract from the fetched task:
   - **Task type** → infer from labels/tags (bug→fix, feature→feat, etc.) — still confirm with user in 3a
   - **Description** → use task title + body as initial description in 3c
   - **Acceptance criteria** → carry over to DESIGN-SPEC later
6. Show the user what was fetched:

```
📋 Task linked: <provider> <task_id>
   <title>
   Status: <status>  Priority: <priority>

   Description preview:
   <first 3 lines of description>

   Dùng thông tin này làm context cho design.
```

7. Continue to Step 3 with pre-filled context — the user can still override everything.

**If no task URL detected:** Skip this step, proceed to Step 3 normally.

---

## Step 2f: Media detection (auto)

Scan **two sources** for media files:

1. **User's input** — file paths or pasted screenshots/videos in the message
2. **Task-link attachments** — files downloaded to `$SESSION_DIR/attachments/` by Step 2e

**Detection patterns:**
- File paths ending in: `.png`, `.jpg`, `.jpeg`, `.webp`, `.gif` (images)
- File paths ending in: `.mp4`, `.mov`, `.webm`, `.avi`, `.mkv` (videos)
- Screenshots pasted or attached by the user

**Check task-link attachments:**
```bash
# If task-link downloaded attachments, scan them too
if [ -d "$SESSION_DIR/attachments" ]; then
  ls "$SESSION_DIR/attachments/"
fi
```

**If images detected (from either source):**
- Claude reads images natively — no processing needed
- Note the image paths for reference during design discussion (Step 3–5)
- Use as visual context when generating DESIGN-SPEC (e.g., layout descriptions, component structure)

**If videos detected (from either source):**
1. Invoke the `visual-debug` skill for video processing:
   - Check ffmpeg availability: `hoangsa-cli media check-ffmpeg`
   - **Always quote the path** and validate it contains no shell metacharacters: `hoangsa-cli media analyze "$VIDEO_PATH" --output-dir "/tmp/hoangsa-menu-$(date +%s)"`
   - Read the output `montage.png` (annotated frame grid with timestamps)
   - Read the output `diff-montage.png` (red overlay showing changes between frames)
2. Include visual analysis findings as design context for Step 3–5

**If no media detected (from either source):** Skip this step, proceed to Step 3.

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

**Cross-cutting (ask when relevant regardless of type):**
- Timeline pressure? (affects depth of spec)
- Who else is affected? (team coordination)
- Any existing examples or references to follow?

Be smart about depth: a quick config change doesn't need 5 deep-dive questions. A complex new feature does. Match the number of questions to the complexity of the task — 1-2 for simple tasks, 3-5 for complex ones.

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

**If `RESEARCH_MODE` is `"full"`:** delegate codebase research to the research workflow in **auto mode**, scoped to **codebase only**. Pass the `codebase` metadata from config so research agents don't re-detect project structure from scratch:

```
Invoke /hoangsa:research with:
  - Topic: the task description from CONTEXT.md
  - Scope: "codebase"
  - Mode: "auto"
  - Session: use the current $SESSION_DIR (research output goes to $SESSION_DIR/RESEARCH.md)
  - Thoth: pass THOTH_STATUS so research agents use Thoth tools when available
```

This avoids duplicating the parallel research agents — the research workflow handles structure, patterns, dependencies, and tests analysis with Thoth-first fallback.

Set a soft timeout of 120 seconds for research. If research does not complete, proceed with available context and note in DESIGN-SPEC that research was incomplete.

Wait for RESEARCH.md to be written to `$SESSION_DIR/` before proceeding.

**If `RESEARCH_MODE` is `"inline"` (default):** skip the full research agent and perform lightweight inline research instead:

1. Use Thoth recall (if available) or Grep/Glob to find code relevant to the task description from CONTEXT.md
2. Read key files identified in CONTEXT.md (context_pointers, referenced paths)
3. Write a minimal `RESEARCH.md` to `$SESSION_DIR/` summarising findings (relevant symbols, files, patterns, dependencies)

Report: `⏭️ Full research skipped (research_mode=inline) — using inline research`

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

**If Thoth available:** Use `memory_impact({target: "symbolName", direction: "upstream"})` for each symbol being modified to discover all affected files (direct callers at d=1, indirect at d=2). This prevents missing files that import or call the changed code.

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

**Important:** Only include sections relevant to the task. A docs task should NOT have empty "Types / Data Models" sections. A CI task should NOT have "Interfaces / APIs". Delete irrelevant sections entirely — don't leave them blank.

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

## Edge Cases
| Case | Input | Expected | Covers |
|------|-------|----------|--------|

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

Before reporting completion in Step 8, output this table. Every row MUST show DONE or SKIPPED:

```
| Step | Status |
|------|--------|
| 0. Setup (lang + Thoth) | DONE / SKIPPED |
| 1. Init session | DONE / SKIPPED |
| 2. Gather input | DONE / SKIPPED |
| 3. Create CONTEXT.md | DONE / SKIPPED |
| 4. Research | DONE / SKIPPED |
| 5. DESIGN-SPEC.md | DONE / SKIPPED |
| 6. TEST-SPEC.md | DONE / SKIPPED |
| 7. Validate specs | DONE / SKIPPED |
| 8. Save + commit | DONE / SKIPPED |
```

If any step shows SKIPPED without explicit user approval, go back and complete it before stopping.

---

## Rules

| Rule | Detail |
|------|--------|
| **DON'T skip discussion** | Ask before deciding |
| **≥3 options** | For every important decision |
| **Language-agnostic** | Use the actual stack's syntax |
| **Acceptance = command** | Runnable, not prose |
| **Loop until approved** | User can revise indefinitely |
| **Validate before done** | Run hoangsa-cli before step 8 |
| **AskUserQuestion for all interactions** | Every user-facing question uses AskUserQuestion |
| **Save preferences on first ask** | Never ask the same preference twice — save to config.json |
| **Auto-detect before asking** | Detect tech stack from manifests, don't ask what's detectable |
