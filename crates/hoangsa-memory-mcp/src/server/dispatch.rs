//! JSON-RPC method dispatch: `initialize`, `tools/*`, `resources/*`,
//! `prompts/*`, and the hoangsa-memory-private `hoangsa-memory.call` extension.

use serde::Deserialize;
use serde_json::{Value, json};
use hoangsa_memory_core::Event;
use hoangsa_memory_policy::CurationConfig;
use time::OffsetDateTime;
use tracing::warn;
use uuid::Uuid;

use crate::proto::{
    CallToolResult, Capabilities, ContentBlock, GetPromptResult, InitializeResult,
    MCP_PROTOCOL_VERSION, PromptMessage, Resource, ResourceContents, RpcError, RpcIncoming,
    RpcResponse, ServerInfo, ToolOutput, error_codes,
};

use super::Server;
use super::catalog::{
    arg_str, prompts_catalog, render_grounding_prompt, render_nudge_prompt,
    render_reflect_prompt, tools_catalog,
};

/// URI of the `MEMORY.md` resource.
const MEMORY_URI: &str = "hoangsa-memory://memory/MEMORY.md";
/// URI of the `LESSONS.md` resource.
const LESSONS_URI: &str = "hoangsa-memory://memory/LESSONS.md";

impl Server {
    /// Dispatch a single request. Returns `Ok(None)` for notifications.
    pub async fn handle(&self, msg: RpcIncoming) -> Option<RpcResponse> {
        let is_note = msg.is_notification();
        let id = msg.id.clone().unwrap_or(Value::Null);

        let outcome = match msg.method.as_str() {
            "initialize" => Ok(self.initialize()),
            "initialized" | "notifications/initialized" => {
                // Notification — silently accept.
                return None;
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.tools_list()),
            "tools/call" => self.tools_call(msg.params).await,
            // hoangsa-memory-private extension: same dispatch as
            // `tools/call` but returns the raw `ToolOutput` (with structured
            // `data`) instead of the text-only `CallToolResult`. Consumed
            // by the CLI thin-client so it can honour `--json` and
            // pretty-print.
            "hoangsa-memory.call" => self.memory_call(msg.params).await,
            "resources/list" => Ok(self.resources_list()),
            "resources/read" => self.resources_read(msg.params).await,
            "prompts/list" => Ok(self.prompts_list()),
            "prompts/get" => self.prompts_get(msg.params).await,
            other => Err(RpcError::new(
                error_codes::METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            )),
        };

        if is_note {
            if let Err(e) = &outcome {
                warn!(code = e.code, msg = %e.message, "notification error (dropped)");
            }
            return None;
        }

        Some(match outcome {
            Ok(result) => RpcResponse::ok(id, result),
            Err(err) => RpcResponse::err(id, err),
        })
    }

    // ---- method handlers --------------------------------------------------

