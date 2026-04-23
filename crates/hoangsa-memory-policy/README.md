# hoangsa-memory-policy

The memory lifecycle layer — the policy core of **Hoangsa Memory**.

It owns:

- The markdown source of truth (`MEMORY.md`, `LESSONS.md`, `USER.md`,
  `skills/*/SKILL.md`).
- TTL-based forgetting for episodic memory.
- Confidence evolution for lessons (reinforcement from outcomes).
- The **nudge** flow in `Mode::Full`: at session end, ask the
  `Synthesizer` whether any new fact, lesson, or skill should be
  persisted.

Design goals (per `DESIGN.md` §5 and §9):

- Deterministic in `Mode::Zero` (TTL + hard delete only — no LLM).
- LLM-curated in `Mode::Full` (nudge instead of algorithmic salience
  scoring — see Hermes).
- Markdown files remain first-class so humans can review diffs in git.

---

## Install (as a dependency)

```toml
[dependencies]
hoangsa-memory-policy = "0.2"
```

---

## Public API

```rust
use hoangsa_memory_policy::{
    MemoryManager,               // append / forget / compact with caps enforced
    WorkingMemory, WorkingNote,  // in-session scratchpad
    ForgetReport,                // structured TTL sweep output
    CapExceededError,            // per-file hard cap enforcement
    MarkdownStoreMemoryExt,      // guarded append extension trait
    check_content_policy,        // lint for redactable secrets
};
use hoangsa_memory_policy::config::{CurationConfig, MemoryConfig};
```

### Caps

`MEMORY.md`, `LESSONS.md`, and `USER.md` each have a soft cap (warn) and
hard cap (reject). When the hard cap is hit, the append returns
`CapExceededError` — callers must compact before retrying. The MCP
server surfaces this to the client so the model can decide what to prune.

### TTL decay

Episodic events age out on a configurable schedule. The rest of the
memory (facts / lessons / preferences) is **not** auto-decayed — it only
disappears on explicit `memory_remove` or `memory_replace`.

---

## License

MIT OR Apache-2.0.
