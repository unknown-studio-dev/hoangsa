# HOANGSA Init Workflow

> **Boot:** Read `$HOANGSA_ROOT/workflows/common.md` first — universal rules + CLI reference + self-verification template.

You are the onboarding agent. Mission: set up HOANGSA for this project — detect everything possible, ask only what can't be detected, save everything to config.

**Principles:** Detect before asking. Ask once, save forever. Respect user's time — batch questions where possible. Show what was detected, confirm, move on.

---

## Preamble: Resolve install path

```bash
# Resolve HOANGSA install path (local preferred over global)
if [ -x "./.claude/hoangsa/bin/hoangsa-cli" ]; then
  HOANGSA_ROOT="./.claude/hoangsa"
else
  HOANGSA_ROOT="$HOME/.claude/hoangsa"
fi
```

Use `$HOANGSA_ROOT` for all references to the HOANGSA install directory throughout this workflow.

---

## Step 0: Check if already initialized

If `.hoangsa/config.json` exists, read `lang` from it first. If config doesn't exist or `lang` is null, default to English for this interaction.

```bash
if [ -f ".hoangsa/config.json" ]; then
  INIT_LANG=$(node -e "try{const c=require('./.hoangsa/config.json');console.log(c.preferences&&c.preferences.lang||'en')}catch{console.log('en')}" 2>/dev/null || echo "en")
  echo "ALREADY_INITIALIZED"
  echo "LANG=$INIT_LANG"
else
  INIT_LANG="en"
  echo "FRESH"
  echo "LANG=en"
fi
```

If `ALREADY_INITIALIZED` — ask in `INIT_LANG` (vi → Vietnamese text, en → English):

Use AskUserQuestion (translate to INIT_LANG):
  question: "Project is already initialized. What would you like to do?"
  header: "Re-init"
  options:
    - label: "Re-scan codebase", description: "Keep preferences, only update codebase mapping"
    - label: "Full reset", description: "Delete existing config, set up from scratch"
    - label: "Cancel", description: "Do nothing"
  multiSelect: false

"Cancel" → stop. "Re-scan codebase" → skip to Step 3 (keep preferences + model config). "Full reset" → delete `.hoangsa/config.json`, continue from Step 1.

---

## Step 1: User preferences

### 1a. Communication language

Use AskUserQuestion:
  question: "Bạn muốn giao tiếp bằng ngôn ngữ nào?"
  header: "Language"
  options:
    - label: "Tiếng Việt", description: "Giao tiếp, giải thích, hỏi đáp bằng tiếng Việt"
    - label: "English", description: "Communicate, explain, discuss in English"
  multiSelect: false

Save as `lang` ("vi" or "en").

**Language enforcement:** From this point forward, ALL user-facing text in this workflow — questions, options, summaries, reports — **MUST** use the language the user just chose (`vi` → Vietnamese, `en` → English). Do not switch back to English mid-conversation.

### 1b. Spec language

Use AskUserQuestion:
  question: "Ngôn ngữ viết specs (DESIGN-SPEC, TEST-SPEC, RESEARCH.md)?"
  header: "Spec lang"
  options:
    - label: "Cùng ngôn ngữ giao tiếp", description: "Specs viết cùng ngôn ngữ đã chọn ở trên"
    - label: "Tiếng Việt", description: "Specs luôn viết bằng tiếng Việt"
    - label: "English", description: "Specs luôn viết bằng English — phổ biến cho team quốc tế"
  multiSelect: false

### 1c. Interaction level

Use AskUserQuestion:
  question: "Mức độ tương tác?"
  header: "Interaction"
  options:
    - label: "Detailed", description: "Hỏi kỹ từng bước — phù hợp khi mới dùng hoặc task phức tạp"
    - label: "Quick", description: "Dùng defaults, chỉ hỏi khi thật sự cần — cho user đã quen HOANGSA"
  multiSelect: false

### 1d. Review style

