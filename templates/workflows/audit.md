# Audit Workflow

Perform a comprehensive codebase audit across 8 dimensions, producing a detailed AUDIT-REPORT.md that teams can use as a refactoring roadmap.

**Principles:** Parallel scanning for speed. Evidence-based — every finding must include file paths, line numbers, and concrete examples. Severity-rated so teams can prioritize. Actionable — each finding includes a suggested fix. Use GitNexus when available, fall back gracefully.

---

## Step 0a: Language enforcement

```bash
# Resolve HOANGSA install path (local preferred over global)
if [ -x "./.claude/hoangsa/bin/hoangsa-cli" ]; then
  HOANGSA_ROOT="./.claude/hoangsa"
else
  HOANGSA_ROOT="$HOME/.claude/hoangsa"
fi

LANG_PREF=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . lang)
```

All user-facing text — questions, reports, summaries, error messages — **MUST** use the language from `lang` preference (`vi` → Vietnamese, `en` → English, `null` → default English). This applies throughout the **ENTIRE** workflow. Do not switch languages mid-conversation. Template examples in this workflow are illustrative — adapt them to match the user's `lang` preference.

---

## Step 0b: GitNexus index check (interactive)

```bash
if [ ! -d ".gitnexus" ]; then
  echo "GITNEXUS_MISSING"
elif [ -f ".gitnexus/.outdated" ] && [ "$(cat .gitnexus/.outdated 2>/dev/null | python3 -c 'import sys,json; d=json.load(sys.stdin); print(len(d.get("changed_files",[])))' 2>/dev/null)" != "0" ]; then
  echo "GITNEXUS_OUTDATED"
else
  echo "GITNEXUS_AVAILABLE"
fi
```

Store result as `GITNEXUS_STATUS`.

If `GITNEXUS_AVAILABLE` or after sync completes, resolve the repo name:

```bash
GITNEXUS_REPO=$(cat .gitnexus/meta.json 2>/dev/null | python3 -c 'import sys,json,os; m=json.load(sys.stdin); print(os.path.basename(m.get("repoPath","")))' 2>/dev/null || basename "$(pwd)")
```

Store as `GITNEXUS_REPO`. Pass both `GITNEXUS_STATUS` and `GITNEXUS_REPO` to all audit agent prompts.

- If `GITNEXUS_AVAILABLE` → continue. Audit agents will use GitNexus for dependency graph, dead code detection, and architectural analysis.
- If `GITNEXUS_MISSING` or `GITNEXUS_OUTDATED` → ask the user:

  Use AskUserQuestion:
    question: "GitNexus index bị outdated/missing. Sync lại để audit sâu hơn?"
    header: "GitNexus"
    options:
      - label: "Sync ngay", description: "Chạy gitnexus analyze (~30s) — có dependency graph, dead code detection, architecture analysis"
      - label: "Bỏ qua", description: "Audit dùng Grep/Glob — vẫn scan được nhưng thiếu dependency graph analysis"
    multiSelect: false

  If user chọn "Sync ngay":
    ```bash
    npx gitnexus analyze --embeddings
    ```
    Set `GITNEXUS_STATUS` = `GITNEXUS_AVAILABLE`.

  If user chọn "Bỏ qua" → set `GITNEXUS_STATUS` = `GITNEXUS_UNAVAILABLE`, continue.

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

## Step 2e: Media detection (auto)

Scan **two sources** for media files:

1. **User's input** — file paths or pasted screenshots/videos in the message
2. **Task-link attachments** — files downloaded to `$SESSION_DIR/attachments/` by task-link detection (if a task URL was provided)

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
- Note the image paths — use as visual evidence when scanning relevant dimensions (e.g., UI/UX issues, layout problems)

**If videos detected (from either source):**
1. Invoke the `visual-debug` skill for video processing:
   - Check ffmpeg availability: `hoangsa-cli media check-ffmpeg`
   - If available: `hoangsa-cli media analyze <video_path> --output-dir /tmp/hoangsa-audit-<timestamp>`
   - Read the output `montage.png` (annotated frame grid with timestamps)
   - Read the output `diff-montage.png` (red overlay showing changes between frames)
2. Include visual analysis findings as context for dimension scanning agents

**If no media detected (from either source):** Skip this step, proceed to Step 3.

---

## Step 3: Detect project metadata

Before scanning, gather project context for accurate analysis:

```
- Read package.json / Cargo.toml / pyproject.toml / go.mod → tech stack, versions
- Detect framework (React, Next.js, Express, Actix, Django, etc.)
- Count total files by type (*.js, *.ts, *.rs, *.py, etc.)
- Detect test framework (jest, vitest, cargo test, pytest, etc.)
- Check for CI/CD config (.github/workflows/, .gitlab-ci.yml, etc.)
- Check for linter/formatter config (.eslintrc, prettier, rustfmt, ruff, etc.)
- Detect monorepo structure (workspaces, lerna, turborepo, etc.)
```

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
.git/            .gitnexus/     .hoangsa/
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

