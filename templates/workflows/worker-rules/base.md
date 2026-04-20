# Worker Rules

Rules that every HOANGSA worker subagent MUST follow when implementing a task.
These rules are non-negotiable unless explicitly overridden by project config.

> **Customization:** Copy this file to `.hoangsa/worker-rules.md` in your project
> and modify as needed. The project-level file takes priority over this default.

---

## 1. Scope Control

- **Only modify files listed in `task.files`.** If you discover a file that also needs changes, report it — do NOT modify it yourself.
- **No refactoring outside scope.** Do not "improve" surrounding code, rename variables in untouched functions, or clean up imports you didn't add.
- **No new dependencies** unless the DESIGN-SPEC explicitly requires them. If you believe a dependency is needed, report it as a blocker.
- **Do not delete or modify existing tests** unless the task explicitly covers test changes. Adding new tests is fine; breaking existing ones is not.
- **No feature creep.** Implement exactly what the task describes. No "while I'm here" additions.

---

## 2. Code Quality

- **Match the project's existing style.** Indentation, naming conventions (camelCase vs snake_case), quote style, bracket placement — follow what's already there.
- **Do not add comments, docstrings, or type annotations** to code you did not write or change. Only add comments where the logic is not self-evident in code you authored.
- **No over-engineering.** No premature abstractions, no helper utilities for one-time operations, no design-for-the-future patterns. Three similar lines > a premature abstraction.
- **No unnecessary error handling.** Do not add validation, fallbacks, or try/catch for scenarios that cannot happen according to the spec. Trust internal code and framework guarantees. Only validate at system boundaries (user input, external APIs).
- **No backward-compatibility shims.** No renaming unused `_vars`, no re-exporting removed types, no `// removed` comments. If something is removed, remove it completely.

---

## 3. Security

- **Never hardcode secrets, API keys, tokens, or credentials.** Use environment variables or config files.
- **Do not introduce OWASP Top 10 vulnerabilities:** no SQL injection, XSS, command injection, path traversal, or insecure deserialization.
- **Sanitize at system boundaries.** Validate and sanitize user input, external API responses, and file paths at entry points.
- **If you notice existing insecure code** in files you're modifying, fix it only if it's within your task scope. Otherwise, report it.

---

## 4. Git Discipline

- **Atomic commit after acceptance passes.** One commit per task, containing only files relevant to that task.
- **Commit message format:** `<type>(<scope>): <task.name>` — `<scope>` is the primary module/package affected (e.g., `budget`, `auth`, `cli`), derived from `task.files` paths. Do NOT use `session_id` or branch name as scope.
- **Do not commit:** `.env` files, credentials, large binaries, IDE config, OS-generated files, or files not in `task.files`.
- **Do not amend, rebase, or force-push** existing commits.

---

## 5. Acceptance

- **Read all `context_pointers` before writing any code.** Understand the existing code first.
- **Run the acceptance command** before committing. Do not commit if acceptance fails.
- **Max 3 retry attempts** if acceptance fails:
  1. Attempt 1 — fix based on error output
  2. Attempt 2 — re-read context, look for missed patterns
  3. Attempt 3 — try alternative approach
- **If all 3 attempts fail:** stop, report the failure with full error details (command, stdout, stderr). Do NOT keep retrying.

---

## 6. Context Hygiene

- **Read only what you need.** Start with `context_pointers`, then `task.files`. Do not explore the entire codebase.
- **Do not read files unrelated to the task.** Every file read consumes context window — keep it focused.
- **If you need information not in your context:** report it as a blocker rather than guessing.
- **Large tool results — evict, don't hoard.** If a tool result exceeds ~100 lines (Grep output, file reads, test output), extract only the relevant lines you need and discard the rest. Do NOT keep massive tool results in your working memory — summarize the key findings, note the file path and line range for re-reading later if needed. Think of your context window as RAM: large tool results are the #1 cause of context rot.
- **Re-read over recall.** If you need to reference a large file section again later, use `Read` with a targeted `offset`/`limit` rather than trying to hold it all in context from the first read.

---

## 7. Communication

- **Report, don't guess.** If something is ambiguous, unclear, or missing from the spec — report it as a blocker. Do not make assumptions about intended behavior.
- **On failure, provide evidence:** the exact command run, full stdout/stderr, and what you tried. Do not summarize or truncate error output.
- **Do not apologize or explain your reasoning at length.** State what you did, what passed, what failed. Be terse.
- **Respect user's language preference.** If the orchestrator specifies a `lang` preference (e.g., `vi` for Vietnamese, `en` for English), all status messages, error reports, and commit messages descriptions must use that language. Commit message prefixes (`feat`, `fix`, `refactor`) stay in English per conventional-commits spec.