Use AskUserQuestion:
  question: "Review specs kiểu nào?"
  header: "Review"
  options:
    - label: "Toàn bộ document", description: "Xem cả spec rồi feedback 1 lần — nhanh hơn"
    - label: "Từng section", description: "Review từng phần (Overview, Types, APIs...) — kỹ hơn"
  multiSelect: false

---

## Step 2: Model routing

### 2a. Show profiles

Present the 3 profiles with cost context:

```
Model Profiles:

┌─────────────┬────────────┬────────────┬────────────┐
│ Role        │ quality    │ balanced   │ budget     │
├─────────────┼────────────┼────────────┼────────────┤
│ researcher  │ opus       │ sonnet     │ haiku      │
│ designer    │ opus       │ opus       │ sonnet     │
│ planner     │ opus       │ sonnet     │ haiku      │
│ orchestrator│ opus       │ opus       │ haiku      │
│ worker      │ opus       │ sonnet     │ haiku      │
│ reviewer    │ opus       │ sonnet     │ haiku      │
│ tester      │ sonnet     │ haiku      │ haiku      │
│ committer   │ sonnet     │ haiku      │ haiku      │
├─────────────┼────────────┼────────────┼────────────┤
│ Cost        │ $$$        │ $$         │ $          │
│ Quality     │ Best       │ Good       │ OK         │
└─────────────┴────────────┴────────────┴────────────┘

Roles:
  researcher   — research agents (codebase analysis, web search)
  designer     — menu workflow (write DESIGN-SPEC, TEST-SPEC)
  planner      — prepare workflow (decompose into tasks, DAG)
  orchestrator — cook/fix dispatch (routing, monitoring)
  worker       — implement code (the actual coding)
  reviewer     — semantic review (verify against spec)
  tester       — taste workflow (run tests, report)
  committer    — plate workflow (git commit)
```

### 2b. Choose profile

Use AskUserQuestion:
  question: "Chọn model profile?"
  header: "Profile"
  options:
    - label: "balanced (recommended)", description: "Opus cho design, Sonnet cho code, Haiku cho ops — cân bằng chất lượng/chi phí"
    - label: "quality", description: "Opus cho hầu hết — chất lượng cao nhất, tốn token nhất"
    - label: "budget", description: "Haiku/Sonnet — tiết kiệm token, phù hợp task đơn giản"
  multiSelect: false

### 2c. Per-role overrides (optional)

Use AskUserQuestion:
  question: "Muốn override model cho role nào không?"
  header: "Overrides"
  options:
    - label: "Không cần", description: "Dùng profile mặc định — có thể thay đổi sau"
    - label: "Có, tôi muốn tuỳ chỉnh", description: "Chọn model cho từng role cụ thể"
  multiSelect: false

If "Có":

For each role the user wants to override, use AskUserQuestion:
  question: "Model cho <role>?"
  header: "<role>"
  options:
    - label: "opus", description: "Mạnh nhất — tốn token nhất"
    - label: "sonnet", description: "Cân bằng — phù hợp hầu hết"
    - label: "haiku", description: "Nhanh, rẻ — cho task đơn giản"
    - label: "Giữ default", description: "Dùng theo profile đã chọn"
  multiSelect: false

---

## Step 3: Codebase detection

> **IMPORTANT — DO NOT use raw `[ -f ... ] && ...` in Bash.**
> Claude Code joins multi-line bash with `&&`, so any failed `[ -f ]` test (exit code 1) kills the entire chain.
>
> **Safe patterns:**
> - `[ -f "file" ] && echo "found" || true` — append `|| true`
> - `for f in a b c; do [ -f "$f" ] && echo "$f"; done` — loop absorbs failures
> - **Prefer Glob/Grep/Read tools** over bash for file detection — they never fail on missing files
>
> The detection sections below describe **WHAT to detect**. Use whichever tool is safest:
> - **Glob** for checking if config files exist (e.g., `Glob("*.config.*")`)
> - **Grep** for scanning file contents (e.g., dependencies in package.json)
> - **Read** for parsing manifest files (package.json, Cargo.toml, etc.)
> - **Bash** only for commands that need shell (e.g., `git log`, `node -e`), always with `|| true`

