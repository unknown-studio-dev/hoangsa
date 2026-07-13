# Audit Workflow

> **Boot:** Read `$HOANGSA_ROOT/workflows/common.md` first — universal rules + CLI reference + self-verification template.

Perform a comprehensive codebase audit across 8 dimensions, producing a detailed AUDIT-REPORT.md that teams can use as a refactoring roadmap.

**Principles:** Parallel scanning for speed. Evidence-based — every finding must include file paths, line numbers, and concrete examples. Severity-rated so teams can prioritize. Actionable — each finding includes a suggested fix. Use hoangsa-memory when available, fall back gracefully.

---

---

## Step 1: Session & output setup

```bash
SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session latest 2>/dev/null || echo "")
```

- If `SESSION` is non-empty → use `SESSION_DIR` as output directory.
- If empty → auto-create a standalone audit session. Derive slug from the audit scope:

```bash
# SLUG auto-derived from scope (e.g. "full-codebase", "auth-module")
SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session init chore "$SLUG")
# Extract SESSION_DIR from the result
```

---

## Step 2: Gather audit scope

### 2a. Load saved preferences

```bash
PREFS=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . 2>/dev/null || echo "{}")
```

### 2b. Audit target (ask user)

Use AskUserQuestion:
  question: "Scan ở đâu?"
  header: "Audit Target"
  options:
    - label: "Toàn bộ project", description: "Scan tất cả source code trong project (trừ các path trong .gitignore)"
    - label: "Chọn paths / modules", description: "Chỉ scan một số thư mục hoặc file cụ thể — ghi vào Other (vd: src/auth, src/api/users.ts, lib/)"
  multiSelect: false

Store as `AUDIT_TARGET`.

- If "Toàn bộ project" → `AUDIT_PATHS = ["."]` (root)
- If "Chọn paths / modules" → parse the user's input into a list of paths. Validate each path exists. If any path doesn't exist, warn and ask again.

Store as `AUDIT_PATHS` — all scanning agents must restrict their search (Grep, Glob, Read) to only these paths. When `AUDIT_PATHS` is not `["."]`, agents must use the paths as base directories for all operations.

### 2c. Audit scope (ask user)

Use AskUserQuestion:
  question: "Audit phạm vi nào?"
  header: "Audit Scope"
  options:
    - label: "Full audit (Recommended)", description: "Scan toàn bộ 9 dimensions — architecture, overengineering, dead code, magic values, security, performance, dependencies, tests, docs, DX, maintainability"
    - label: "Quick scan", description: "Chỉ scan 4 dimensions quan trọng nhất — architecture (incl. overengineering, dead code, bloated files), security, code quality (incl. magic values), maintainability"
    - label: "Custom", description: "Chọn dimensions cụ thể — ghi vào Other (vd: security, performance)"
  multiSelect: false

Store as `AUDIT_SCOPE`.

### 2d. Audit depth

Use AskUserQuestion:
  question: "Mức độ chi tiết?"
  header: "Depth"
  options:
    - label: "Surface", description: "Pattern matching + static analysis — nhanh, ~2 phút"
    - label: "Deep (Recommended)", description: "Đọc code, trace execution flows, cross-reference — chính xác hơn, ~5-10 phút"
  multiSelect: false

Store as `AUDIT_DEPTH`.

---

## Step 2e: Task link detection

Apply the shared task-link detection from `task-link.md`:

1. Scan user input for task manager URLs (Linear, Jira, ClickUp, GitHub, Asana)
2. If found → fetch task details via MCP → save to `EXTERNAL-TASK.md`
3. Fetch and process attachments (see `task-link.md` Step 3b) — download to `$SESSION_DIR/attachments/`, classify by type. **Do NOT process videos here** — deferred to Step 2f.
4. Extract from fetched task:
   - **Labels/tags** → scope audit to affected areas
   - **Description** → identify what area to audit
   - **Related tasks/PRs** → context for recent changes

If no task URL → skip, proceed normally.

---

## Step 2f: Media detection (auto)

Apply `common.md §Media detection` — findings become visual evidence for the
dimension scanning agents (e.g. UI/UX issues, layout problems). No media →
Step 3.

---

## Step 3: Detect project metadata

Before scanning, gather project context. **Start from config.json** (detected by `/hoangsa:init`), then fill gaps:

```bash
CONFIG=$("$HOANGSA_ROOT/bin/hoangsa-cli" config get .)
INTERACTION=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . interaction_level)
```

