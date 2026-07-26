---
description: Test — run acceptance tests and report results per task. Use when the user wants to verify tasks, check test results, or validate that implementation matches the spec. Does not fix failures — delegates to /hoangsa:fix.
allowed-tools:
  - Read
  - Bash
  - Write
  - Edit
  - Task
  - AskUserQuestion
---

<objective>
Run acceptance tests for each task in the current session, report pass/fail results clearly, and update task statuses in plan.json.

Does NOT fix failures — reports them with full error output and suggests /hoangsa:fix for remediation. Optionally chains to /hoangsa:plate for passing work.
</objective>

<execution_context>
Load the workflow:

```bash
hoangsa-cli workflow show taste
```

Its stdout IS the workflow — follow it. The command searches the
project-local install, `$CLAUDE_CONFIG_DIR`, then the default profile,
and lists every path it tried if nothing matched.
</execution_context>

<process>
Follow the taste workflow loaded above.
</process>