### 3a. Determine project state

```bash
MANIFESTS=""
for f in package.json Cargo.toml pyproject.toml requirements.txt setup.py go.mod pom.xml build.gradle build.gradle.kts Gemfile mix.exs composer.json; do
  [ -f "$f" ] && MANIFESTS="$MANIFESTS $f"
done
GIT_LOG_COUNT=$(git log --oneline 2>/dev/null | head -5 | wc -l || echo 0)
echo "MANIFESTS=$MANIFESTS"
echo "GIT_LOG_COUNT=$GIT_LOG_COUNT"
```

Decision tree:
- **Has manifests** → Flow A (auto-detect)
- **No manifests but has code** (git commits > 0 or source files exist) → Flow A-lite (file extension scan)
- **Empty project** (no manifests, no commits, no source files) → Flow B (scaffold)

---

### Flow A: Auto-detect existing project

Read `$HOANGSA_ROOT/workflows/init-detect.md` — it holds detection categories
A1–A21 (runtime versions, package manager, frameworks, DB/ORM, API style,
styling, bundler, testing, state management, auth, monitoring, queues, cloud,
docs, monorepo, scripts, CI/CD, git conventions, linters, infrastructure,
entry points). Run the categories in parallel subagents where possible — each
is independent — and collect their outputs for Step 4.

---

### Flow A-lite: Code exists but no manifests

When source files exist but no standard manifest, use Glob to count by extension:

```bash
echo "ts/tsx:$(find . -maxdepth 4 \( -name '*.ts' -o -name '*.tsx' \) ! -path '*/node_modules/*' 2>/dev/null | wc -l)"
echo "js/jsx:$(find . -maxdepth 4 \( -name '*.js' -o -name '*.jsx' \) ! -path '*/node_modules/*' 2>/dev/null | wc -l)"
echo "py:$(find . -maxdepth 4 -name '*.py' ! -path '*/__pycache__/*' 2>/dev/null | wc -l)"
echo "rs:$(find . -maxdepth 4 -name '*.rs' 2>/dev/null | wc -l)"
echo "go:$(find . -maxdepth 4 -name '*.go' 2>/dev/null | wc -l)"
echo "java/kt:$(find . -maxdepth 4 \( -name '*.java' -o -name '*.kt' \) 2>/dev/null | wc -l)"
echo "rb:$(find . -maxdepth 4 -name '*.rb' 2>/dev/null | wc -l)"
echo "php:$(find . -maxdepth 4 -name '*.php' 2>/dev/null | wc -l)"
echo "c/cpp:$(find . -maxdepth 4 \( -name '*.c' -o -name '*.cpp' -o -name '*.h' \) 2>/dev/null | wc -l)"
find . -maxdepth 4 -type f \( -name "*.swift" \) | wc -l
find . -maxdepth 4 -type f \( -name "*.dart" \) | wc -l
```

Detect stack from file counts. Still run A3–A21 for CI, git, linter, infra detection. Build/test/lint commands will be `null` — user must provide.

---

### Flow B: New empty project

No code detected. Ask the user to define the project:

#### B1. What stack?

Use AskUserQuestion:
  question: "Tech stack cho project mới?"
  header: "Stack"
  options:
    - label: "TypeScript/Node", description: "TypeScript với Node.js runtime"
    - label: "Python", description: "Python (FastAPI, Django, Flask...)"
    - label: "Rust", description: "Rust (Axum, Actix, Tokio...)"
    - label: "Go", description: "Go (Gin, Echo, Fiber...)"
  multiSelect: true
  (user chọn Other cho stack khác)

#### B2. Architecture

Use AskUserQuestion:
  question: "Kiến trúc project?"
  header: "Architecture"
  options:
    - label: "Single package", description: "1 project đơn — đủ cho hầu hết use cases"
    - label: "Monorepo", description: "Nhiều packages trong 1 repo — cho project lớn hoặc fullstack"
  multiSelect: false