### 3a. Load from config (already detected by init)

Extract from `CONFIG`:
- `codebase.packages` → tech stack, build/test commands per package
- `codebase.frameworks` → detected frameworks
- `codebase.testing` → test frameworks, config files, file patterns
- `codebase.linters` → linter config
- `codebase.ci` → CI/CD platform
- `codebase.monorepo` → monorepo structure
- `codebase.entry_points` → project entry points — pass to Architecture dimension for focused analysis
- `preferences.tech_stack` → confirmed tech stack

### 3b. Fill gaps (only if config fields are empty/null)

For any field that is `null` or `[]` in config, detect from filesystem:

```
- Read package.json / Cargo.toml / pyproject.toml / go.mod → tech stack, versions
- Detect framework (React, Next.js, Express, Actix, Django, etc.)
- Count total files by type (*.js, *.ts, *.rs, *.py, etc.)
- Detect test framework (jest, vitest, cargo test, pytest, etc.)
- Check for CI/CD config (.github/workflows/, .gitlab-ci.yml, etc.)
- Check for linter/formatter config (.eslintrc, prettier, rustfmt, ruff, etc.)
- Detect monorepo structure (workspaces, lerna, turborepo, etc.)
```

Merge config data with detected data into `PROJECT_META`. Config values take precedence over re-detected values (they were user-confirmed during init).

**Apply `interaction_level`:**
- `"detailed"` → Deep audit shows full findings per dimension with evidence, confirm mode on
- `"concise"` → Show summary table + critical/high issues only, skip low-severity details
- `null` → default to `"detailed"`

Store as `PROJECT_META` — this context is passed to all scanning agents.

### 3b. Build exclusion list (MUST do before scanning)

Collect ignore patterns from the project to avoid scanning generated code, dependencies, and build artifacts. This prevents noise, false positives, and wasted context.

