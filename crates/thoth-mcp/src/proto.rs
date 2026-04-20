//! JSON-RPC 2.0 + MCP wire types.
//!
//! We intentionally keep this module tiny and dependency-free so the server
//! can be audited at a glance. Serialization uses `serde_json::Value` for the
//! payload halves to stay forward-compatible with MCP schema additions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 version literal we emit on every response.
pub const JSONRPC_VERSION: &str = "2.0";

/// The MCP protocol version this server implements.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

// ---- JSON-RPC envelope ----------------------------------------------------

/// An inbound JSON-RPC message. Both requests and notifications arrive here;
/// they're distinguished by the presence of `id`.
#[derive(Debug, Clone, Deserialize)]
pub struct RpcIncoming {
    /// JSON-RPC version. Must be `"2.0"`.
    #[serde(default)]
    pub jsonrpc: String,
    /// Request id. Absent for notifications.
    #[serde(default)]
    pub id: Option<Value>,
    /// Method name (e.g. `"tools/call"`).
    pub method: String,
    /// Opaque parameter blob.
    #[serde(default)]
    pub params: Value,
}

impl RpcIncoming {
    /// Is this a notification (i.e. no response expected)?
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// An outbound JSON-RPC response.
#[derive(Debug, Clone, Serialize)]
pub struct RpcResponse {
    /// Always `"2.0"`.
    pub jsonrpc: &'static str,
    /// Echo of the inbound id.
    pub id: Value,
    /// The successful result, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// The error, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    /// Build a successful response.
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Build an error response.
    pub fn err(id: Value, error: RpcError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize)]
pub struct RpcError {
    /// Error code (see [`error_codes`] or the JSON-RPC spec).
    pub code: i32,
    /// Human-readable message.
    pub message: String,
    /// Optional structured payload for diagnostics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// Construct with only `code` + `message`.
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Attach structured `data` to this error.
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

/// Standard JSON-RPC 2.0 error codes.
pub mod error_codes {
    /// Invalid JSON received.
    pub const PARSE_ERROR: i32 = -32700;
    /// The JSON sent is not a valid request.
    pub const INVALID_REQUEST: i32 = -32600;
    /// The method does not exist.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Invalid method parameters.
    pub const INVALID_PARAMS: i32 = -32602;
    /// Internal server error.
    pub const INTERNAL_ERROR: i32 = -32603;
}

// ---- MCP payload types ----------------------------------------------------

/// `initialize` result.
#[derive(Debug, Clone, Serialize)]
pub struct InitializeResult {
    /// Protocol version echoed back.
    #[serde(rename = "protocolVersion")]
    pub protocol_version: &'static str,
    /// Capability map.
    pub capabilities: Capabilities,
    /// Server identity.
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
}

/// Capability block returned from `initialize`.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Capabilities {
    /// Tools capability — empty object means "supported".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Value>,
    /// Resources capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<Value>,
    /// Prompts capability — enables `prompts/list` + `prompts/get`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Value>,
}

/// Server identity block.
#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    /// Implementation name.
    pub name: String,
    /// Implementation version (typically the crate version).
    pub version: String,
}

/// A single tool advertised in `tools/list`.
#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    /// Tool id (unique within the server).
    pub name: String,
    /// One-line human description.
    pub description: String,
    /// JSON Schema for the arguments.
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Response payload for `tools/call`.
#[derive(Debug, Clone, Serialize)]
pub struct CallToolResult {
    /// Content blocks (we only emit text blocks).
    pub content: Vec<ContentBlock>,
    /// Whether the call ended with an error. Default `false`.
    #[serde(rename = "isError")]
    pub is_error: bool,
}

/// A single content block inside a tool response.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentBlock {
    /// Plain text block.
    Text {
        /// The text payload.
        text: String,
    },
}

impl ContentBlock {
    /// Convenience constructor for a text block.
    pub fn text(t: impl Into<String>) -> Self {
        ContentBlock::Text { text: t.into() }
    }
}