If monorepo → ask how many packages and their names/stacks.

#### B3. Build/Test commands

For each stack selected, show defaults and ask to confirm:

```
Defaults cho TypeScript:
  Build: npm run build
  Test:  npx jest
  Lint:  npx eslint .

Dùng defaults? [OK / Thay đổi]
```

#### B4. Git convention

Use AskUserQuestion:
  question: "Git commit convention?"
  header: "Git"
  options:
    - label: "Conventional Commits", description: "feat(scope): message — chuẩn phổ biến nhất"
    - label: "Free-form", description: "Không convention cố định"
    - label: "Ticket prefix", description: "[TICKET-123] message — cho team dùng Jira/Linear"
  multiSelect: false

#### B5. CI

Use AskUserQuestion:
  question: "CI/CD platform?"
  header: "CI/CD"
  options:
    - label: "GitHub Actions", description: ".github/workflows/"
    - label: "GitLab CI", description: ".gitlab-ci.yml"
    - label: "Chưa cần", description: "Setup CI sau"
  multiSelect: false

---

## Step 4: Show detection summary + confirm

Present everything detected (or specified):

```
📋 HOANGSA Init Summary

Preferences:
  Giao tiếp:   Tiếng Việt
  Specs:       English
  Interaction: Quick
  Review:      Toàn bộ document

Model Profile: balanced
  designer → opus | worker → sonnet | tester → haiku
  (no overrides)

Codebase:
  Stacks:    [TypeScript, Python]
  Monorepo:  Yes (pnpm workspaces)
  Packages:
    📦 api        (packages/api)     — TypeScript
       build: npm run build | test: npx jest | lint: npx eslint .
    📦 web        (packages/web)     — TypeScript
       build: npm run build | test: npx jest | lint: npx eslint .
    📦 ml-service (packages/ml)      — Python
       build: — | test: pytest | lint: ruff check .

  CI:        GitHub Actions
  Git:       Conventional Commits
  Infra:     Docker, docker-compose
  Linters:   eslint, prettier, ruff

  Worker Rules addons detected: [react, typescript, python]
    (auto-loaded at runtime by cook/fix workflows)

OK? [Confirm / Sửa]
```

**Addon detection logic:** Before showing the summary, match detected stacks and frameworks against addon frontmatter `frameworks` fields. For each addon in `templates/workflows/worker-rules/addons/`, an addon applies if any value in its `frameworks` list matches:
- `config.json` `preferences.tech_stack` entries (e.g., `"typescript"`, `"python"`)
- Any `frameworks` key from detected framework detection (A3) — e.g., `"react"`, `"nestjs"`, `"django"`
- Any package-level framework from `codebase.packages[].frameworks`

List only matching addon names in the summary line. If none match, omit the line.

Use AskUserQuestion:
  question: "Config có OK không?"
  header: "Confirm"
  options:
    - label: "OK — lưu", description: "Lưu config và bắt đầu dùng HOANGSA"
    - label: "Sửa preferences", description: "Quay lại sửa ngôn ngữ / interaction / review"
    - label: "Sửa model", description: "Quay lại sửa profile / overrides"
    - label: "Sửa codebase", description: "Sửa stack / build / test commands"
  multiSelect: false

If "OK" → proceed to Step 4b (addon selection).
If "Sửa ..." → jump back to the relevant step, re-run from there.

---

## Step 4b: Addon selection (post-detection)

After user confirms config, offer addon customization. Auto-detected addons may miss frameworks (e.g., NestJS detected only as "typescript"). This step lets users add/remove addons before saving.

```bash
ADDON_LIST=$("$HOANGSA_ROOT/bin/hoangsa-cli" addon list .)
```

Parse available addons and the auto-detected active list. Show:

```
Worker-rules addons (auto-detected):
  ✅ rust — rust, axum, actix-web, rocket, warp, tokio, leptos, tauri
  ✅ javascript — javascript, nodejs, node, bun

  Có muốn thêm/bớt addons?
```