**Load ignore files in order** (later files add to the list, they don't replace):

```bash
# Collect all ignore sources
IGNORE_SOURCES=""
for f in .gitignore .dockerignore .eslintignore .prettierignore; do
  [ -f "$f" ] && IGNORE_SOURCES="$IGNORE_SOURCES $f"
done
```

**Always exclude** (even if no ignore files exist):

```
node_modules/    dist/          build/         target/
.next/           .nuxt/         .output/       out/
vendor/          __pycache__/   .venv/         venv/
.git/            .hoangsa/memory/        .hoangsa/
*.min.js         *.min.css      *.bundle.js
*.map            *.lock         package-lock.json
*.generated.*    *.pb.go        *_generated.rs
coverage/        .nyc_output/   htmlcov/
.terraform/      .serverless/
```

**Also exclude** files matching patterns from loaded ignore files above.

Store the combined exclusion list as `AUDIT_EXCLUDES`. Every scanning agent receives this list and **must skip** any file matching these patterns. When using Grep or Glob, apply exclusions via glob filters. When reading directory listings, filter out excluded paths before processing.

If a project has a custom ignore file (e.g., `.auditignore`), respect it too.

---

## Step 4: Parallel dimension scanning

Launch one scanning agent per dimension selected by `AUDIT_SCOPE` (Agent tool, parallel). Dimension specs live in separate files — do NOT read them into the orchestrator context; pass the path and let each agent read its own spec.

| # | Dimension | Spec file (`$HOANGSA_ROOT/workflows/audit-dimensions/`) |
|---|-----------|--------------------------------------------------------|
| 1 | Architecture & Structure (bloated files, dead code, overengineering, module inconsistency) | `01-architecture.md` |
| 2 | Code Smells & Anti-patterns (magic values, primitive obsession) | `02-code-smells.md` |
| 3 | Security | `03-security.md` |
| 4 | Performance | `04-performance.md` |
| 5 | Dependency Health | `05-dependency-health.md` |
| 6 | Test Quality | `06-test-quality.md` |
| 7 | Documentation | `07-documentation.md` |
| 8 | Developer Experience (DX) | `08-dx.md` |
| 9 | Simplify Scan — codebase-wide (4 criteria: preserve functionality, project standards, clarity, balance) | `09-simplify.md` |

Scope mapping: **Full audit** → all 9 · **Quick scan** → 1, 2, 3, 9 · **Custom** → the user's list.

Each agent prompt contains:
- `PROJECT_META`, `AUDIT_EXCLUDES`, `AUDIT_PATHS`
- First action: `Read $HOANGSA_ROOT/workflows/audit-dimensions/<file>` — that file IS the dimension spec (what to scan for, with evidence requirements)
- Scan only within `AUDIT_PATHS` (use as base dirs for Grep/Glob/Read); skip everything matching `AUDIT_EXCLUDES`
- Output: JSON array of findings, one object per finding:

```
{id: "<PREFIX>-001", severity: "CRITICAL|HIGH|MEDIUM|LOW", title: "...",
 file: "src/api.ts", line: 42, evidence: "<concrete example from code>",
 impact: "<what breaks or gets harder>", suggestion: "<specific action>",
 effort: "S|M|L|XL"}
```

ID prefix per dimension: ARCH / SMELL / SEC / PERF / DEP / TEST / DOC / DX / SIMP.

Use the conversation archive to spot focus areas: `memory_archive_topics()` — high-conversation areas are likely churn and deserve extra attention.

### Model selection

```bash
MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model researcher 2>/dev/null || echo "sonnet")
```

Use the resolved model for all scanning agents.

---

### Memory Health Agent (additional dimension)

Analyze the quality and health of hoangsa-memory memory:

1. `memory_show()` — read full MEMORY.md and LESSONS.md
2. Check for:
   - Stale facts that reference deleted files or renamed symbols
   - Duplicate or near-duplicate entries
   - Lessons with high failure rates (should be quarantined)
   - Facts that contradict current code state
3. For stale or contradictory entries, recommend removal:
   `memory_remove({kind: "fact|lesson", text: "<substring of stale entry>"})`
4. Report findings with specific entries to remove/update

---

## Step 5: Cross-reference & deduplicate

After all scanning agents return, cross-reference findings:

1. **Merge duplicates** — same issue found by multiple dimensions → keep the most detailed one, reference the other
2. **Connect related issues** — a GOD FILE might also be a COVERAGE GAP and a DX issue → link them
3. **Validate findings** — for Deep audit: re-read key files to confirm findings aren't false positives
4. **Assign severity** using this rubric:

| Severity | Definition | Examples |
|----------|-----------|----------|
| CRITICAL | Actively causing bugs, security breach, or data loss | SQL injection, secrets in code, race condition causing data corruption |
| HIGH | Will cause problems soon, or significantly slows development | Circular dependencies, missing auth checks, N+1 queries on main paths |
| MEDIUM | Code smell that makes maintenance harder over time | God files, missing tests for critical paths, outdated deps (minor) |
| LOW | Nice to fix but not urgent | Naming inconsistencies, missing docs, unused deps |

---

## Step 6: User review (confirm mode only for Deep audit)

If `AUDIT_DEPTH` is "Deep":

Use AskUserQuestion:
  question: "Tìm thấy N issues. Bạn muốn xem trước summary hay generate full report luôn?"
  header: "Audit Results Preview"
  options:
    - label: "Show summary first", description: "Xem tóm tắt theo severity trước khi generate full report"
    - label: "Generate full report", description: "Tạo AUDIT-REPORT.md chi tiết luôn"
  multiSelect: false

If user chọn "Show summary first" → display:

```
AUDIT SUMMARY
═════════════
CRITICAL: N issues
HIGH:     N issues
MEDIUM:   N issues
LOW:      N issues

Top issues:
1. [CRITICAL] SEC-001: SQL injection in userController.js:45
2. [CRITICAL] SEC-002: API key hard-coded in config.js:12
3. [HIGH] ARCH-001: Circular dependency api → auth → api
...
```

Then ask if they want the full report.

---

## Step 7: Generate AUDIT-REPORT.md

Use this template:

```markdown
# Audit Report

**Project:** <project name>
**Date:** <YYYY-MM-DD>
**Target:** <"entire project" or list of scanned paths>
**Scope:** <full / quick / custom dimensions>
**Depth:** <surface / deep>
**Tech Stack:** <detected stack>

---

## Executive Summary

<2-3 paragraph overview: overall health assessment, most critical issues, recommended priorities>

### Health Score

| Dimension | Score | Issues |
|-----------|-------|--------|
| Architecture & Structure | 🔴/🟡/🟢 | N findings |
| ↳ Overengineering | 🔴/🟡/🟢 | N findings |
| ↳ Dead Code | 🔴/🟡/🟢 | N findings |
| ↳ Bloated Files | 🔴/🟡/🟢 | N findings |
| Code Quality | 🔴/🟡/🟢 | N findings |
| ↳ Magic Values | 🔴/🟡/🟢 | N findings |
| Security | 🔴/🟡/🟢 | N findings |
| Performance | 🔴/🟡/🟢 | N findings |
| Dependencies | 🔴/🟡/🟢 | N findings |
| Tests | 🔴/🟡/🟢 | N findings |
| Documentation | 🔴/🟡/🟢 | N findings |
| Developer Experience | 🔴/🟡/🟢 | N findings |
| Simplify Scan | 🔴/🟡/🟢 | N findings |
| ↳ Standards Compliance | 🔴/🟡/🟢 | N% compliant |
| ↳ Clarity | 🔴/🟡/🟢 | N findings |
| ↳ Balance | 🔴/🟡/🟢 | N findings |

🔴 = critical/high issues found, 🟡 = medium issues only, 🟢 = low or no issues

---

## Critical & High Priority Issues

<List all CRITICAL and HIGH severity findings here, grouped by dimension>

### <Finding ID>: <Title>

- **Severity:** CRITICAL / HIGH
- **Dimension:** Architecture / Security / ...
- **Location:** `file/path.ext:line`
- **Evidence:**
  ```
  <actual code snippet showing the problem>
  ```
- **Impact:** <what breaks or could break>
- **Suggested Fix:**
  ```
  <code showing how to fix, or step-by-step instructions>
  ```
- **Effort:** S / M / L / XL
- **Related:** <links to related findings, if any>

---

## Medium Priority Issues

<Same format as above, for MEDIUM severity findings>

---

## Low Priority Issues

<Same format but can be more concise — table format acceptable>

| ID | Title | Location | Effort | Suggested Fix |
|----|-------|----------|--------|---------------|
| ... | ... | ... | ... | ... |

---

## Refactoring Roadmap

Based on the findings, here's a suggested sequence for tackling these issues:

### Phase 1: Critical Fixes (do immediately)
<List CRITICAL items — these are blocking or dangerous>

### Phase 2: High Priority (next sprint)
<List HIGH items — these cause significant friction>

### Phase 3: Medium Priority (planned work)
<Group MEDIUM items into logical refactoring tasks>

### Phase 4: Low Priority (opportunistic)
<LOW items to fix when touching nearby code>

---

## Dependency Audit Summary

| Package | Status | Current | Latest | Risk |
|---------|--------|---------|--------|------|
| ... | outdated/vulnerable/unused | ... | ... | ... |

---

## Statistics

- Total files scanned: N
- Total issues found: N
  - Critical: N
  - High: N
  - Medium: N
  - Low: N
- Estimated refactoring effort: <total>
- Most problematic files: <top 5 files with most issues>
```

---

## Step 8: Save and report

Save the report:

```bash
# Write AUDIT-REPORT.md to output directory
# Path: $SESSION_DIR/AUDIT-REPORT.md
```

Report to the user:

```
✅ Audit complete!
   Scope:    <full / quick / custom>
   Depth:    <surface / deep>
   Issues:   N total (C critical, H high, M medium, L low)
   Output:   <path to AUDIT-REPORT.md>

Next steps:
   - Review AUDIT-REPORT.md and prioritize
   - /hoangsa:menu  — design a refactoring task from the findings
   - /hoangsa:fix   — quick-fix a specific issue
```

---

## Rules

Universal rules live in `common.md §Universal rules`. Audit-specific additions:

| Rule | Detail |
|------|--------|
| **Evidence required** | Every finding must include file path + line number + code snippet — no vague claims |
| **No false alarms** | If uncertain, re-read the code to confirm before reporting. Mark uncertain findings with ⚠️ |
| **Severity consistency** | Use the severity rubric in Step 5 — don't inflate or deflate |
| **Actionable fixes** | Every finding must include a specific suggested fix, not just "refactor this" |
| **Effort estimation** | S=<1hr, M=1-4hr, L=4-8hr, XL=>8hr — estimate for a developer familiar with the codebase |
| **Parallel scanning** | Run dimension agents in parallel — do not scan sequentially |
| **hoangsa-memory first** | Use hoangsa-memory tools when available for more accurate dependency/impact analysis |
| **Redact secrets** | If secrets are found, report their existence but NEVER include actual values |
| **Respect scope** | Only scan dimensions the user selected — don't add unrequested dimensions |
| **Cross-reference** | Step 5 must run after all agents complete — don't skip deduplication |
| **Refactoring roadmap** | Always include a phased roadmap — the whole point is guiding refactoring |
