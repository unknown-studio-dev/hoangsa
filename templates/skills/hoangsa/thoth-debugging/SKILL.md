---
name: thoth-debugging
description: >
  Use when the user is debugging a bug, tracing an error, or asking
  why something fails. Examples: "why is X failing?", "where does this
  error come from?", "trace this bug", "who calls this method?",
  "this endpoint returns 500".
metadata:
  version: "0.0.1"
---

# Debugging with Thoth

Find the origin of a bug by walking the graph backwards from the
symptom. Recall locates the suspect symbol; `thoth_symbol_context`
walks its callers until you reach the real cause.

## When to Use

- "Why is this function failing?"
- "Trace where this error comes from"
- "Who calls this method?"
- "This endpoint returns 500, what's upstream?"
- Any investigation that starts with a symptom and wants a root cause.

## Workflow

```
1. resources/read thoth://memory/LESSONS.md           → known-bug lessons first
2. thoth_recall({query: "<error or symptom>"})        → find the suspect
3. thoth_symbol_context({fqn: <suspect>})             → callers / callees
4. thoth_impact({fqn, direction: "up"})               → full upstream
5. Read the suspect + its callers carefully
6. After the fix: thoth_lesson_outcome + remember_lesson
```

### 1. Check LESSONS.md first

A past session may have hit this bug. `LESSONS.md` entries are keyed
on trigger strings — scan for anything matching the symptom. If a
lesson says "this exact error comes from X", start there.

```
resources/read { uri: "thoth://memory/LESSONS.md" }
```

### 2. Recall the symptom

Feed the error message literally. Retrieval is strongest on exact
tokens (function names, error strings, log lines). For example:

```
thoth_recall { query: "connection refused retry pool exhausted" }
```

Include the error type, a noun from the failing operation, and any
concrete identifier (module, handler, queue name).

### 3. Drill with symbol_context

Once recall returns a candidate FQN:

```
thoth_symbol_context { fqn: "pool::checkout" }
```

Pay special attention to:

- **`callers`** — who invoked the failing path. Bugs often live in the
  caller's assumptions, not the callee.
- **`callees`** — what the suspect delegates to. The actual throw site
  may be 1–2 hops deeper.
- **`references`** — non-call uses. A misconfigured trait impl, a
  generic bound violation, a type that escapes.

### 4. Walk upstream when stuck

If context doesn't make the cause obvious, run:

```
thoth_impact { fqn: <suspect>, direction: "up", depth: 3 }
```

The depth-1 callers are the contexts in which the bug manifests.
Often the bug is "caller A passes a value the callee doesn't handle";
seeing all callers at once exposes the outlier.

### 5. Read the code

Once you have a narrow set of suspect files, use `Read` with line
ranges from the graph results — don't load whole files. The graph
already gave you `path:line` for every hit.

### 6. Reflect after the fix

When the fix lands and tests pass:

```
thoth_lesson_outcome {
  signal: "success",
  triggers: ["<trigger of any lesson you followed>"]
}
```

If the bug is durable and non-obvious (not a typo), persist a lesson:

```
thoth_remember_lesson {
  trigger: "seeing ETIMEDOUT from the pool on cold start",
  advice: "pool::checkout has a 5s dial timeout; raise it or pre-warm
           via pool::ensure_min in the init path."
}
```

Keep the trigger as a **situation description**, not a command.
Lessons fire on situation match; imperatives don't match future
recalls.

## Anti-patterns

- **Recalling the fix instead of the symptom.** "Fix retry on
  connection failure" matches the wrong chunks. Search what you see
  (the error), not what you want (the fix).
- **Reading whole files on a guess.** The graph gives exact
  `path:line`; use it. Reading a 2000-line file to find a 5-line bug
  burns context.
- **Fix-then-forget.** If the bug was subtle and the fix was non-obvious,
  a lesson saves 30 minutes next time. If the bug was a typo, skip.
- **Assuming the top recall hit is the bug.** Top-ranked ≠ guilty.
  Read the top 3 chunks; the guilty one is often #2 because the
  retriever rewarded surface similarity over actual behaviour.

## When the symptom isn't indexed

If the error message lives in a dependency (e.g. `reqwest: connection
refused`), recall won't find it — the error text isn't in your code.
Recall for the *call site* instead: `"reqwest get client"` or the
wrapper function's name. The graph will lead you to the caller, which
*is* in your code.
