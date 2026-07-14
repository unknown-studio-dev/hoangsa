//! Tool and prompt catalogs advertised via `tools/list` / `prompts/list`,
//! plus the prompt renderers.

use serde_json::{Value, json};

use crate::proto::{Prompt, PromptArgument, Tool};

// ===========================================================================
// Tool catalog
// ===========================================================================

pub(super) fn tools_catalog() -> Vec<Tool> {
    vec![
        Tool {
            name: "memory_recall".to_string(),
            description: "Hybrid recall (symbol + BM25 + graph + markdown + semantic) over the \
                          code memory. Returns ranked chunks with path, line span, preview, and \
                          graph context (callers/callees/imports). Bodies are stripped by default \
                          — agents should `Read path:L-L` on a hit if they need the full body. \
                          Pass `detail: true` to get bodies inline. Use `scope` to include archived \
                          conversations."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural language or keyword query." },
                    "top_k": { "type": "integer", "minimum": 1, "maximum": 64, "default": 8 },
                    "scope": {
                        "type": "string",
                        "enum": ["curated", "archive", "all"],
                        "default": "curated",
                        "description": "What to search: 'curated' (default) = code + facts/lessons, \
                                        'archive' = verbatim conversations only, \
                                        'all' = code + facts/lessons + archive."
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Filter facts to those with any of these tags (wing/scope filter)."
                    },
                    "min_score": {
                        "type": "number",
                        "minimum": 0.0,
                        "default": 0.0,
                        "description": "Absolute fused-score floor. Chunks below this are dropped. \
                                        An internal noise floor (~0.05) also always applies — set this \
                                        higher only to demand stronger hits."
                    },
                    "detail": {
                        "type": "boolean",
                        "default": false,
                        "description": "Return full chunk bodies inline. Default `false` — recall \
                                        returns coordinates (path + line span + preview); caller \
                                        should `Read path:L-L` for full content. Set `true` when \
                                        you need bodies in one round trip."
                    },
                    "log_event": {
                        "type": "boolean",
                        "default": true,
                        "description": "Whether to persist this call as a `query_issued` event in \
                                        episodes.db. Agent-initiated recalls (default true) MUST log \
                                        — that's how `hoangsa-cli enforce` proves the agent consulted memory \
                                        before mutating. Automated hooks that auto-recall for context \
                                        injection (e.g. UserPromptSubmit) pass `false` so their \
                                        ceremonial recall doesn't satisfy the gate on the agent's behalf."
                    }
                },
                "required": ["query"]
            }),
        },
        Tool {
            name: "memory_index".to_string(),
            description: "Walk a source tree, parse every supported file, and populate the \
                          indexes (symbols, call graph, BM25, chunks)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Source path. Defaults to '.'." }
                }
            }),
        },
        Tool {
            name: "memory_remember_fact".to_string(),
            description: "Append a semantic fact to MEMORY.md. Use this when you learn \
                          something about the codebase that should survive across sessions."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The fact itself. First line becomes the heading." },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tags for later filtering."
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["always", "on-demand"],
                        "default": "always",
                        "description": "always = injected every session start; on-demand = only surfaced via memory_recall."
                    }
                },
                "required": ["text"]
            }),
        },
        Tool {
            name: "memory_remember_lesson".to_string(),
            description: "Append a reflective lesson to LESSONS.md. Use this after a mistake \
                          or surprise so future sessions can avoid the trap. `trigger` may be \
                          a plain string (legacy) or a structured object with optional \
                          `tool` / `path_glob` / `cmd_regex` / `content_regex` matchers plus \
                          a required `natural` description. `suggested_enforcement` is audit- \
                          only — the lesson is always saved at `Advise` tier; promotion is \
                          evidence-driven by the outcome harvester (REQ-03)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "trigger": {
                        "oneOf": [
                            {
                                "type": "string",
                                "description": "Legacy natural-language trigger."
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "tool":           { "type": "string", "description": "Tool name filter: Edit/Write/Bash/etc." },
                                    "path_glob":      { "type": "string", "description": "Glob for Edit/Write/Read path." },
                                    "cmd_regex":      { "type": "string", "description": "Regex for Bash command strings." },
                                    "content_regex":  { "type": "string", "description": "Regex for Edit old_string/new_string." },
                                    "natural":        { "type": "string", "description": "Human-readable trigger description." }
                                },
                                "required": ["natural"]
                            }
                        ]
                    },
                    "advice":  { "type": "string", "description": "The lesson / rule itself." },
                    "suggested_enforcement": {
                        "type": "string",
                        "enum": ["Advise", "Require", "Block", "WorkflowGate"],
                        "description": "Tier the proposer suggests. Audit only — stored lesson enforcement starts at Advise."
                    },
                    "block_message": {
                        "type": "string",
                        "description": "Message shown via stderr when this lesson blocks a tool call (used once promoted to Block)."
                    },
                    "stage": {
                        "type": "boolean",
                        "default": false,
                        "description": "Force staging to LESSONS.pending.md even in auto-commit mode."
                    }
                },
                "required": ["trigger", "advice"]
            }),
        },
        Tool {
            name: "memory_remember_preference".to_string(),
            description: "Append a user preference to USER.md. Returns a structured \
                          `cap_exceeded` / `content_policy` error (isError=true) when the \
                          write would exceed `[memory].cap_user_bytes` or the content policy \
                          rejects the payload."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The preference itself. First line becomes the heading." },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tags for later filtering."
                    }
                },
                "required": ["text"]
            }),
        },
        Tool {
            name: "memory_replace".to_string(),
            description: "Replace one entry in MEMORY.md / LESSONS.md / USER.md identified by \
                          a substring match. Use this to update an existing fact / lesson / \
                          preference instead of appending a near-duplicate (REQ-04)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind":     { "type": "string", "enum": ["fact", "lesson", "preference"] },
                    "query":    { "type": "string", "description": "Substring identifying the entry to replace." },
                    "new_text": { "type": "string", "description": "Replacement entry body." }
                },
                "required": ["kind", "query", "new_text"]
            }),
        },
        Tool {
            name: "memory_remove".to_string(),
            description: "Remove one entry from MEMORY.md / LESSONS.md / USER.md identified by \
                          a substring match. Use this to prune obsolete entries after a cap \
                          hit (REQ-05)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind":  { "type": "string", "enum": ["fact", "lesson", "preference"] },
                    "query": { "type": "string", "description": "Substring identifying the entry to remove." }
                },
                "required": ["kind", "query"]
            }),
        },
        Tool {
            name: "memory_skills_list".to_string(),
            description: "List every installed skill under .hoangsa/memory/skills/.".to_string(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "memory_show".to_string(),
            description: "Return the current MEMORY.md, LESSONS.md, and USER.md as plain text. \
                          For large memory sets, prefer memory_wakeup (compact index) + \
                          memory_detail (drill into specific entries)."
                .to_string(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "memory_wakeup".to_string(),
            description: "Compact one-line-per-entry index of facts, lessons, and user preferences. \
                          By default only shows `always`-scope facts (core context). \
                          Pass `include_on_demand: true` to also show on-demand facts. \
                          Use at session start for a cheap overview, then call \
                          memory_detail for specific entries."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "string",
                        "enum": ["all", "facts", "lessons", "preferences"],
                        "default": "all",
                        "description": "Which memory surface to index."
                    },
                    "include_on_demand": {
                        "type": "boolean",
                        "default": false,
                        "description": "When true, also include on-demand facts (normally only surfaced via memory_recall)."
                    }
                }
            }),
        },
        Tool {
            name: "memory_detail".to_string(),
            description: "Return the full content of a specific fact or lesson. \
                          Pass an index from memory_wakeup (e.g. 'F03', 'L01') or \
                          a heading substring to match."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Entry index (e.g. 'F03', 'L01') or heading substring."
                    }
                },
                "required": ["id"]
            }),
        },
        Tool {
            name: "memory_impact".to_string(),
            description: "Blast-radius analysis over the code graph. Given a symbol FQN, \
                          returns every reachable symbol grouped by distance. Use \
                          `direction=\"up\"` (default) to answer \"what breaks if I change \
                          this?\" (callers / references / subtypes); `\"down\"` for \
                          \"what does this depend on?\" (callees / parent types)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "fqn": { "type": "string", "description": "Fully qualified name (module::symbol)." },
                    "direction": {
                        "type": "string",
                        "enum": ["up", "down", "both"],
                        "default": "up"
                    },
                    "depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 8,
                        "default": 3
                    }
                },
                "required": ["fqn"]
            }),
        },
        Tool {
            name: "memory_symbol_context".to_string(),
            description: "360-degree view of a single symbol: callers, callees, parent types, \
                          subtypes, references, siblings, and unresolved imports. Use this \
                          when you already know the FQN of a symbol and want structured context \
                          around it (post-`memory_recall` drill-down)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "fqn": { "type": "string", "description": "Fully qualified name (module::symbol)." },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 128,
                        "default": 32,
                        "description": "Per-section cap on the returned neighbours."
                    }
                },
                "required": ["fqn"]
            }),
        },
        Tool {
            name: "memory_event_trace".to_string(),
            description: "Trace publishers and subscribers of an event-bus topic. \
                          Given a `topic` string (and optionally a `bus` receiver \
                          name), returns every indexed function that emits the \
                          topic and every handler subscribed to it. Use this when \
                          following a pub/sub flow that `memory_symbol_context` \
                          can't connect because publisher and subscriber are \
                          decoupled by a broker."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "topic": { "type": "string", "description": "Event topic string (e.g. \"user.created\")." },
                    "bus":   { "type": "string", "description": "Optional receiver name to disambiguate (`bus`, `socket`, …). Omit to scan all buses." },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 128,
                        "default": 32,
                        "description": "Per-section cap on emitters / subscribers."
                    }
                },
                "required": ["topic"]
            }),
        },
        Tool {
            name: "memory_detect_changes".to_string(),
            description: "Parse a unified diff (e.g. `git diff`), find every indexed symbol \
                          whose declaration span overlaps a changed hunk, and return their \
                          upstream blast radius. Ideal as a PR pre-check — answers \"which \
                          code is downstream of my edit and should be re-tested?\"."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "diff":  { "type": "string", "description": "Unified diff text (`git diff` output)." },
                    "depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 6,
                        "default": 2,
                        "description": "Blast-radius depth (BFS levels of callers / references / subtypes)."
                    }
                },
                "required": ["diff"]
            }),
        },
        Tool {
            name: "memory_skill_propose".to_string(),
            description: "Draft a new SKILL.md under .hoangsa/memory/skills/<slug>.draft/ — used when \
                          you've noticed ≥5 related lessons and want to consolidate them into \
                          a reusable skill. The user promotes via `hoangsa-memory skills install`."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "slug":            { "type": "string", "description": "kebab-case slug for the draft directory." },
                    "body":            { "type": "string", "description": "Full SKILL.md body starting with `---` frontmatter." },
                    "source_triggers": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Triggers of the lessons this skill consolidates."
                    }
                },
                "required": ["slug", "body"]
            }),
        },
        // ---- conversation turn tools ----
        Tool {
            name: "memory_turn_save".to_string(),
            description: "Save a verbatim conversation turn (user or assistant) to the \
                          episodic log. Called automatically by hooks or manually by the \
                          agent to preserve important exchanges. Optional `commit_sha` + \
                          `file_paths` let `memory_archive_search` link a turn back to the \
                          code state / files changed at that moment."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_id":  { "type": "string", "description": "Session identifier." },
                    "role":        { "type": "string", "enum": ["user", "assistant"] },
                    "content":     { "type": "string", "description": "Verbatim turn content." },
                    "commit_sha":  {
                        "type": "string",
                        "description": "Optional git HEAD sha at the time of the turn (full or short)."
                    },
                    "file_paths":  {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional file paths touched around this turn."
                    }
                },
                "required": ["session_id", "role", "content"]
            }),
        },
        Tool {
            name: "memory_turns_search".to_string(),
            description: "Full-text search over saved conversation turns. Returns matching \
                          turns with session context."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query (FTS5 MATCH)." },
                    "top_k": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10 }
                },
                "required": ["query"]
            }),
        },
        // ---- archive tools ----
        Tool {
            name: "memory_archive_status".to_string(),
            description: "Archive summary: total sessions, turns, and curated count. \
                          ~100 tokens. Good for L0 orientation."
                .to_string(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "memory_archive_topics".to_string(),
            description: "List topics in the conversation archive with session and turn counts. \
                          Optionally filter by project."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Filter by project name." }
                }
            }),
        },
        Tool {
            name: "memory_archive_search".to_string(),
            description: "Semantic search across archived verbatim conversations stored in the \
                          in-process vector store. Returns the most relevant conversation turns. \
                          Use this to find past discussions, decisions, and context."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural language search query." },
                    "top_k": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10 },
                    "project": { "type": "string", "description": "Filter by project name." },
                    "topic": { "type": "string", "description": "Filter by topic." }
                },
                "required": ["query"]
            }),
        },
        Tool {
            name: "memory_archive_ingest".to_string(),
            description: "Ingest Claude Code conversation sessions into the archive via the \
                          daemon, reusing the already-initialised embedder. Invoked by hook \
                          forwarding (PreCompact / SessionEnd) so concurrent Claude Code \
                          sessions don't each reload the fastembed ONNX model."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Only ingest sessions from this project." },
                    "topic":   { "type": "string", "description": "Override auto-detected topic for all ingested sessions." },
                    "refresh": { "type": "boolean", "description": "Re-ingest already-seen sessions (pick up new turns).", "default": false },
                    "limit":   { "type": "integer", "minimum": 0, "description": "Cap ingest at N most recent session files. 0 disables the implicit first-run cap." }
                }
            }),
        },
        // ---- graph traversal / analytics tools ----
        Tool {
            name: "memory_graph_query".to_string(),
            description: "Trace how code connects: traverse from seed symbol(s) to their \
                          callers/callees/refs/imports, any depth, filtered by edge kind. \
                          Reach for this INSTEAD of repeated Grep/Read when answering 'who \
                          calls X', 'what does X reach', or mapping a dependency fan-out — \
                          one call replaces many searches. Returns a JSON (default) or \
                          Graphviz DOT subgraph; unknown FQNs land in `unresolved`, never errors."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "start": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Starting FQN(s) for BFS (exact or suffix-resolved)."
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["out", "in", "both"],
                        "default": "out",
                        "description": "Edge direction to follow."
                    },
                    "edge_kinds": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Filter to these edge kinds only (calls, imports, references, extends, declared_in, emits, subscribes). Omit for all kinds."
                    },
                    "max_depth": {
                        "type": "integer",
                        "minimum": 0,
                        "default": 3,
                        "description": "Maximum BFS depth."
                    },
                    "max_nodes": {
                        "type": "integer",
                        "minimum": 1,
                        "default": 500,
                        "description": "Maximum number of nodes in the output. `truncated` is set when the cap is hit."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["json", "dot"],
                        "default": "json",
                        "description": "Output format: json (default) or Graphviz DOT."
                    }
                },
                "required": ["start"]
            }),
        },
        Tool {
            name: "memory_graph_paths".to_string(),
            description: "Answer 'how does A reach B?': the shortest dependency/call path \
                          between two symbols. Use INSTEAD of hand-tracing call chains through \
                          Grep. Returns `found: false` when unreachable within max_depth; never \
                          errors on unknown FQNs."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": { "type": "string", "description": "Source FQN." },
                    "to":   { "type": "string", "description": "Destination FQN." },
                    "edge_kinds": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Restrict path search to these edge kinds. Omit for all kinds."
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["out", "in", "both"],
                        "default": "out"
                    },
                    "max_depth": {
                        "type": "integer",
                        "minimum": 1,
                        "default": 10,
                        "description": "Maximum hop count."
                    }
                },
                "required": ["from", "to"]
            }),
        },
        Tool {
            name: "memory_graph_communities".to_string(),
            description: "Architecture map: cluster tightly-coupled symbols via label \
                          propagation over Calls/Imports/Extends. Use to answer 'what are the \
                          main components/modules?' without reading the whole tree. Communities \
                          sorted largest-first; empty graph returns an empty list."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "min_size": {
                        "type": "integer",
                        "minimum": 1,
                        "default": 3,
                        "description": "Drop communities smaller than this."
                    }
                }
            }),
        },
        Tool {
            name: "memory_graph_processes".to_string(),
            description: "Walk execution flows from entry points (`::main` or `entry_globs`) \
                          down Calls edges. Use to answer 'walk me through what happens from \
                          startup' or 'trace the main flow' without reading files. One \
                          cycle-safe flow per entry point, up to max_depth."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "max_depth": {
                        "type": "integer",
                        "minimum": 1,
                        "default": 8,
                        "description": "Maximum DFS depth from each entry point."
                    },
                    "entry_globs": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Additional glob patterns for entry-point detection (supports * prefix/suffix). Symbols ending in `::main` are always included."
                    }
                }
            }),
        },
        Tool {
            name: "memory_taint_paths".to_string(),
            description: "Security dataflow: can untrusted input reach a dangerous sink? \
                          Traces source→sink taint over DataDep and Calls edges (never plain \
                          control-flow). Requires an index built with `--pdg`. Use to audit \
                          injection/exec risks. Returns findings (source, sink, path edges) \
                          plus truncated/source_matches/sink_matches; omitting sources/sinks \
                          uses built-in defaults (env vars, stdin, args → subprocess, eval, \
                          fs::write, query)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "sources": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Substring patterns for source nodes (FQN or text payload). Omit or leave empty to use built-in defaults."
                    },
                    "sinks": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Substring patterns for sink nodes (FQN or text payload). Omit or leave empty to use built-in defaults."
                    },
                    "max_depth": {
                        "type": "integer",
                        "minimum": 1,
                        "default": 12,
                        "description": "Maximum BFS depth from each source node."
                    },
                    "max_findings": {
                        "type": "integer",
                        "minimum": 1,
                        "default": 50,
                        "description": "Stop after this many findings; sets truncated=true when hit."
                    }
                }
            }),
        },
    ]
}

