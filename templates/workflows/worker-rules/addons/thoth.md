---
name: thoth
frameworks: ["*"]
test_frameworks: []
priority: 70
inject_position: after_base
allowed_tools: []
pre_invoke_gate: "hoangsa-cli pref get . thoth_strict | grep -q true"
---

# Thoth — Code Intelligence

If the orchestrator tells you Thoth is available (`THOTH_AVAILABLE`), use it to understand code before modifying it. Thoth provides a pre-indexed knowledge graph of the codebase — it's faster and more accurate than grepping.

### Before editing a symbol (function, class, method):

```
thoth_impact({target: "symbolName", direction: "upstream"})
```

Check the blast radius. If risk is HIGH or CRITICAL, report it to the orchestrator before proceeding — do not silently push through.

### When you need to understand a symbol's callers/callees:

```
thoth_symbol_context({name: "symbolName"})
```

This gives you the full picture — who calls it, what it calls, which execution flows it participates in. Use this instead of grepping for function names.

### When tracing a bug or finding related code:

```
thoth_recall({query: "description of what you're looking for"})
```

Returns execution flows ranked by relevance. Better than `Grep` for understanding how pieces connect.

### Rules:

- **Impact before edit.** Run `thoth_impact` on every symbol you're about to modify. This is not optional — it prevents breaking callers you didn't know about.
- **HIGH/CRITICAL = report.** If impact analysis returns HIGH or CRITICAL risk, report it to the orchestrator with the affected symbols. Do not proceed without acknowledgment.
- **Fallback gracefully.** If a Thoth tool call fails (timeout, error), fall back to Grep/Glob. Do not block on it.
- **Thoth unavailable = skip.** If the orchestrator does not pass `THOTH_AVAILABLE`, use Grep/Glob as usual. Do not attempt Thoth calls.

---

## Knowledge Graph Maintenance

When you modify a module's **public interface** (exported functions, public methods, API endpoints, type definitions consumed by other modules):

1. **Add new relationships:** `thoth_kg_add({subject: "<your module>", predicate: "exports|provides|depends_on", object: "<related module/type>", confidence: 0.9})`
2. **Invalidate changed relationships:** `thoth_kg_invalidate({subject: "<module>", predicate: "<old relationship>", object: "<old target>"})` then add the new one.
3. **Skip if:** The change is purely internal (private functions, local variables, implementation details that don't cross module boundaries).

---

## Archive Awareness

Before starting implementation, search past conversations for relevant context:

```
thoth_archive_search({query: "<module name> <task domain>"})
```

Look for:
- Prior bug fixes in the same area (avoid repeating mistakes)
- Past design decisions that constrain your options
- Solutions that were tried and abandoned (with reasons)

If archive returns relevant hits, factor them into your approach. If no relevant hits, proceed normally — this is a quick check, not a blocker.

---

## Override Protocol

If the Thoth gate **blocks** your edit (strict/require mode prevents Write/Edit without recent recall):

1. **First try:** Run `thoth_recall({query: "<what you're editing>"})` — the gate may just need a fresh recall.
2. **If still blocked:** File an override request:
   ```
   thoth_override_request({rule_id: "<blocked rule>", reason: "<why this edit is necessary>", tool_call_hash: "<from error>"})
   ```
3. **Wait for orchestrator** to surface the request to the user for approval.
4. **NEVER use `thoth_defer_reflect`** as a workaround — it bypasses the review chain and is not auditable.

---

## Change Scope Verification

After completing your task and before committing:

```
thoth_detect_changes({diff: "<git diff of your changes>"})
```

Verify that:
- Only symbols in your `task.files` are affected
- No unexpected blast radius (transitive changes you didn't intend)
- If unexpected symbols appear, report them to the orchestrator — do not commit

This is a quick sanity check, not a blocker if the changes are intentional.

---

## Memory Detail Lookup

When `thoth_recall` returns a fact or lesson with a partial match (truncated content or unclear context):

```
thoth_memory_detail({kind: "fact|lesson", index: <N>})
```

Use this to read the full content before acting on it. Do not make assumptions based on truncated recall results.