Use AskUserQuestion:
  question: "Addons auto-detected xong. Muốn chỉnh không?"
  header: "Addons"
  options:
    - label: "OK — giữ nguyên", description: "Dùng addons đã detect — recommended"
    - label: "Thêm addons", description: "Chọn thêm addons từ danh sách available"
    - label: "Bỏ addons", description: "Xóa addons không cần thiết"
  multiSelect: false

If "OK" → proceed to Step 5.

If "Thêm addons" → show multi-select of inactive addons:
  Use AskUserQuestion:
    question: "Chọn addons muốn thêm:"
    header: "Add"
    options: (up to 4 inactive addons, sorted by relevance to detected stack)
    multiSelect: true

  For selected addons:
  ```bash
  "$HOANGSA_ROOT/bin/hoangsa-cli" addon add . '["addon1","addon2"]'
  ```

If "Bỏ addons" → show multi-select of active addons:
  Use AskUserQuestion:
    question: "Chọn addons muốn bỏ:"
    header: "Remove"
    options: (active addons)
    multiSelect: true

  For selected addons:
  ```bash
  "$HOANGSA_ROOT/bin/hoangsa-cli" addon remove . '["addon1"]'
  ```

After changes, show updated addon list and proceed to Step 5.

---

## Step 5: Save config

Write `.hoangsa/config.json`:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" config set . '<full config JSON>'
```

After config save, verify by reading back `.hoangsa/config.json`:

```bash
node -e "try{const c=require('./.hoangsa/config.json');console.log('CONFIG_OK');console.log(JSON.stringify(c))}catch(e){console.log('CONFIG_FAIL');console.log(e.message)}" 2>/dev/null
```

If read-back fails or content doesn't match the intended config, warn user and offer to retry or save manually (e.g., "Config save could not be verified. Would you like to retry, or save the config manually by copying the JSON?").

Full config structure:

```json
{
  "profile": "balanced",
  "model_overrides": {},
  "preferences": {
    "lang": "vi",
    "spec_lang": "en",
    "tech_stack": ["typescript", "python"],
    "interaction_level": "quick",
    "auto_taste": null,
    "auto_plate": null,
    "auto_serve": null,
    "research_scope": null,
    "research_mode": null,
    "review_style": "whole_document"
  },
  "codebase": {
    "monorepo": true,
    "packages": [
      {
        "name": "api",
        "path": "packages/api",
        "stack": "typescript",
        "build": "npm run build",
        "test": "npx jest",
        "lint": "npx eslint .",
        "dev": "npm run dev"
      },
      {
        "name": "ml-service",
        "path": "packages/ml",
        "stack": "python",
        "build": null,
        "test": "pytest",
        "lint": "ruff check .",
        "dev": null
      }
    ],
    "ci": "github-actions",
    "git_convention": "conventional-commits",
    "linters": ["eslint", "prettier", "ruff"],
    "infra": ["docker", "docker-compose"],
    "entry_points": ["packages/api/src/index.ts", "packages/ml/main.py"]
  },
  "task_manager": {
    "provider": null,
    "mcp_server": null,
    "verified": false,
    "verified_at": null,
    "project_id": null,
    "default_list": null
  }
}
```

---

## Step 5b: Generate project-level worker rules

After saving config, generate `.hoangsa/worker-rules.md` using the Write tool.

The file is a short project-specific header that references which worker-rules addons will be auto-loaded at runtime (by cook/fix workflows) based on the detected stack. Do NOT copy addon content here — addons are loaded at runtime.

Template:

```markdown
# Worker Rules — <project name or repo dir>

Project-level worker rules. Extends the HOANGSA base worker-rules with addons matched to this project's stack.

## Detected addons

The following addons will be auto-loaded at runtime based on this project's tech stack:

- **react** — matches: react, react-native, expo
- **typescript** — matches: typescript
- **python** — matches: python, django, fastapi, flask