Launch up to 9 parallel scanning agents based on `AUDIT_SCOPE`. Use the **Agent tool** to spawn one subagent per dimension. Each agent receives:
- `PROJECT_META`, `AUDIT_EXCLUDES`, and `AUDIT_PATHS` for context
- The dimension spec (what to scan for, from the dimension definitions below)
- Instructions to output findings as a JSON array of `{id, file, line, severity, title, evidence, impact, suggestion, effort}`

Agents must:
- Only scan files within `AUDIT_PATHS` (use these as base directories for Grep/Glob)
- Skip all files matching `AUDIT_EXCLUDES` — do not read, grep, or report findings from excluded paths

Example agent invocation:
```
Agent tool → subagent per dimension
  Input: dimension spec + PROJECT_META + AUDIT_EXCLUDES + AUDIT_PATHS
  Output: JSON array of findings [{id: "ARCH-001", file: "src/api.ts", line: 42, severity: "HIGH", title: "...", evidence: "...", impact: "...", suggestion: "...", effort: "M"}]
```

**Dimensions:**
1. Architecture & Structure (includes bloated files, dead code, overengineering, module inconsistency)
2. Code Smells & Anti-patterns (includes magic values, primitive obsession)
3. Security
4. Performance
5. Dependency Health
6. Test Quality
7. Documentation
8. Developer Experience
9. Simplify Scan — codebase-wide (4 criteria: preserve functionality, project standards, clarity, balance)

### Model selection

```bash
MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model researcher 2>/dev/null || echo "sonnet")
```

Use the resolved model for all scanning agents.

---

### Dimension 1: Architecture & Structure

Goal: Identify structural problems that make the codebase hard to understand, maintain, or extend.

