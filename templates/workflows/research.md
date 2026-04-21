# Research Workflow

Conduct deep research on a topic — codebase structure, patterns, or external knowledge — and produce a structured RESEARCH.md report.

**Principles:** Ask before assuming. Use hoangsa-memory when available, fall back gracefully. Support both auto and confirm modes. Use AskUserQuestion for all user interactions.

---

---

## Step 1: Session detection

Detect whether an active session exists:

```bash
SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session latest)
```

- If `SESSION` is non-empty → extract `SESSION_DIR` from the result and use it as the output directory.
- If `SESSION` is empty (no active session) → auto-create a standalone research session. Derive slug from the research topic:

```bash
# SLUG auto-derived from topic (e.g. "auth-patterns", "logging-architecture")
SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session init docs "$SLUG")
# Extract SESSION_DIR from the result
```

This makes the workflow flexible — it works both inside a full HOANGSA session and as a standalone research tool.

---

## Step 2: Gather input

### 2a. Load saved preferences

```bash
PREFS=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get .)
```

Extract `research_scope` and `research_mode` from preferences.

### 2b. Research topic (always ask — this changes every time)

Use AskUserQuestion:
  question: "Bạn muốn research topic gì?"
  header: "Research topic"
  options:
    - label: "Tôi sẽ mô tả", description: "Gõ topic vào ô 'Other' bên dưới"
    - label: "Xem ví dụ trước", description: "Xem ví dụ topic trước khi tự viết"
  multiSelect: false

  If user chọn "Xem ví dụ trước" → show examples such as:
    - "Authentication flow in this codebase"
    - "Best practices for rate limiting in Node.js"
    - "How does the payment module work?"
  Then ask again with the same question.
  If user chọn "Tôi sẽ mô tả" or Other → use their input as `RESEARCH_TOPIC`.

### 2c. Research scope (use saved or ask once)

If `research_scope` is `null` (first time):

  Use AskUserQuestion:
    question: "Phạm vi research mặc định?"
    header: "Scope"
    options:
      - label: "Codebase only", description: "Chỉ research trong codebase hiện tại"
      - label: "External only", description: "Chỉ tìm kiếm tài liệu và best practices bên ngoài"
      - label: "Both (Recommended)", description: "Research cả codebase lẫn tài liệu ngoài — kết quả toàn diện nhất"
    multiSelect: false

  Save immediately:

  ```bash
  "$HOANGSA_ROOT/bin/hoangsa-cli" pref set . research_scope "both"
  ```

If already saved → use it. Show briefly:

```
Research scope: Both (codebase + external)  [saved]
```

Store as `RESEARCH_SCOPE`.

### 2d. Research mode (use saved or ask once)

If `research_mode` is `null` (first time):

  Use AskUserQuestion:
    question: "Chế độ chạy research mặc định?"
    header: "Mode"
    options:
      - label: "Auto", description: "Tự động chạy hết — không hỏi thêm, nhanh nhất"
      - label: "Confirm", description: "Dừng ở các bước quan trọng để user review"
    multiSelect: false

  Save immediately:

  ```bash
  "$HOANGSA_ROOT/bin/hoangsa-cli" pref set . research_mode "auto"
  ```

If already saved → use it.

Store as `RESEARCH_MODE`.

---

## Step 3: Codebase research (parallel agents)

Skip this step if `RESEARCH_SCOPE` is "external".

First, check hoangsa-memory availability:

```bash
command -v hoangsa-memory &>/dev/null && echo "MEMORY_AVAILABLE" || echo "MEMORY_NOT_INSTALLED"
```

Store result as `MEMORY_STATUS`.

### Model selection

```bash
MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model researcher)
```

Use the resolved model for spawning research agents.

**MEMORY_ACTOR:** Set `MEMORY_ACTOR=hoangsa/research-<agent>` for research agents. This selects the `hoangsa/research-*` gate policy (gate disabled) so read-only research agents are not blocked by the gate.

Launch 3 parallel research agents:

### Agent 1 — Structure

Goal: Understand how the codebase is organized relative to the research topic.

```
If MEMORY_STATUS == "MEMORY_AVAILABLE":
  - Run memory_recall({query: "<RESEARCH_TOPIC>"}) to find relevant execution flows
  - Run memory_symbol_context({name: "<key symbol found>"}) for top symbols
Else (MEMORY_NOT_INSTALLED fallback):
  - Use Glob to find project entry points (index.*, main.*, app.*, server.*)
  - Use Grep to find files referencing the research topic keywords
  - Map module/package layout from directory structure

Output:
  - Project entry points
  - Module layout relevant to topic
  - List of files related to RESEARCH_TOPIC with short descriptions
```

### Agent 2 — Patterns

Goal: Identify coding patterns and conventions used in the codebase.

```
If MEMORY_STATUS == "MEMORY_AVAILABLE":
  - Use memory_symbol_context({name: "<relevant function or class>"}) for key symbols
  - Trace callers and callees to understand patterns
Else (MEMORY_NOT_INSTALLED fallback):
  - Use Grep to find error handling patterns (try/catch, Result, Option, etc.)
  - Use Grep to find async patterns (async/await, Promise, Future, goroutine)
  - Sample 2–3 similar implementations for naming conventions

Output:
  - Error handling pattern
  - Async pattern (or N/A)
  - Naming conventions
  - Logging/tracing patterns
```

### Agent 3 — Dependencies

Goal: Identify relevant dependencies and their roles.

```
- Read package manager files: package.json, Cargo.toml, requirements.txt, go.mod, pyproject.toml
- List packages relevant to RESEARCH_TOPIC and their versions
- Note known limitations or gotchas of key dependencies
- Check for feature flags or environment-based configuration

Output:
  - Relevant packages with versions
  - What each package is used for
  - Known limitations
```

