//! Thin-client that forwards CLI requests to a running MCP daemon via
//! its Unix domain socket (`.thoth/mcp.sock`).
//!
//! When the MCP server is alive (spawned by Claude Code), `redb` holds an
//! exclusive file lock on `graph.redb`. Instead of fighting for the lock
//! the CLI connects to the socket and sends a JSON-RPC request — the MCP
//! server executes it against its already-open store and returns the
//! result. If the socket doesn't exist or the connection fails, the
//! caller should fall back to opening the store directly.
//!
//! We speak the Thoth-private `thoth.call` RPC (not MCP `tools/call`) so
//! the response carries the structured [`ToolOutput`] `data` half — that's
//! what `--json` and the CLI's pretty-printers need. The MCP wire format
//! is text-only and would force us to re-parse text on every command.
//!
//! # Guarantees
//!
//! - **Timeout-bounded**. Every request has a hard wall-clock deadline;
//!   a hung daemon can't freeze the CLI forever.
//! - **Unique request ids** via a process-wide `AtomicU64`. A single
//!   client that makes multiple calls never reuses an id (robust even
//!   though we don't currently multiplex — it costs nothing to be
//!   correct).

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Default per-request timeout. Indexing can be slow on cold caches; pick
/// something generous but not infinite. Callers that need more can use
/// [`DaemonClient::call_with_timeout`].
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Monotonically increasing request-id source. Process-wide (not
/// per-client) — makes logs easier to correlate and costs nothing.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A connected handle to the MCP daemon socket.
pub struct DaemonClient {
    stream: UnixStream,
}

impl DaemonClient {
    /// Try to connect to `<root>/mcp.sock`. Returns `None` if the socket
    /// doesn't exist or the connection is refused (daemon not running).
    pub async fn try_connect(root: &Path) -> Option<Self> {
        let sock = root.join("mcp.sock");
        let stream = UnixStream::connect(&sock).await.ok()?;
        Some(Self { stream })
    }

    /// Call a named MCP tool and return the raw `ToolOutput` JSON.
    ///
    /// The returned `Value` has shape:
    /// ```json
    /// { "data": <tool-specific>, "text": "<rendered>", "isError": false }
    /// ```
    ///
    /// Tool-level errors surface as `Ok(value)` with `isError: true`.
    /// Transport / protocol errors are returned as `Err`.
    pub async fn call(&mut self, tool: &str, arguments: Value) -> anyhow::Result<Value> {
        self.call_with_timeout(tool, arguments, DEFAULT_TIMEOUT)
            .await
    }

    /// Like [`Self::call`] but with a caller-specified timeout.
    pub async fn call_with_timeout(
        &mut self,
        tool: &str,
        arguments: Value,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "thoth.call",
            "params": {
                "name": tool,
                "arguments": arguments,
            }
        });

        let fut = self.roundtrip(&request);
        let resp: Value = match tokio::time::timeout(timeout, fut).await {
            Ok(inner) => inner?,
            Err(_) => anyhow::bail!("thoth-mcp daemon did not respond within {:?}", timeout),
        };

        // Check for JSON-RPC level error.
        if let Some(err) = resp.get("error") {
            let msg = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown daemon error");
            anyhow::bail!("{msg}");
        }

        let result = resp
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("daemon response missing `result`"))?;
        Ok(result)
    }

    /// Write one request line and read one response line. Separated so
    /// [`Self::call_with_timeout`] can wrap it in `tokio::time::timeout`.
    async fn roundtrip(&mut self, request: &Value) -> anyhow::Result<Value> {
        let mut line = serde_json::to_string(request)?;
        line.push('\n');

        let (reader, mut writer) = self.stream.split();
        writer.write_all(line.as_bytes()).await?;
        writer.flush().await?;

        let mut buf = String::new();
        let mut reader = BufReader::new(reader);
        let n = reader.read_line(&mut buf).await?;
        if n == 0 {
            anyhow::bail!("daemon closed the connection before replying");
        }

        Ok(serde_json::from_str(buf.trim())?)
    }
}

// ---- Convenience helpers on `Value` -------------------------------------

/// Extract the `text` field from a `ToolOutput` JSON value. Falls back to
/// an empty string so the CLI always has something to print.
pub fn tool_text(result: &Value) -> &str {
    result.get("text").and_then(|v| v.as_str()).unwrap_or("")
}

/// Extract the `data` field from a `ToolOutput` JSON value. Returns
/// `Value::Null` if absent.
pub fn tool_data(result: &Value) -> Value {
    result.get("data").cloned().unwrap_or(Value::Null)
}

/// Is the tool-level `isError` flag set?
pub fn tool_is_error(result: &Value) -> bool {
    result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}
