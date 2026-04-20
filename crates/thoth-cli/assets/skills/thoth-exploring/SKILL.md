---
name: thoth-exploring
description: >
  Use when the user asks how code works, wants to understand
  architecture, trace execution flows, or explore unfamiliar parts of
  the codebase. Examples: "how does X work?", "what calls this
  function?", "show me the auth flow", "where is the DB logic?".
metadata:
  version: "0.0.1"
---

# Exploring Codebases with Thoth

Understand unfamiliar code without reading every file. The hybrid
retriever (`thoth_recall`) finds entry points by intent; the graph
(`thoth_symbol_context`) walks relationships.

## When to Use

- "How does authentication work?"
- "What's the overall architecture?"
- "Show me the main components"
- "Where is the database layer?"
- Onboarding to an unfamiliar repo.

## Workflow

```
1. resources/read thoth://memory/MEMORY.md           → durable facts first
2. thoth_recall({query: "<concept>"})                 → candidate chunks
3. thoth_symbol_context({fqn})                        → 360° around each
4. Walk callers to find entry points                  → root of flow
5. Cite chunk ids in the explanation
```

### 1. Read durable memory

Before recalling, skim `MEMORY.md`. Facts there encode architectural
decisions and conventions you'd otherwise miss. If a fact names the
module you're about to explore, it saves a recall.

```
resources/read { uri: "thoth://memory/MEMORY.md" }
resources/read { uri: "thoth://memory/LESSONS.md" }
```

(Or equivalently: `thoth_memory_show`.)

### 2. Recall by intent

Use the user's own vocabulary. For "how does auth work", try:

```
thoth_recall { query: "authentication login session token" }
```

Prefer **nouns** over verbs. The hybrid retriever rewards topical
density. If the first query misses, widen with synonyms or narrow with
concrete symbols you see in the README.

### 3. Drill with symbol_context

Pick the top-ranked chunk's FQN. Then:

```
thoth_symbol_context { fqn: "auth::verify_token", limit: 16 }
```

Sections to read:

- **`callers`** — who invokes this → likely entry points.
- **`callees`** — what it delegates to → next layer.
- **`siblings`** — other symbols in the same file → adjacent concerns.
- **`references`** — non-call uses (type refs, trait bounds).
- **`extends` / `extended_by`** — inheritance / impl relationships.

### 4. Walk upward to the entry point

The entry point of a flow usually has few callers (it's a request
handler, a CLI command, a main loop). Repeatedly follow `callers`
until the list collapses to 0–1. That symbol is the flow's root.

Once you have root + N inner symbols, the story writes itself:

> Auth starts at `router::handle_login` (1 caller: `main::serve`).
> It calls `auth::verify_credentials` → `db::users::find_by_email` →
> `password::verify_hash`. On success, `auth::issue_token` signs a
> JWT via `crypto::sign`. All five symbols live under `crates/auth/`.

### 5. Cite chunks

Every claim about behaviour should cite the chunk id from step 2 or a
FQN+line from step 3. Ungrounded claims about "what the code does"
are the #1 source of drift in exploratory sessions.

## Anti-patterns

- **Skimming the file tree instead of recalling.** Names lie. The
  retriever ranks by semantic + lexical + graph signals; directory
  names are only one of those.
- **Ignoring MEMORY.md.** If a fact says "auth lives in crates/net,
  not crates/auth — historical accident", you save 20 minutes of
  misdirected recall.
- **Stopping at the first chunk.** The top-ranked hit is the *most
  similar* chunk, not necessarily the *most important*. Read the top
  3–5 before deciding where to drill.
- **Recalling on full sentences.** "How does the system authenticate
  users?" is noisier than `authentication login session`. Strip
  function words.

## When recall returns nothing

If `thoth_recall` gives `(no matches — did you run thoth_index?)`,
the graph is empty. Run `thoth index .` (CLI) or `thoth_index` (MCP)
and retry. If it still returns nothing, the concept genuinely isn't
indexed — ask the user for a file path to anchor from, or fall back to
`Grep` for a literal keyword.