_(addon matching: `frameworks` field in each addon's frontmatter vs `tech_stack` + detected frameworks in config.json)_

## Project overrides

Add any project-specific rule overrides below. These take priority over base worker-rules and addons.

<!-- Example:
- Prefer `yarn` over `npm` for all package installs
- Use `src/__tests__/` for test file placement (not colocated)
-->
```

Replace the addon list with only the addons actually detected for this project. If no addons match, write "No framework-specific addons detected — base worker-rules apply."

---

## Step 6: Chain preferences (optional — quick setup)

For chain preferences (`auto_taste`, `auto_plate`, `auto_serve`), present them as a batch instead of asking one at a time later:

Use AskUserQuestion:
  question: "Auto-chain preferences — chạy tự động sau mỗi bước?"
  header: "Auto-chain"
  options:
    - label: "Recommended", description: "auto_taste=on, auto_plate=off, auto_serve=off — test tự động, commit thủ công"
    - label: "Full auto", description: "Tất cả on — cook → taste → plate tự động"
    - label: "Manual", description: "Tất cả off — tôi sẽ gọi từng command"
    - label: "Tuỳ chỉnh", description: "Chọn on/off cho từng chain"
  multiSelect: false

If "Tuỳ chỉnh" → ask each one individually.
Otherwise → save based on choice.

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" pref set . auto_taste true
"$HOANGSA_ROOT/bin/hoangsa-cli" pref set . auto_plate false
"$HOANGSA_ROOT/bin/hoangsa-cli" pref set . auto_serve false
```

---

## Step 6b: Workflow quality & optimization config

Configure quality and optimization settings that control cook/fix/menu/prepare workflows.

These 6 settings are the same keys controlled by `pref set . profile <name>` presets. Init lets the user pick a preset or customize individually.

Use AskUserQuestion:
  question: "Cấu hình quality & optimization cho workflow?"
  header: "Quality"
  options:
    - label: "Recommended", description: "quality_gate=on, simplify=on, test_runs=1, research=inline, context=selective, memory_strict=off"
    - label: "Strict", description: "Tất cả on/full, test_runs=2 — chất lượng cao nhất, tốn token"
    - label: "Minimal", description: "Tất cả off/inline/selective — nhanh, tiết kiệm token"
    - label: "Tuỳ chỉnh", description: "Chọn on/off cho từng setting"
  multiSelect: false

Preset values (save each via `"$HOANGSA_ROOT/bin/hoangsa-cli" pref set . <key> <value>`):

| Key | Recommended | Strict | Minimal |
|-----|-------------|--------|---------|
| `quality_gate` | true | true | false |
| `simplify_pass` | true | true | false |
| `test_runs` | 1 | 2 | 1 |
| `research_mode` | inline | full | inline |
| `context_mode` | selective | full | selective |
| `memory_strict` | false | true | false |

If "Tuỳ chỉnh" → ask each setting individually.

First batch (up to 4 questions per AskUserQuestion call):

Use AskUserQuestion:
  1. question: "Quality gate — chạy kiểm tra chất lượng sau mỗi task?"
     header: "Gate"
     options:
       - label: "On", description: "Review code quality sau mỗi task trong cook/fix"
       - label: "Off", description: "Bỏ qua quality gate — nhanh hơn"
  2. question: "Simplify pass — tự động simplify code sau khi implement?"
     header: "Simplify"
     options:
       - label: "On", description: "Chạy simplify pass để clean up code"
       - label: "Off", description: "Bỏ qua simplify — giữ nguyên code"
  3. question: "Số lần chạy test cho mỗi task?"
     header: "Test runs"
     options:
       - label: "1", description: "Chạy 1 lần — đủ cho hầu hết cases"
       - label: "2", description: "Chạy 2 lần — phát hiện flaky tests"
       - label: "3", description: "Chạy 3 lần — strict, cho CI-grade confidence"
  4. question: "Research mode — cách chạy research trong menu workflow?"
     header: "Research"
     options:
       - label: "inline (recommended)", description: "Research ngay trong context — nhanh, ít token"
       - label: "full", description: "Spawn subagent chạy /hoangsa:research đầy đủ — kỹ hơn, tốn token"

Second batch:

Use AskUserQuestion:
  1. question: "Context mode — cách load context cho worker trong prepare?"
     header: "Context"
     options:
       - label: "selective (recommended)", description: "Chỉ load context liên quan đến task — tiết kiệm token"
       - label: "full", description: "Load toàn bộ context pack — đầy đủ nhất, tốn token"
  2. question: "hoangsa-memory strict — bắt buộc impact analysis trước mỗi edit?"
     header: "hoangsa-memory"
     options:
       - label: "Off (recommended)", description: "hoangsa-memory là khuyến khích, worker có thể skip để tiết kiệm token"
       - label: "On", description: "Bắt buộc memory_impact/memory_recall trước mỗi edit — an toàn hơn, tốn token"

Save each setting via `"$HOANGSA_ROOT/bin/hoangsa-cli" pref set .` accordingly.

---

## Step 6c: Research scope

Configure what sources the `/hoangsa:research` workflow searches:

Use AskUserQuestion:
  question: "Research scope — nguồn nào khi chạy /hoangsa:research?"
  header: "Scope"
  options:
    - label: "both (recommended)", description: "Codebase + web search — đầy đủ nhất"
    - label: "codebase", description: "Chỉ phân tích codebase — không search web"
    - label: "web", description: "Chỉ search web — không phân tích codebase"
  multiSelect: false

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" pref set . research_scope "<chosen value>"
```

---

## Step 7: hoangsa-memory index

If project has code (Flow A or A-lite):

```
Indexing codebase with hoangsa-memory...
```

```bash
timeout 120 hoangsa-memory --json index . && rm -f .hoangsa/memory/.outdated && echo "MEMORY_OK" || echo "MEMORY_FAIL"
```

After successful indexing, warm up the memory and show skills:

```
memory_wakeup()
memory_skills_list()
```

Display installed hoangsa-memory skills in the Step 8 report.

If `hoangsa-memory index` fails or times out (>120s), warn user: "hoangsa-memory indexing failed. You can retry later with `/hoangsa:index`." Continue with remaining steps — indexing is non-blocking.

If project is empty (Flow B):

```
Project mới — skip indexing. Chạy /hoangsa:index sau khi có code.
```

---

## Step 7b: Seed memory-guidance pointer

Seed `.hoangsa/memory-guidance.md` and inject the pointer block into
project-level `CLAUDE.md` + `AGENTS.md` so Claude Code and subagents load
hoangsa-memory usage instructions at SessionStart. Idempotent — re-running
replaces the block between `<!-- hoangsa-memory-start -->` / `<!-- hoangsa-memory-end -->`
markers without touching anything else.

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" memory-guidance sync .
```

Run in both Flow A and Flow B — an empty project still benefits from the
pointer so the first agent to land on the repo knows the memory tools are
available.

---

## Step 8: Report

Retrieve memory and skills data for the report:
```
memory_show()
```

```
✅ HOANGSA initialized!

   Config:       .hoangsa/config.json
   Profile:      balanced
   Stacks:       [TypeScript, Python]
   Packages:     3
   Worker rules: .hoangsa/worker-rules.md (addons: react, typescript, python)
   hoangsa-memory:        ✅ indexed (148 symbols)
   Memory: <N> facts, <N> lessons
   Skills: <list of installed hoangsa-memory skills>

   Get started:
     /hoangsa:menu     — design a new feature
     /hoangsa:fix      — fix a bug
     /hoangsa:research — explore the codebase
     /hoangsa:check    — view session status
     /hoangsa:help     — show all commands
```

---

## Rules

Universal rules live in `common.md §Universal rules`. Init-specific additions:

| Rule | Detail |
|------|--------|
| **Detect before asking** | Auto-detect everything possible from filesystem |
| **Batch questions** | Group related questions, don't ask one at a time |
| **Show summary before saving** | User confirms the full config before write |
| **Handle empty projects** | Flow B scaffolds config for projects with no code yet |
| **Handle re-init** | Offer to keep preferences when re-scanning codebase |
