---
name: memory
frameworks: ["*"]
test_frameworks: []
priority: 70
inject_position: after_base
allowed_tools: []
pre_invoke_gate: "hoangsa-cli pref get . memory_strict | grep -q true"
---

# hoangsa-memory — Code Intelligence

If the orchestrator tells you hoangsa-memory is available (`MEMORY_AVAILABLE`), use it to understand code before modifying it. hoangsa-memory provides a pre-indexed knowledge graph of the codebase — it's faster and more accurate than grepping.

### Before editing a symbol (function, class, method):

```
memory_impact({target: "symbolName", direction: "upstream"})
```

Check the blast radius. If risk is HIGH or CRITICAL, report it to the orchestrator before proceeding — do not silently push through.

### When you need to understand a symbol's callers/callees:

```
memory_symbol_context({name: "symbolName"})
```

This gives you the full picture — who calls it, what it calls, which execution flows it participates in. Use this instead of grepping for function names.

### When tracing a bug or finding related code:

```
memory_recall({query: "description of what you're looking for"})
```

Returns execution flows ranked by relevance. Better than `Grep` for understanding how pieces connect.

### Rules:

- **Impact before edit.** Run `memory_impact` on every symbol you're about to modify. This is not optional — it prevents breaking callers you didn't know about.
- **HIGH/CRITICAL = report.** If impact analysis returns HIGH or CRITICAL risk, report it to the orchestrator with the affected symbols. Do not proceed without acknowledgment.
- **Fallback gracefully.** If a hoangsa-memory tool call fails (timeout, error), fall back to Grep/Glob. Do not block on it.
- **hoangsa-memory unavailable = skip.** If the orchestrator does not pass `MEMORY_AVAILABLE`, use Grep/Glob as usual. Do not attempt hoangsa-memory calls.

---

## Remember What You Learn

When you discover something worth persisting (a non-obvious fact about the codebase, a gotcha, a lesson learned from a mistake):

1. **Facts:** `memory_remember_fact({text: "<concise fact>", tags: ["<tag>"]})` — durable truths about the codebase.
2. **Lessons:** `memory_remember_lesson({trigger: "<when this applies>", advice: "<what to do>"})` — actionable advice for future edits.

Before any non-trivial edit, recall first: `memory_recall({query: "<what you're about to do>"})`.

---

## Archive Awareness

Before starting implementation, search past conversations for relevant context:

```
memory_archive_search({query: "<module name> <task domain>"})
```

Look for:
- Prior bug fixes in the same area (avoid repeating mistakes)
- Past design decisions that constrain your options
- Solutions that were tried and abandoned (with reasons)

If archive returns relevant hits, factor them into your approach. If no relevant hits, proceed normally — this is a quick check, not a blocker.

---

## Change Scope Verification

After completing your task and before committing:

```
memory_detect_changes({diff: "<git diff of your changes>"})
```

Verify that:
- Only symbols in your `task.files` are affected
- No unexpected blast radius (transitive changes you didn't intend)
- If unexpected symbols appear, report them to the orchestrator — do not commit

This is a quick sanity check, not a blocker if the changes are intentional.

---

## Memory Detail Lookup

When `memory_recall` returns a fact or lesson with a partial match (truncated content or unclear context):

```
memory_detail({kind: "fact|lesson", index: <N>})
```

Use this to read the full content before acting on it. Do not make assumptions based on truncated recall results.