### Agent 4 — Archive Mining (run in parallel with Agents 1-3)

Search past conversations for prior research and discussions on this topic:

```
memory_archive_search({query: "<RESEARCH_TOPIC>"})
memory_archive_topics()
memory_turns_search({query: "<RESEARCH_TOPIC>"})
```

Extract:
- Prior research findings on the same or related topics
- Design decisions that were discussed but may not be in MEMORY.md
- Past approaches tried and their outcomes

Include findings in RESEARCH.md under a "## Prior Conversations" section.

Collect outputs from all 4 agents before proceeding.

---

## Step 4: External research

Skip this step if `RESEARCH_SCOPE` is "codebase".

Search for external knowledge related to `RESEARCH_TOPIC`:

```
Strategy (try in order):
  1. If MCP tools are available → use them for structured documentation lookup
  2. Fallback → use WebSearch to find relevant articles, docs, RFCs
  3. Use WebFetch to retrieve and read the most relevant pages

Search queries to run (adjust to topic):
  - "<RESEARCH_TOPIC> best practices"
  - "<RESEARCH_TOPIC> documentation"
  - "<RESEARCH_TOPIC> common pitfalls"
  - "<tech stack> <RESEARCH_TOPIC> examples"

For each search result:
  - Note the source URL
  - Extract key findings relevant to RESEARCH_TOPIC
  - Note how it applies to this project
```

Collect at least 2–3 credible sources before proceeding.

---

## Step 5: User review (confirm mode only)

Skip this step if `RESEARCH_MODE` is "auto".

If `RESEARCH_MODE` is "confirm":

Use AskUserQuestion:
  question: "Kết quả research có OK không? Cần research thêm gì không?"
  header: "Review"
  options:
    - label: "OK — tiếp tục tổng hợp", description: "Kết quả đủ, tiến hành viết RESEARCH.md"
    - label: "Research thêm codebase", description: "Cần tìm hiểu thêm trong codebase — ghi rõ topic vào Other"
    - label: "Research thêm external", description: "Cần tìm thêm tài liệu ngoài — ghi rõ topic vào Other"
  multiSelect: false

If user chọn "Research thêm codebase" → re-run Step 3 with the additional topic from Other, then re-ask Step 5.
If user chọn "Research thêm external" → re-run Step 4 with the additional queries from Other, then re-ask Step 5.
If user chọn "OK" → proceed to Step 6.

Maximum 3 additional research passes. After 3 passes, synthesize all findings and proceed to save RESEARCH.md.

---

## Step 6: Synthesize RESEARCH.md

Combine all findings from Steps 3 and 4 into a single `RESEARCH.md` document.

Use this template:

```markdown
# Research: <RESEARCH_TOPIC>

## Tech Stack Detected
<Language, framework, runtime versions detected from codebase>

## Project Structure
<Module/package layout, entry points relevant to topic>

### Relevant Files
<Files related to topic, with short descriptions>

## Patterns & Conventions
- Error handling: <pattern>
- Async: <pattern or N/A>
- Naming: <convention>
- Logging: <pattern>

## Test Patterns
- Framework: <name>
- Location: <where tests live>
- Mocking: <approach>
- Integration: <structure>

## Relevant Dependencies
| Package | Version | Used for |
|---------|---------|----------|

## External Research
### <Topic 1>
- Source: <URL or reference>
- Key findings: <summary>
- Relevance: <how it applies to this project>

### <Topic 2>
- Source: <URL or reference>
- Key findings: <summary>
- Relevance: <how it applies to this project>

## Key Findings
<Most important discoveries that affect design or implementation decisions>

## Risks & Concerns
<Breaking changes, race conditions, performance cliffs, security considerations>
```

Omit sections that are not applicable (e.g., omit "External Research" if scope was "codebase only"; omit "Project Structure" if scope was "external only").

---

## Step 7: Save and report

Save RESEARCH.md to the output directory:

```bash
# If inside a session:
cp RESEARCH.md "$SESSION_DIR/RESEARCH.md"

# If standalone:
# File is already at $SESSION_DIR/RESEARCH.md
```

Report to the user:

```
✅ Research complete!
   Topic:    <RESEARCH_TOPIC>
   Scope:    <codebase / external / both>
   Mode:     <auto / confirm>
   Output:   <path to RESEARCH.md>
   Sections: <N>

Next steps:
   - /hoangsa:menu  — use this research to design a spec
   - /hoangsa:cook  — start implementing if you already have a plan
```

---

## Rules

| Rule | Detail |
|------|--------|
| **AskUserQuestion for all interactions** | Every user-facing question uses AskUserQuestion — no plain text prompts |
| **hoangsa-memory first, fallback gracefully** | Always try hoangsa-memory tools first; use Grep/Glob if index unavailable |
| **WebSearch/WebFetch as fallback** | For external research, MCP first, then WebSearch/WebFetch |
| **Non-blocking index check** | hoangsa-memory warning in Step 0 never halts the workflow |
| **Session-flexible** | Works inside a HOANGSA session or standalone — Step 1 handles both |
| **auto and confirm modes** | Confirm mode adds Step 5 review loop; auto skips it |
| **RESEARCH.md always produced** | Output file is always written, regardless of scope or mode |
| **Parallel agents for codebase** | Run Agents 1, 2, 3 in parallel — do not run sequentially |
| **Loop until satisfied (confirm mode)** | User can trigger additional research passes in Step 5 |
| **Save preferences on first ask** | Scope + mode saved to config, never asked again |
