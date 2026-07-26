---
name: hoangsa:audit
description: Audit the codebase — scan for code smells, architecture issues, security risks, performance problems, and tech debt → AUDIT-REPORT.md. Use when the user wants a health check, code review, wants to find problems before refactoring, says "audit", "review codebase", "find issues", "tech debt", or needs a comprehensive analysis of what needs fixing.
allowed-tools:
  - Read
  - Bash
  - Write
  - Glob
  - Grep
  - Task
  - AskUserQuestion
  - WebSearch
  - WebFetch
---

<objective>
Perform a comprehensive audit of the current codebase, producing a detailed AUDIT-REPORT.md that covers:

- Architecture & structure issues
- Code smells & anti-patterns
- Security vulnerabilities
- Performance bottlenecks
- Dependency health (outdated, vulnerable, unused)
- Test coverage gaps
- Documentation gaps
- Prioritized action items for refactoring

The report is designed for teams — detailed enough that multiple developers can independently pick up refactoring tasks from it.
</objective>

<execution_context>
Load the workflow:

```bash
hoangsa-cli workflow show audit
```

Its stdout IS the workflow — follow it. The command searches the
project-local install, `$CLAUDE_CONFIG_DIR`, then the default profile,
and lists every path it tried if nothing matched.
</execution_context>

<process>
Follow the audit workflow loaded above.
</process>

