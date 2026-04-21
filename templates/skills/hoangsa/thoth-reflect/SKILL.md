---
name: thoth-reflect
description: >
  This skill should be used at the end of a coding session, after a
  bug fix, a finished feature, a deployment, or whenever the user says
  "we're done", "wrap up", "summarize the session", "what did we learn",
  or "save what you learned". It drives a structured self-reflection
  pass that decides whether to persist new preferences, facts, lessons,
  or skills to Thoth's long-term memory. Also triggers on phrases like
  "reflect", "postmortem", or "retrospective" applied to the current
  session.
metadata:
  version: "0.1.0"
---

# Thoth Reflect

Drive a deliberate reflection pass over the session that just ended.
The goal is to extract durable memory — not chat noise. Thoth already
captured the raw episodic log; your job is to decide what deserves to
survive after the TTL sweeps the log clean.

## Procedure

### 1. Pull the timeline

Call the Thoth MCP server:

- `thoth_memory_show` — current `USER.md` + `MEMORY.md` + `LESSONS.md`
  contents, so you know what's already there and don't duplicate it.
- `resources/read thoth://memory/MEMORY.md` (same data, different wire
  shape — use whichever your client supports).

If the session had enough tool calls to make a summary expensive, ask the
user for a one-paragraph recap instead of reading the full log.

### 2. Decide across three surfaces

Walk the session and ask three questions in order:

1. **Did the user reveal a durable preference?** Tone, language, testing
   style, commit style, workflow choice — anything that would shape HOW
   you respond in future sessions, regardless of project. Save as a
   **preference** → USER.md.
2. **Did we discover a project-specific invariant?** A non-obvious fact
   about this repo's architecture, a naming convention, a gotcha that
   lives across sessions. Save as a **fact** → MEMORY.md.
3. **Did we learn a pattern worth replaying?** A situation where a
   specific approach beat the obvious one — phrased as `when X → do Y`.
   Save as a **lesson** → LESSONS.md.

If none of the three apply, reply `no memory needed` and stop.

### 3. Persist

For each decision from step 2, call the matching tool.

**Preference** (first-person, cross-project):

```
thoth_remember_preference {
  "text": "User prefers concise Vietnamese responses in code review
           contexts, with commit messages in English.",
  "tags": ["style", "language"]
}
```

**Fact** (project-specific invariant):

```
thoth_remember_fact {
  "text": "The HTTP client in crates/net uses its own retry wrapper; the
           reqwest defaults are overridden in src/client.rs:42.",
  "tags": ["net", "retry"]
}
```

Keep the first line crisp — it becomes the MEMORY.md heading. Don't
restate things already visible from the file tree (e.g. "this repo is a
Rust workspace" is not a fact worth persisting).

**Lesson** (action-triggered advice):

```
thoth_remember_lesson {
  "trigger": "adding a retry to an HTTP call in this repo",
  "advice": "Use the existing RetryPolicy in crates/net/retry.rs; do not
             add reqwest middleware directly or it double-retries."
}
```

Lessons fire when their `trigger` matches a future intent, so write the
trigger as a situation description, not a command.

### 4. Handle `cap_exceeded`

If any of the three tools returns a structured `cap_exceeded` error, do
NOT silently drop the entry. The error payload includes a `preview` list
— pick a stale entry from it, then call `thoth_memory_replace` (to
consolidate) or `thoth_memory_remove` (to drop), and retry the remember
call. Reflection is exactly when the memory store is most likely to be
near its cap, so you MUST know this path.

See the `memory-discipline` skill for the full cap-exceeded flow.

### 5. Confirm and close

Write a two-line summary to the user:

```
Saved to memory:
  - preference: "Vietnamese responses, English commits"
  - fact:       "HTTP retry lives in crates/net/retry.rs"
  - lesson:     "trigger → advice"
  (or) no durable memory this session.
```

Don't dump the full USER.md/MEMORY.md back — the user can open the files.

## Quality gates

A memory entry is worth persisting only if it clears the bar for its
surface:

**Preference (USER.md)** — YES only if:
- the user said or implied it directly ("I prefer X", correcting your
  default behaviour, accepting an unusual choice without pushback),
- it applies across projects, not just this one,
- it's durable (style, language, workflow — not "use model X" for a
  one-shot task).

**Fact (MEMORY.md)** — YES only if:
- it would save a future session a round-trip (e.g. a recall that failed
  and a chunk that wasn't indexed),
- it encodes a decision or convention, not a raw file path,
- it wouldn't be obvious from a five-minute read of the README,
- it's not already in MEMORY.md under a similar heading.

**Lesson (LESSONS.md)** — YES only if:
- the trigger is specific enough to fire in the right situation but
  general enough to recur,
- the advice encodes a non-obvious pattern (the obvious path failed, or
  there's a hidden constraint),
- it's not already in LESSONS.md under a similar trigger.

If none of those clear the bar, emit `no memory needed` and stop. False
positives pollute the store faster than missing entries hurt.

## Failure modes to avoid

- **Diary-style dumps.** "Today we fixed the parser." Not a fact. Skip.
- **Overly specific code lines.** The code graph already holds that.
  Facts should live at a higher level than symbols.
- **Duplicate lessons.** Check existing LESSONS.md first; if the trigger
  already exists, bump its confidence via `thoth_lesson_outcome` instead
  of appending a near-duplicate.
- **Lessons for one-off bugs.** A lesson earns its place by being likely
  to recur across sessions.
- **Project facts saved as preferences.** If it's specific to this
  repo's code, it belongs in MEMORY.md, not USER.md. Preferences cross
  project boundaries.
- **Cap-blind appends.** If MEMORY/LESSONS is near its cap, prefer
  `thoth_memory_replace` (consolidate into an existing heading) over
  adding a new entry that will push the file over the cap.