/// An MCP resource descriptor.
#[derive(Debug, Clone, Serialize)]
pub struct Resource {
    /// URI (we use the `thoth://` scheme).
    pub uri: String,
    /// Human-readable name.
    pub name: String,
    /// Short description.
    pub description: String,
    /// Content type (e.g. `"text/markdown"`).
    #[serde(rename = "mimeType")]
    pub mime_type: String,
}

/// Inlined resource contents returned by `resources/read`.
#[derive(Debug, Clone, Serialize)]
pub struct ResourceContents {
    /// Echo of the requested URI.
    pub uri: String,
    /// Content type.
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    /// Inline text body.
    pub text: String,
}

/// Canonical output of every tool inside the server.
///
/// Two downstream wrappers consume this:
///
/// - The **MCP** `tools/call` handler projects it onto
///   [`CallToolResult`] by keeping only the rendered `text` (as a single
///   text content block). That's what Claude Code / Cowork / Cursor see,
///   because the MCP wire format is text-centric.
/// - The **Thoth** `thoth.call` handler (a private RPC extension used only
///   by the CLI thin-client over the Unix socket) returns the struct
///   verbatim so the CLI can look at `data` and honour `--json`,
///   pretty-print tables, etc.
///
/// Tools should populate both fields. `data` is the machine-readable
/// truth; `text` is a human fallback used when structured data is not
/// consumed (e.g. MCP clients, terminals without `--json`).
#[derive(Debug, Clone, Serialize)]
pub struct ToolOutput {
    /// Structured result. Shape is tool-specific; see each tool's docs.
    pub data: Value,
    /// Pretty-printed rendering of `data` for direct display. Usually a
    /// short multi-line string; never contains ANSI colour codes.
    pub text: String,
    /// `true` if the tool considered the call a failure. Both the MCP
    /// wrapper and the CLI use this to pick an error exit path.
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

impl ToolOutput {
    /// Successful output with both structured data and a rendered form.
    pub fn new(data: Value, text: impl Into<String>) -> Self {
        Self {
            data,
            text: text.into(),
            is_error: false,
        }
    }

    /// Shortcut for purely textual results (e.g. "remembered fact: ...").
    /// `data` falls back to `{ "message": <text> }` so CLI --json still
    /// produces something meaningful.
    pub fn message(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            data: serde_json::json!({ "message": text }),
            text,
            is_error: false,
        }
    }

    /// Error output. `data.error` holds the message; `text` is "error: …".
    pub fn error(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        Self {
            data: serde_json::json!({ "error": msg }),
            text: format!("error: {msg}"),
            is_error: true,
        }
    }
}

// ---- MCP prompts ----------------------------------------------------------

/// A single prompt descriptor advertised in `prompts/list`.
///
/// Prompts are templates (optionally parameterised) that MCP clients can
/// expand into their own context window. They're the canonical vehicle for
/// "ask Claude to self-reflect using this preset" — Thoth just assembles
/// the text, Claude (on the client side) runs it.
#[derive(Debug, Clone, Serialize)]
pub struct Prompt {
    /// Prompt id — unique within the server.
    pub name: String,
    /// One-line human description.
    pub description: String,
    /// Optional argument schema. Empty = no arguments.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<PromptArgument>,
}

/// Single named argument in a prompt template.
#[derive(Debug, Clone, Serialize)]
pub struct PromptArgument {
    /// Argument id.
    pub name: String,
    /// Description shown to the model picking values.
    pub description: String,
    /// Whether the argument must be supplied.
    pub required: bool,
}

/// Response payload for `prompts/get`.
///
/// Follows the MCP spec: a short description plus one-or-more
/// `PromptMessage` blocks that the client concatenates into its context.
#[derive(Debug, Clone, Serialize)]
pub struct GetPromptResult {
    /// Echo of the prompt description (lets clients cache metadata).
    pub description: String,
    /// Rendered conversation — typically a single `user` message carrying
    /// the fully substituted template.
    pub messages: Vec<PromptMessage>,
}

/// One message inside a rendered prompt.
#[derive(Debug, Clone, Serialize)]
pub struct PromptMessage {
    /// `"user"`, `"assistant"`, or `"system"`.
    pub role: String,
    /// Content block — we only ever emit text here.
    pub content: ContentBlock,
}