// ===========================================================================
// Prompts catalog
// ===========================================================================

/// Descriptors advertised by `prompts/list`. Each maps to a renderer in
/// [`Server::prompts_get`]; rendering is pure string substitution so the
/// server stays deterministic and dependency-free.
pub(super) fn prompts_catalog(grounding_enabled: bool) -> Vec<Prompt> {
    let mut prompts = vec![
        Prompt {
            name: "memory_reflect".to_string(),
            description:
                "End-of-step self-reflection: decide whether to save a lesson or fact based \
                 on what just happened."
                    .to_string(),
            arguments: vec![
                PromptArgument {
                    name: "summary".to_string(),
                    description: "One-paragraph summary of what the agent just did.".to_string(),
                    required: true,
                },
                PromptArgument {
                    name: "outcome".to_string(),
                    description: "What went right or wrong (tests, user feedback, etc.)."
                        .to_string(),
                    required: false,
                },
            ],
        },
        Prompt {
            name: "memory_nudge".to_string(),
            description:
                "Pre-action nudge: surface the most relevant lessons and force the agent to \
                 acknowledge them before proceeding."
                    .to_string(),
            arguments: vec![PromptArgument {
                name: "intent".to_string(),
                description: "What the agent is about to do.".to_string(),
                required: true,
            }],
        },
    ];
    if grounding_enabled {
        prompts.push(Prompt {
            name: "memory_grounding_check".to_string(),
            description: "Ask the agent to verify a factual claim against the indexed code before \
                 asserting it to the user."
                .to_string(),
            arguments: vec![PromptArgument {
                name: "claim".to_string(),
                description: "The claim to verify.".to_string(),
                required: true,
            }],
        });
    }
    prompts
}