    fn initialize(&self) -> Value {
        let result = InitializeResult {
            protocol_version: MCP_PROTOCOL_VERSION,
            capabilities: Capabilities {
                tools: Some(json!({})),
                resources: Some(json!({})),
                prompts: Some(json!({})),
            },
            server_info: ServerInfo {
                name: "hoangsa-memory-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };
        serde_json::to_value(result).unwrap_or_else(|_| json!({}))
    }

    fn tools_list(&self) -> Value {
        json!({ "tools": tools_catalog() })
    }

    /// MCP `tools/call` — returns a text-only [`CallToolResult`] (which is
    /// what every MCP client understands). The structured `data` half of
    /// [`ToolOutput`] is dropped; clients wanting the machine-readable
    /// form should call [`Self::memory_call`] via `hoangsa-memory.call`
    /// instead.
    async fn tools_call(&self, params: Value) -> Result<Value, RpcError> {
        let out = self.dispatch_tool(params).await?;
        let wrapped = CallToolResult {
            content: vec![ContentBlock::text(out.text)],
            is_error: out.is_error,
        };
        serde_json::to_value(wrapped)
            .map_err(|e| RpcError::new(error_codes::INTERNAL_ERROR, e.to_string()))
    }

    /// hoangsa-memory-private `hoangsa-memory.call` — returns the raw
    /// [`ToolOutput`] so the CLI thin-client can honour `--json` and
    /// pretty-print structured data. Dispatch logic is shared with
    /// [`Self::tools_call`].
    async fn memory_call(&self, params: Value) -> Result<Value, RpcError> {
        let out = self.dispatch_tool(params).await?;
        serde_json::to_value(out)
            .map_err(|e| RpcError::new(error_codes::INTERNAL_ERROR, e.to_string()))
    }

    /// Shared dispatch used by both `tools/call` and `hoangsa-memory.call`. Tool
    /// errors are folded into `ToolOutput { is_error: true, .. }` so the
    /// RPC layer can still emit a successful envelope (callers inspect
    /// `is_error` on the payload).
    pub(super) async fn dispatch_tool(&self, params: Value) -> Result<ToolOutput, RpcError> {
        #[derive(Deserialize)]
        struct CallParams {
            name: String,
            #[serde(default)]
            arguments: Value,
        }
        let CallParams { name, arguments } = serde_json::from_value(params)
            .map_err(|e| RpcError::new(error_codes::INVALID_PARAMS, e.to_string()))?;

        let result = match name.as_str() {
            "memory_recall" => self.tool_recall(arguments).await,
            "memory_index" => self.tool_index(arguments).await,
            "memory_remember_fact" => self.tool_remember_fact(arguments).await,
            "memory_remember_lesson" => self.tool_remember_lesson(arguments).await,
            "memory_remember_preference" => self.tool_remember_preference(arguments).await,
            "memory_replace" => self.tool_memory_replace(arguments).await,
            "memory_remove" => self.tool_memory_remove(arguments).await,
            "memory_skills_list" => self.tool_skills_list().await,
            "memory_show" => self.tool_memory_show().await,
            "memory_wakeup" => self.tool_wakeup(arguments).await,
            "memory_detail" => self.tool_memory_detail(arguments).await,
            "memory_skill_propose" => self.tool_skill_propose(arguments).await,
            "memory_impact" => self.tool_impact(arguments).await,
            "memory_symbol_context" => self.tool_symbol_context(arguments).await,
            "memory_event_trace" => self.tool_event_trace(arguments).await,
            "memory_detect_changes" => self.tool_detect_changes(arguments).await,
            "memory_turn_save" => self.tool_turn_save(arguments).await,
            "memory_turns_search" => self.tool_turns_search(arguments).await,
            "memory_archive_status" => self.tool_archive_status().await,
            "memory_archive_topics" => self.tool_archive_topics(arguments).await,
            "memory_archive_search" => self.tool_archive_search(arguments).await,
            "memory_archive_ingest" => self.tool_archive_ingest(arguments).await,
            "memory_graph_query" => self.tool_graph_query(arguments).await,
            "memory_graph_paths" => self.tool_graph_paths(arguments).await,
            "memory_graph_communities" => self.tool_graph_communities(arguments).await,
            "memory_graph_processes" => self.tool_graph_processes(arguments).await,
            other => {
                return Err(RpcError::new(
                    error_codes::METHOD_NOT_FOUND,
                    format!("unknown tool: {other}"),
                ));
            }
        };

        Ok(match result {
            Ok(out) => out,
            Err(e) => ToolOutput::error(format!("{e:#}")),
        })
    }

    fn resources_list(&self) -> Value {
        let resources = vec![
            Resource {
                uri: MEMORY_URI.to_string(),
                name: "MEMORY.md".to_string(),
                description:
                    "Declarative facts (full text). For a compact index, use memory_wakeup."
                        .to_string(),
                mime_type: "text/markdown".to_string(),
            },
            Resource {
                uri: LESSONS_URI.to_string(),
                name: "LESSONS.md".to_string(),
                description: "Lessons learned (full text). For a compact index, use memory_wakeup."
                    .to_string(),
                mime_type: "text/markdown".to_string(),
            },
        ];
        json!({ "resources": resources })
    }

    async fn resources_read(&self, params: Value) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct ReadParams {
            uri: String,
        }
        let ReadParams { uri } = serde_json::from_value(params)
            .map_err(|e| RpcError::new(error_codes::INVALID_PARAMS, e.to_string()))?;

        let file = match uri.as_str() {
            MEMORY_URI => "MEMORY.md",
            LESSONS_URI => "LESSONS.md",
            other => {
                return Err(RpcError::new(
                    error_codes::INVALID_PARAMS,
                    format!("unknown resource uri: {other}"),
                ));
            }
        };

        let path = self.inner.root.join(file);
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(RpcError::new(error_codes::INTERNAL_ERROR, e.to_string())),
        };

        let contents = ResourceContents {
            uri,
            mime_type: "text/markdown".to_string(),
            text,
        };
        Ok(json!({ "contents": [contents] }))
    }

    // ---- prompts ----------------------------------------------------------

    fn prompts_list(&self) -> Value {
        let disc = CurationConfig::load_or_default_sync(&self.inner.root);
        json!({ "prompts": prompts_catalog(disc.grounding_check) })
    }

    async fn prompts_get(&self, params: Value) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct GetParams {
            name: String,
            #[serde(default)]
            arguments: serde_json::Map<String, Value>,
        }
        let GetParams { name, arguments } = serde_json::from_value(params)
            .map_err(|e| RpcError::new(error_codes::INVALID_PARAMS, e.to_string()))?;

        let (description, body) = match name.as_str() {
            "memory_reflect" => (
                "Reflect on the session so far and decide what to remember.",
                render_reflect_prompt(&arguments),
            ),
            "memory_nudge" => {
                // Record that the agent actually expanded the nudge prompt —
                // strict-mode gates use this to distinguish "ran a recall"
                // from "actually reflected on lessons".
                let intent = arg_str(&arguments, "intent").to_string();
                let ev = Event::NudgeInvoked {
                    id: Uuid::new_v4(),
                    intent: intent.clone(),
                    at: OffsetDateTime::now_utc(),
                };
                match self.resources().await {
                    Ok(res) => {
                        if let Err(e) = res.store.episodes.append(&ev).await {
                            warn!(error = %e, "failed to log NudgeInvoked event");
                        }
                    }
                    Err(e) => warn!(error = %e, "failed to open resources for NudgeInvoked"),
                }
                (
                    "Nudge before a risky step: recall relevant lessons and plan.",
                    render_nudge_prompt(&arguments),
                )
            }
            "memory_grounding_check" => (
                "Verify a claim against the indexed codebase before asserting it.",
                render_grounding_prompt(&arguments),
            ),
            other => {
                return Err(RpcError::new(
                    error_codes::INVALID_PARAMS,
                    format!("unknown prompt: {other}"),
                ));
            }
        };

        let result = GetPromptResult {
            description: description.to_string(),
            messages: vec![PromptMessage {
                role: "user".to_string(),
                content: ContentBlock::text(body),
            }],
        };
        serde_json::to_value(result)
            .map_err(|e| RpcError::new(error_codes::INTERNAL_ERROR, e.to_string()))
    }
}