```
Scan for:

1. CIRCULAR DEPENDENCIES
   - Trace import graphs: A → B → C → A
   - For JS/TS: follow import/require statements across files
   - For Rust: check mod/use relationships in modules
   - If GitNexus available: use gitnexus_cypher({repo: GITNEXUS_REPO}) to query dependency cycles
   - If GitNexus unavailable: use Grep to trace import/require statements across files, building a dependency graph manually. Start from high-fan-in files and follow import chains.
   - Evidence: list the cycle chain with file paths

2. BLOATED FILES / GOD FILES / GOD CLASSES
   - Files with >300 lines of logic (warning), >500 lines (critical) — exclude config/generated/test files
   - Classes/modules with >10 public methods or >20 total methods
   - Files that are imported by >10 other files (high fan-in = single point of failure)
   - Files mixing multiple concerns (e.g., routing + business logic + DB queries in one file)
   - Single file handling >3 distinct responsibilities — each responsibility should be its own module
   - Measure: count functions, count lines, count imports, count responsibilities
   - Evidence: file path, line count, number of exports/methods, list of importers, list of distinct responsibilities found

3. LAYERING VIOLATIONS
   - Business logic in controllers/handlers (should be in services/domain)
   - Database queries outside of repository/data layer
   - Direct HTTP calls in business logic (should be in adapters)
   - UI components containing business logic
   - Evidence: file path, line numbers, what layer it belongs to vs where it is

4. INCONSISTENCY ACROSS MODULES
   This is about modules in the same project that solve similar problems in different ways — making onboarding confusing and refactoring risky.

   - STRUCTURAL INCONSISTENCY
     - Some modules use flat file structure, others use nested directories
     - Some features co-locate tests, others put tests in a separate tree
     - Inconsistent index/barrel file patterns (some modules have index.ts, others don't)
     - Evidence: compare directory layouts of 2+ similar modules

   - PATTERN INCONSISTENCY
     - Error handling differs between modules: one throws, another returns Result/Option, another returns null
     - Async patterns differ: one module uses async/await, another uses callbacks, another uses .then()
     - Data validation done at different layers: some validate at controller, some at service, some not at all
     - State management differs: one module uses global store, another passes props, another uses context
     - Evidence: module A file:line (pattern X) vs module B file:line (pattern Y)

   - NAMING INCONSISTENCY
     - Same concept named differently: "user" in one module, "account" in another, "member" in third
     - Function naming: getUserById vs fetchUser vs loadUserData vs findUser — pick one convention
     - File naming: some PascalCase, some kebab-case, some camelCase in the same project
     - Export naming: some default exports, some named exports, no consistent rule
     - Evidence: list of naming variants for the same concept across modules

   - API STYLE INCONSISTENCY
     - REST endpoints: some use /api/v1/users, others use /users, others use /api/user
     - Response format: some return {data: ...}, others return {result: ...}, others return raw
     - Config loading: some read env vars directly, others use a config module, others use dotenv inline
     - Logging: some use console.log, others use a logger, others use debug()
     - Evidence: file:line for each variant, suggested unified pattern

   - DEPENDENCY INCONSISTENCY
     - Same problem solved by different packages in different modules (axios + fetch + got)
     - Different versions of the same package in monorepo workspaces
     - Some modules pin exact versions, others use ranges
     - Evidence: package list per module showing the inconsistency

   How to scan:
     - Pick 3-5 modules/features of similar complexity
     - For each: note error handling, async pattern, naming, file structure, config approach
     - Create a comparison matrix — any column with >1 variant = inconsistency
     - Rate severity by how confusing it is for a new developer

5. DEAD CODE & ZOMBIE CODE
   - Exported symbols with zero importers (if GitNexus available: gitnexus_cypher({repo: GITNEXUS_REPO}) to find orphan nodes; if GitNexus unavailable: Grep for `export` declarations, then Grep for each exported name across all files — zero matches = dead export)
   - Files not imported anywhere — entire modules nobody calls
   - Functions defined but never invoked (grep for definition, then grep for usage — 0 hits = dead)
   - Feature flags that are always on/off (grep for the flag, check all branches — if only one branch ever runs, the other is dead)
   - Commented-out code blocks >5 lines — this is not "backup", it's noise (git has history)
   - TODO/FIXME/HACK comments older than 6 months (check git blame) — these are zombie tasks, either do them or delete them
   - Unused variables, unused imports (for languages without compiler warnings)
   - Unreachable code after return/throw/break statements
   - Deprecated functions still in codebase but no longer called
   - Test files for deleted source files
   - Evidence: file path, symbol name, confirmation of zero references, git blame date for stale comments

6. OVERENGINEERING / FAKE ARCHITECTURE (Kiến trúc giả cầy)
   This is about code that looks sophisticated but adds complexity without real value — architecture theater.

   - PREMATURE ABSTRACTION
     - Interfaces/traits/abstract classes with only 1 implementation — if there's only ever one impl, the abstraction is a tax on readability
     - Generic/template parameters used in only 1 concrete type
     - Factory patterns that create only 1 product — a constructor would do
     - Strategy/plugin patterns with only 1 strategy ever registered
     - Evidence: interface file:line, the single implementation file:line

   - INDIRECTION FOR INDIRECTION'S SAKE
     - Wrapper functions that just call another function without adding logic (pass-through wrappers)
     - Files that only re-export from another file (barrel files with no aggregation value)
     - Middleware/interceptor chains with only 1 middleware
     - Base classes that exist only to be extended by 1 child
     - Service → Repository → DAO → DB chain when Service → DB would suffice
     - Evidence: the wrapper/indirection, what it wraps, proof that it adds nothing

   - DESIGN PATTERN ABUSE
     - Singleton used where a plain module-level variable works
     - Observer/EventEmitter for communication between 2 components that could just call each other
     - Builder pattern for objects with <4 fields
     - Command pattern wrapping simple function calls
     - DI container for <5 dependencies (manual injection is fine)
     - Evidence: the pattern, where it's used, simpler alternative

   - UNNECESSARY ABSTRACTION LAYERS
     - >3 layers between user input and actual work (Controller → Service → Manager → Handler → Worker)
     - Abstract base classes with only abstract methods (that's just an interface/trait, use one)
     - Utility/helper classes that could be standalone functions
     - "Manager", "Handler", "Processor", "Engine" classes that manage only 1 thing
     - Evidence: trace the call chain from entry to actual logic, count the hops

   - CONFIG/TYPE OVERKILL
     - Complex config schemas for things with only 2-3 options
     - Type hierarchies >3 levels deep for simple data
     - Enum with only 2 values where a boolean would suffice
     - Custom error types for errors that are never specifically caught
     - Evidence: the over-complex type/config, what it could be simplified to

   Scoring guide: count the ratio of "abstraction code" (interfaces, base classes, factories, wrappers) vs "real work code" (actual logic). If >30% of a module is abstraction scaffolding, it's likely overengineered.

7. NAMING INCONSISTENCIES
   - Mixed naming conventions (camelCase vs snake_case in same language)
   - Inconsistent file naming (PascalCase.js vs kebab-case.js)
   - Misleading names (function name doesn't match what it does)
   - Evidence: examples of inconsistencies with file paths
```

Output format per finding:
```
- ID: ARCH-001
- Severity: CRITICAL | HIGH | MEDIUM | LOW
- Title: <short description>
- Location: <file:line>
- Evidence: <concrete example from code>
- Impact: <what breaks or gets harder because of this>
- Suggested fix: <specific action to take>
- Effort: S | M | L | XL
```