pub(super) fn arg_str<'a>(args: &'a serde_json::Map<String, Value>, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("").trim()
}

pub(super) fn render_reflect_prompt(args: &serde_json::Map<String, Value>) -> String {
    let summary = arg_str(args, "summary");
    let outcome = arg_str(args, "outcome");
    format!(
        "You just finished a step. Reflect on it before moving on.\n\
         \n\
         ## What you did\n\
         {summary}\n\
         \n\
         ## Outcome observed\n\
         {outcome}\n\
         \n\
         ## Decide\n\
         1. Is there a durable FACT worth saving about this codebase?\n\
            If yes, call `memory_remember_fact` with a one-line summary.\n\
         2. Is there a LESSON — a non-obvious pattern a future session would miss?\n\
            If yes, call `memory_remember_lesson` with a crisp `trigger` and `advice`.\n\
         3. If neither, reply `no memory needed` and continue.\n\
         \n\
         Be conservative: only save memory that is useful, specific, and not \
         already obvious from the code itself.",
        summary = if summary.is_empty() {
            "(not provided)"
        } else {
            summary
        },
        outcome = if outcome.is_empty() {
            "(not provided)"
        } else {
            outcome
        },
    )
}

pub(super) fn render_nudge_prompt(args: &serde_json::Map<String, Value>) -> String {
    let intent = arg_str(args, "intent");
    format!(
        "Before you act, recall what past sessions learned.\n\
         \n\
         ## Intended action\n\
         {intent}\n\
         \n\
         ## Required checks\n\
         1. Call `memory_recall` with a short query derived from the intent above.\n\
         2. Read LESSONS.md via `resources/read hoangsa-memory://memory/LESSONS.md` and pick \
            every lesson whose `trigger` plausibly applies.\n\
         3. Restate the plan in one paragraph, naming each lesson you're honouring.\n\
         4. Only then execute. If a lesson advises against the plan, STOP and ask \
            the user before proceeding.",
        intent = if intent.is_empty() {
            "(not provided)"
        } else {
            intent
        },
    )
}

pub(super) fn render_grounding_prompt(args: &serde_json::Map<String, Value>) -> String {
    let claim = arg_str(args, "claim");
    format!(
        "Verify the following claim against the indexed codebase BEFORE asserting it.\n\
         \n\
         ## Claim\n\
         {claim}\n\
         \n\
         ## Procedure\n\
         1. Call `memory_recall` with the most load-bearing nouns from the claim.\n\
         2. Read the returned chunks and decide: supported, contradicted, or \
            insufficient evidence.\n\
         3. If supported, cite at least one chunk id when you answer the user.\n\
         4. If contradicted or insufficient, say so honestly — do not hedge.",
        claim = if claim.is_empty() {
            "(not provided)"
        } else {
            claim
        },
    )
}
