---
name: ui-details
frameworks: ["react", "react-native", "expo", "nextjs", "vue", "svelte", "angular", "flutter", "swift", "swiftui", "uikit"]
test_frameworks: []
priority: 55
inject_position: after_base
allowed_tools: []
pre_invoke_gate: null
exclude_task_types: ["research", "analysis"]
exclude_worker_roles: ["readonly", "reviewer"]
---

# UI Detail Discipline

Small UI details are where "works on my machine" ships broken. Every UI surface you create or modify MUST handle all the states below — and you MUST see each one rendered before you commit.

## State checklist (all mandatory)

- **Empty** — no data yet: a meaningful empty state, never a blank area or a rendered `undefined`.
- **Loading** — visible indicator; layout must not jump when content arrives.
- **Error** — human-readable message plus a retry path; never a silent blank or a raw error object.
- **Disabled / busy** — interactive elements disable during in-flight actions; no double-submit.
- **Overflow** — long text truncates or wraps deliberately; test with a string 3× longer than the design shows.
- **Responsive** — verify at narrow AND wide breakpoints (mobile: small device and large device).
- **Focus / keyboard** — interactive elements reachable and visibly focused via keyboard.

If the task's `edge_cases` list adds UI-specific cases, they extend this checklist — they never replace it.

## Fidelity

- Match the spec/design exactly for spacing, alignment, and typography — "close enough" is a bug.
- Reuse existing design tokens and shared components; do not hardcode one-off colors, sizes, or shadows.

## Verification (non-negotiable)

- Render every state above in the REAL running app before committing — compiling plus green unit tests is NOT verification for UI.
- Follow the fe-testing skill flow 5 (run-and-observe): boot the app, drive each state, screenshot each one into the evidence dir given in your task envelope, and list the paths in your report.
- If a state cannot be reached or verified, report it as a blocker — do not commit unverified UI.