---

### Dimension 2: Code Smells & Anti-patterns

Goal: Find patterns that indicate deeper design problems or make code hard to maintain.

```
Scan for:

1. DUPLICATION
   - Near-identical functions/blocks across files (>10 lines similar)
   - Copy-pasted logic with minor variations
   - Evidence: file paths, line ranges, diff between duplicates
   - How to detect: Use Grep to find distinctive code patterns (function signatures, unique string literals), then Read both files to compare. Focus on files with similar names or in similar module positions.

2. LONG FUNCTIONS
   - Functions/methods >50 lines
   - Deeply nested logic (>4 levels of indentation)
   - Evidence: function name, file:line, line count, nesting depth
   - How to detect: Use Grep with regex to find function/method declarations (e.g., `function\s+\w+|=>\s*\{|def\s+\w+`), then Read to count lines between opening and closing braces. For nesting, use Grep to find lines with 4+ levels of indentation (e.g., `^\s{16,}` or `^\t{4,}`).

3. MAGIC VALUES (Numbers & Strings)
   This is one of the most common and insidious code smells — values with implicit meaning scattered through code.

   - MAGIC NUMBERS
     - Numeric literals in conditions: `if (status === 3)` — what does 3 mean?
     - Timeout/retry values: `setTimeout(fn, 86400000)` — is that 1 day? Name it
     - Array indices with implicit meaning: `parts[2]` — what's at index 2?
     - Bit flags/masks: `flags & 0x04` — define named constants
     - Threshold values: `if (score > 0.85)` — why 0.85? Name and document it
     - Acceptable: 0, 1, -1, 100 (percentage), common HTTP status codes in context
     - Evidence: file:line, the magic number, what it likely means, suggested constant name

   - MAGIC STRINGS
     - String comparisons: `if (type === "premium_v2")` — use an enum/constant
     - Event names: `emit("user-data-loaded")` — define in a constants file
     - Config keys: `config["db.pool.max"]` — use typed config objects
     - Error messages used as control flow: `if (err.message.includes("not found"))` — use error codes/types
     - API endpoints hardcoded: `fetch("/api/v2/users")` — centralize route definitions
     - CSS class names in JS: `element.classList.add("active-state")` — use CSS modules or constants
     - Evidence: file:line, the magic string, how many places it appears, suggested approach

   - SCATTERED CONSTANTS
     - Same magic value appears in >2 files — proves it should be a shared constant
     - Related magic values not grouped (e.g., status codes 1,2,3,4 defined in different files)
     - Constants defined but the raw value is still used elsewhere (partial migration)
     - Evidence: the value, all locations where it appears, suggested centralization

   How to scan:
     - Grep for numeric literals in conditionals: `if.*===?\s*\d+[^.)]` (exclude 0, 1, common values)
     - Grep for string literals in comparisons: `===?\s*["'][a-z]`
     - Grep for setTimeout/setInterval with raw numbers
     - Check if a constants/enums file exists — if yes, check for values that should be there but aren't

4. PRIMITIVE OBSESSION (beyond magic values)
   - Functions taking >5 parameters — should be an options object/struct
   - Passing raw strings where a type/enum would be safer (e.g., role as string vs Role enum)
   - Parallel arrays instead of array of objects
   - Using string IDs without a branded/newtype wrapper (userId vs orderId both plain strings)
   - Evidence: function signature, examples of unsafe primitive usage
   - How to detect: Use Grep to find function declarations with many commas in parameter lists (e.g., `function\s+\w+\(.*,.*,.*,.*,.*,`). Read files to inspect parameter types.

5. SHOTGUN SURGERY INDICATORS
   - A single logical change requires touching >5 files
   - If GitNexus available: check impact analysis for high-fan-out symbols
   - If GitNexus unavailable: use Grep to find a symbol's usages across files; if >5 files reference it, flag as high fan-out
   - Evidence: symbol name, list of files that would need changes

6. FEATURE ENVY
   - Functions that use more data from another module than their own
   - Excessive chaining of object.property.property.method()
   - Evidence: function name, what external data it accesses
   - How to detect: Use Grep to find long property chains (e.g., `\w+\.\w+\.\w+\.\w+`). Read functions and count imports from other modules vs own module.

7. INAPPROPRIATE COUPLING
   - Concrete dependencies where interfaces/abstractions would be better
   - Hard-coded configuration values
   - Tight coupling between modules that should be independent
   - Evidence: import statements, hard-coded values with file:line
   - How to detect: Use Grep to find import statements, then analyze which modules import from which. Look for cross-layer imports (e.g., UI importing DB modules).

8. ERROR HANDLING SMELLS
   - Empty catch blocks (swallowing errors silently)
   - Catching generic exceptions (catch(e) / except Exception)
   - Missing error handling on I/O operations, network calls
   - Inconsistent error return patterns (sometimes throw, sometimes return null)
   - Evidence: file:line, the problematic pattern
   - How to detect: Use Grep for `catch\s*\(` then Read to check if the catch block is empty. Grep for `catch\s*\(\s*\w+\s*\)\s*\{\s*\}` to find empty catches directly.

9. ASYNC ANTI-PATTERNS
   - Await in loops (should be Promise.all / join)
   - Missing error handling on promises
   - Callback hell (>3 nested callbacks)
   - Mixed async patterns (callbacks + promises + async/await)
   - Evidence: file:line, the pattern found
   - How to detect: Use Grep for `for.*await\s` or `while.*await\s` to find await-in-loop. Grep for `.then(` without `.catch(` nearby. Grep for deeply indented callback patterns.
```

---

### Dimension 3: Security

Goal: Identify security vulnerabilities before they reach production.

```
Scan for:

1. INJECTION RISKS
   - String concatenation in SQL queries (SQL injection)
   - Unsanitized user input in shell commands (command injection)
   - Unescaped output in HTML templates (XSS)
   - Template literals with user data in eval/Function constructor
   - Evidence: file:line, the vulnerable pattern
   - How to detect: Grep for SQL string concatenation (`query\s*\(.*\+|query\s*\(.*\$\{`), `exec(`/`execSync(` with variables, `eval(`, `innerHTML\s*=`, `dangerouslySetInnerHTML`.

2. SECRETS & CREDENTIALS
   - Hard-coded API keys, tokens, passwords in source code
   - Secrets in config files that aren't in .gitignore
   - .env files committed to git
   - Evidence: file:line (redact actual values!)
   - How to detect: Grep for patterns like `(api[_-]?key|secret|password|token|credentials)\s*[:=]`, `sk-[a-zA-Z0-9]`, `AKIA[A-Z0-9]`. Use Glob to find `.env*` files and check if they are in .gitignore.

3. AUTHENTICATION & AUTHORIZATION
   - Missing auth checks on protected routes/endpoints
   - Insecure token storage (localStorage for sensitive tokens)
   - Missing CSRF protection
   - Weak password/token validation
   - Evidence: route/endpoint, what protection is missing

4. DATA EXPOSURE
   - Logging sensitive data (passwords, tokens, PII)
   - Error messages exposing internal details to users
   - Debug endpoints accessible in production
   - Evidence: file:line, what data is exposed

5. DEPENDENCY VULNERABILITIES
   - Run `npm audit` / `cargo audit` / `pip audit` if available
   - Check for known CVEs in dependencies
   - Evidence: package name, version, CVE ID, severity

6. INSECURE DEFAULTS
   - CORS set to * in production
   - Debug mode enabled by default
   - Missing security headers (CSP, HSTS, X-Frame-Options)
   - Permissive file permissions
   - Evidence: config file:line, the insecure default
```

---

### Dimension 4: Performance

Goal: Find code patterns that cause slowness, memory leaks, or scalability problems.

```
Scan for:

1. N+1 QUERIES / WATERFALLS
   - Database calls inside loops
   - Sequential API calls that could be parallel
   - Evidence: file:line, the loop + call pattern
   - How to detect: Grep for DB query calls (`\.query\(|\.find\(|\.exec\(`), then Read surrounding code to check if they are inside for/while/forEach loops.

2. MEMORY LEAKS
   - Event listeners not cleaned up (addEventListener without removeEventListener)
   - Growing collections without bounds (arrays/maps that only grow)
   - Closures capturing large scopes unnecessarily
   - Missing cleanup in React useEffect
   - Evidence: file:line, what's leaking and why
   - How to detect: Grep for `addEventListener` and check if corresponding `removeEventListener` exists in the same file. Grep for `useEffect` and check if a cleanup function is returned.

3. EXPENSIVE OPERATIONS IN HOT PATHS
   - Regex compilation inside loops (should be compiled once)
   - JSON.parse/stringify in frequently-called functions
   - Synchronous I/O in async contexts (readFileSync in server handlers)
   - Evidence: file:line, the expensive operation, how often it's called
   - How to detect: Grep for `new RegExp` or `JSON.parse|JSON.stringify` inside function bodies, then Read to check if they are in loops or hot paths. Grep for `readFileSync|writeFileSync` in server/handler files.

4. MISSING CACHING / MEMOIZATION
   - Repeated expensive computations with same inputs
   - API calls for data that rarely changes
   - Evidence: function name, file:line, why caching would help

5. BUNDLE / BUILD ISSUES (for frontend)
   - Large dependencies imported for small utility (moment.js for date formatting)
   - Missing tree-shaking (importing entire library vs specific exports)
   - Missing code splitting / lazy loading for routes
   - Evidence: import statement, file:line, bundle size impact

6. CONCURRENCY ISSUES
   - Race conditions (shared mutable state without synchronization)
   - Missing debounce/throttle on frequent events
   - Unbounded parallelism (spawning unlimited concurrent tasks)
   - Evidence: file:line, the race condition or unbounded pattern
```

---

### Dimension 5: Dependency Health

Goal: Assess the health and risk of third-party dependencies.

```
Scan for:

1. OUTDATED DEPENDENCIES
   - Run `npm outdated` / check Cargo.toml vs crates.io / pip list --outdated
   - Categorize: patch behind, minor behind, major behind
   - Evidence: package, current version, latest version, how far behind

2. UNUSED DEPENDENCIES
   - Packages in package.json/Cargo.toml not imported anywhere in code
   - devDependencies used in production code
   - Evidence: package name, declared in X, zero imports found
   - How to detect: Read package.json/Cargo.toml to list dependencies, then Grep for each package name in source files. Zero matches = unused.

3. RISKY DEPENDENCIES
   - Packages with <100 weekly downloads (low community)
   - Packages with no updates in >2 years (abandoned)
   - Packages with known maintainer issues
   - Single-maintainer packages for critical functionality
   - Evidence: package name, download stats, last update date

4. DEPENDENCY BLOAT
   - Multiple packages doing the same thing (lodash + underscore, moment + dayjs)
   - Large dependencies where smaller alternatives exist
   - Evidence: overlapping packages, size comparison

5. VERSION PINNING
   - Missing lockfile (package-lock.json, Cargo.lock)
   - Overly permissive version ranges (^, ~, *)
   - Evidence: package.json entries with loose ranges
```

---

### Dimension 6: Test Quality

Goal: Evaluate test coverage, quality, and gaps.

```
Scan for:

1. COVERAGE GAPS
   - Critical paths without tests (auth, payment, data validation)
   - Public API functions without corresponding test files
   - If test files exist: check if they actually test meaningful scenarios
   - Evidence: untested function/module, its importance, risk
   - How to detect: Use Glob to find source files (`src/**/*.{ts,js,rs,py}`), then Glob for matching test files (`**/*.test.*|**/*.spec.*|**/test_*`). Source files without corresponding test files = coverage gap.

2. TEST SMELLS
   - Tests without assertions (test runs but verifies nothing)
   - Tests that always pass (testing implementation, not behavior)
   - Excessive mocking (tests that don't verify real behavior)
   - Flaky test indicators (timeouts, sleep, race conditions in tests)
   - Tests >100 lines (too complex, testing too many things)
   - Evidence: test file:line, the smell
   - How to detect: Grep test files for `it\(|test\(` blocks, then Read to check if they contain `expect|assert|should`. Grep for `sleep|setTimeout|\.skip` in test files.

3. MISSING TEST TYPES
   - No integration tests (only unit tests)
   - No E2E tests for critical user flows
   - No error case testing (only happy path)
   - No edge case testing (empty input, null, boundary values)
   - Evidence: what type is missing, what it should cover

4. TEST INFRASTRUCTURE
   - Missing CI integration (tests don't run on push/PR)
   - Slow test suite (>5 minutes)
   - No test data management (fixtures, factories)
   - Evidence: CI config gaps, test run timing
```

---

### Dimension 7: Documentation

Goal: Identify documentation gaps that slow down onboarding and maintenance.

```
Scan for:

1. MISSING CRITICAL DOCS
   - No README or README is stale/generic
   - No setup/installation guide
   - No architecture overview for complex projects
   - Missing API documentation for public interfaces
   - Evidence: what's missing, why it matters
   - How to detect: Use Glob to check for `README*`, `CONTRIBUTING*`, `docs/**`. Read README and check if it has setup instructions, architecture section, and API docs links.

2. STALE DOCUMENTATION
   - README references features/files that no longer exist
   - Comments describing behavior that doesn't match the code
   - Outdated examples that would fail if run
   - Evidence: doc file:line, what's stale, what it should say
   - How to detect: Read README/docs, extract referenced file paths and command examples, then use Glob/Bash to verify they still exist or work.

3. UNDOCUMENTED DECISIONS
   - Complex logic without explaining why (not what)
   - Workarounds without linking to the issue they work around
   - Configuration with non-obvious values and no explanation
   - Evidence: file:line, the unclear decision

4. MISSING INLINE DOCS
   - Public APIs without parameter/return documentation
   - Complex algorithms without explanation
   - Non-obvious side effects not documented
   - Evidence: function name, file:line, what needs documenting
```

---

### Dimension 8: Developer Experience (DX)

Goal: Find friction points that slow down development.

```
Scan for:

1. BUILD & SETUP ISSUES
   - Complex multi-step setup (>5 commands to get running)
   - Missing or broken scripts in package.json
   - Undocumented environment variables
   - Evidence: what's missing or broken
   - How to detect: Read package.json scripts section. Grep for `process.env.` to find referenced env vars, then check if they are documented in README or `.env.example`.

2. CODE ORGANIZATION DX
   - Deep directory nesting (>5 levels)
   - Unclear where to add new code (no conventions documented)
   - Mixed concerns in directories (tests mixed with source)
   - Evidence: directory structure issues
   - How to detect: Use Glob with `**/*` and inspect path depth. Use Bash `find . -type d -mindepth 5` to find deeply nested directories.

3. TOOLING GAPS
   - No linter configured
   - No formatter configured (or not enforced)
   - No pre-commit hooks
   - No type checking (for JS projects: missing TypeScript or JSDoc)
   - Evidence: what tool is missing, how it would help
   - How to detect: Use Glob to check for `.eslintrc*`, `.prettierrc*`, `tsconfig.json`, `.pre-commit-config.yaml`, `.husky/`. Read package.json for lint/format scripts.

4. WORKFLOW FRICTION
   - No dev server with hot reload
   - Slow feedback loop (build takes >30s for small changes)
   - Missing convenience scripts (no `npm run dev`, etc.)
   - Evidence: what's slow or missing
```

---

### Dimension 9: Simplify Scan (Codebase-wide)

Goal: Apply the 4 criteria from Claude's code-simplifier across the **entire codebase** (not just recent changes). This surfaces code that "works fine" but is unnecessarily complex, inconsistent, or hard to maintain — the kind of issues that accumulate silently and make refactoring painful.

Unlike the simplify skill which operates on recent diffs, this dimension audits the full codebase to find systemic simplification opportunities.

```
Scan using 4 criteria:

1. PRESERVE FUNCTIONALITY (Identify risky patterns)
   Goal: find code where the current structure makes it easy to accidentally break behavior during refactoring.

   - Functions with hidden side effects not obvious from name/signature
     - A function called `getUser()` that also updates a cache or emits an event
     - A setter that triggers validation, network calls, or state changes beyond the set
     - Evidence: function name, file:line, the hidden side effect, why it's dangerous

   - Implicit ordering dependencies
     - Code that only works because functions are called in a specific undocumented order
     - Module initialization that depends on another module being loaded first
     - Evidence: the dependent code, the implicit assumption, what breaks if order changes

   - Mutation through references
     - Functions that mutate input parameters instead of returning new values
     - Shared mutable state passed between modules without clear ownership
     - Evidence: function that mutates its argument, file:line, what gets mutated

   - Fragile equality/comparison
     - Comparing objects by reference where value comparison is intended
     - String comparison for things that should be enums (e.g., `if (status === "active")`)
     - Evidence: the fragile comparison, file:line, safer alternative

2. PROJECT STANDARDS COMPLIANCE (Consistency audit)
   Goal: scan the whole codebase for deviations from the project's own established patterns.

   - Import style
     - Mixed import styles: `require()` vs `import` in same project
     - Inconsistent import ordering (some files group by type, others don't)
     - Relative imports where aliases exist, or vice versa
     - Evidence: file A (style X) vs file B (style Y), count of each style

   - Function declaration style
     - Mixed: arrow functions, function declarations, function expressions for the same use case
     - Inconsistent parameter handling: some destructure, some don't
     - Evidence: examples of each style with file:line, which is dominant

   - Error handling pattern
     - Project uses try/catch in some places, .catch() in others, Result types in others
     - Some functions throw on error, others return null/undefined, others return error objects
     - Evidence: comparison table of patterns across modules

   - Naming conventions
     - Variable naming: camelCase vs snake_case in same language
     - Boolean naming: some use `isActive`, others use `active`, others use `hasActive`
     - Event/callback naming: `onClick` vs `handleClick` vs `onClickHandler`
     - Evidence: variants found with file:line for each

   - File & directory conventions
     - Count files following each pattern, identify the dominant one, flag outliers
     - Evidence: pattern distribution, list of outlier files

   How to scan:
     - Sample 10-15 files across different modules
     - For each file, note: import style, function style, error handling, naming
     - Build a compliance matrix — the majority pattern = "project standard"
     - Flag files that deviate from the majority pattern

3. CLARITY OPPORTUNITIES (Simplification candidates)
   Goal: find code that works but is harder to read/maintain than necessary.

   - UNNECESSARY COMPLEXITY
     - Nested ternaries: `a ? b ? c : d : e` — rewrite as if/else or switch
     - Deeply nested conditionals (>3 levels) — extract into named functions
     - Complex boolean expressions: `if (!(!a || (b && !c)))` — simplify or name the condition
     - Dense one-liners: `return arr.filter(x => x.active).map(x => x.id).reduce((a,b) => a+b, 0)` — break into steps with meaningful variable names
     - Evidence: file:line, the complex code, suggested simplification

   - REDUNDANT CODE
     - Assignments that are immediately overwritten
     - Conditions that are always true/false (detectable from surrounding context)
     - Variables assigned but only used once in the next line — inline them
     - Wrapper functions that add no logic (pass-through to another function)
     - Type assertions/casts that are unnecessary (value is already that type)
     - Evidence: file:line, the redundant code, proof it's redundant

   - UNCLEAR NAMING
     - Single-letter variables outside of loop indices and lambdas (`const x = getUser()`)
     - Abbreviations that aren't universally understood (`const usr = ...`, `const mgr = ...`)
     - Generic names: `data`, `result`, `temp`, `info`, `item`, `obj`, `val` — name the actual thing
     - Boolean variables that don't read as true/false questions (`const valid` vs `const isValid`)
     - Functions with vague names: `process()`, `handle()`, `run()`, `execute()`, `doStuff()`
     - Evidence: file:line, the unclear name, suggested better name

   - CONSOLIDATION OPPORTUNITIES
     - Related logic scattered across a file — could be grouped into a section or extracted
     - Switch/if-else chains that could be a lookup table/map
     - Repeated parameter lists across functions — should be an object/struct
     - Evidence: the scattered pieces, suggested consolidation approach

4. BALANCE CHECK (Over-simplification risks)
   Goal: identify places where previous "simplification" or "clever code" went too far.

   - OVERLY CLEVER CODE
     - Bitwise operations for non-performance-critical logic (`x | 0` to floor, `!!value` to bool)
     - Regex used where string methods would be clearer
     - Abuse of short-circuit evaluation for side effects: `condition && doSomething()`
     - Comma operator, void operator, or other obscure operators in application code
     - Evidence: file:line, the clever code, what it does in plain language

   - OVER-COMPRESSED FUNCTIONS
     - Functions that handle >3 distinct responsibilities to avoid "too many functions"
     - God functions that are "simple" in terms of abstraction but do too much
     - Evidence: function name, file:line, list of responsibilities it handles

   - MISSING HELPFUL ABSTRACTIONS
     - The same 3-5 line pattern repeated 4+ times — should be a named function
     - Complex operations without a descriptive wrapper (e.g., raw regex without explaining what it matches)
     - Domain concepts not represented in code (e.g., "user role check" is inline everywhere instead of `canAccess(user, resource)`)
     - Evidence: the repeated pattern, all locations, suggested abstraction

   - PREMATURE INLINING
     - Constants inlined after someone "simplified" them — now the meaning is lost
     - Helper functions inlined at call sites — making the caller harder to read
     - Evidence: file:line, what was inlined, why it should be a named thing
```

### Summary output for Dimension 9

After scanning, produce a Simplify Score:

```
SIMPLIFY SCAN RESULTS
═════════════════════
Files sampled: N / N total
Standards compliance: N% (files following dominant patterns)
Clarity score: HIGH / MEDIUM / LOW
Balance: OK / OVER-SIMPLIFIED / OVER-COMPLEX

Top simplification opportunities:
1. [file:line] — <what to simplify and how>
2. [file:line] — <what to simplify and how>
...
```

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

| Rule | Detail |
|------|--------|
| **Evidence required** | Every finding must include file path + line number + code snippet — no vague claims |
| **No false alarms** | If uncertain, re-read the code to confirm before reporting. Mark uncertain findings with ⚠️ |
| **Severity consistency** | Use the severity rubric in Step 5 — don't inflate or deflate |
| **Actionable fixes** | Every finding must include a specific suggested fix, not just "refactor this" |
| **Effort estimation** | S=<1hr, M=1-4hr, L=4-8hr, XL=>8hr — estimate for a developer familiar with the codebase |
| **Parallel scanning** | Run dimension agents in parallel — do not scan sequentially |
| **GitNexus first** | Use GitNexus tools when available for more accurate dependency/impact analysis |
| **Redact secrets** | If secrets are found, report their existence but NEVER include actual values |
| **AskUserQuestion for all interactions** | Every user-facing question uses AskUserQuestion |
| **Respect scope** | Only scan dimensions the user selected — don't add unrequested dimensions |
| **Cross-reference** | Step 5 must run after all agents complete — don't skip deduplication |
| **Refactoring roadmap** | Always include a phased roadmap — the whole point is guiding refactoring |
