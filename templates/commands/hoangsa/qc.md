---
name: hoangsa:qc
description: QC — take a spec, design test cases, execute them, and report verdicts where every pass AND every fail is backed by captured evidence. Use when the user wants to QC/QA a feature against a spec, write and run test cases from requirements, or verify a delivery with proof. Does not fix bugs — hands reproducible bug reports to /hoangsa:fix.
allowed-tools:
  - Read
  - Bash
  - Task
  - AskUserQuestion
---

<objective>
From spec to proven verdicts: derive test cases from the given spec (file, session TEST-SPEC, or external task), get the case list approved, execute every case against the real system, and capture evidence artifacts for each verdict — pass or fail. Failures become reproducible bug reports routed to /hoangsa:fix.

Every verdict must link captured evidence on disk; a pass without evidence is re-run, never reported.
</objective>

<execution_context>
Resolve HOANGSA install path — check local first, then global:
1. `./.claude/hoangsa/workflows/qc.md`
2. `~/.claude/hoangsa/workflows/qc.md`
Read the first path that exists.
</execution_context>

<process>
Follow the qc workflow loaded above.
</process>
