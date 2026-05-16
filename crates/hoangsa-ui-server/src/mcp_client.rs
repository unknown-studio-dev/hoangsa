//! Thin JSON-RPC client over the per-project `mcp.sock` UnixStream.
//!
//! The daemon (`hoangsa-memory-mcp`) speaks ndjson on its Unix socket:
//! one JSON-RPC 2.0 envelope per line, response on the next line. This
//! module wraps a single round-trip — connect, optionally `initialize`,
//! send one request, read one response, drop the connection. No pool:
//! local AF_UNIX is sub-millisecond and the daemon itself multiplexes
//! incoming connections.
//!
//! UI handlers use [`call_memory_tool`] to invoke any memory tool through
//! the daemon's private `hoangsa-memory.call` extension, which returns the
//! structured [`ToolOutput`](hoangsa_memory_mcp::proto::ToolOutput) shape
//! (`data` + `text` + `isError`) rather than the text-only MCP envelope.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;

/// Time budget for a single tool call. Recall on a warm daemon is
/// typically <100 ms; cold-bootstrap (open redb + tantivy) can take
/// ~1-2 s for the first request of a freshly opened project. Anything
/// past 10 s means the daemon is wedged — return a 504 to the UI rather
/// than hold the request open indefinitely.
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum McpError {
    /// `mcp.sock` is absent or `connect()` returned ECONNREFUSED. The
    /// UI maps this to a 503 + "daemon unreachable" banner.
    #[error("daemon unreachable: {0}")]
    Unreachable(String),
    /// Connected but the round-trip blew the deadline.
    #[error("daemon timed out after {0:?}")]
    Timeout(Duration),
    /// Connected but the daemon sent malformed JSON or no response at all.
    #[error("bad daemon response: {0}")]
    BadResponse(String),
    /// Daemon replied with a JSON-RPC `error` envelope.
    #[error("rpc error {code}: {message}")]
    RpcError { code: i32, message: String },
    /// Local I/O failure on the socket.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Open a connection, send one JSON-RPC request, read the response.
///
/// The daemon handles `initialize` lazily on the dispatcher side — we
/// don't need to send a separate handshake before `hoangsa-memory.call`,
/// which is the only method this client exercises.
async fn rpc_call(socket_path: &Path, method: &str, params: Value) -> Result<Value, McpError> {
    let stream = match UnixStream::connect(socket_path).await {
        Ok(s) => s,
        Err(e) => return Err(McpError::Unreachable(e.to_string())),
    };
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let mut payload = serde_json::to_vec(&req)
        .map_err(|e| McpError::BadResponse(format!("encode request: {e}")))?;
    payload.push(b'\n');

    let work = async {
        writer.write_all(&payload).await?;
        writer.flush().await?;
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(McpError::BadResponse("daemon closed socket".into()));
        }
        let resp: Value = serde_json::from_str(line.trim())
            .map_err(|e| McpError::BadResponse(format!("parse response: {e}")))?;
        if let Some(err) = resp.get("error") {
            let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(-32603) as i32;
            let message = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("(no message)")
                .to_string();
            return Err(McpError::RpcError { code, message });
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    };

    match timeout(RPC_TIMEOUT, work).await {
        Ok(res) => res,
        Err(_) => Err(McpError::Timeout(RPC_TIMEOUT)),
    }
}

/// Invoke a memory tool through the daemon's `hoangsa-memory.call`
/// extension. Returns the raw [`ToolOutput`] shape — i.e. `{data, text,
/// isError}` — so handlers can forward `data` to the UI verbatim.
///
/// The tool's own error path (e.g. `memory_remove` not finding a match)
/// arrives as `isError: true` inside the result, *not* as an `RpcError`.
/// `RpcError` is reserved for protocol-level failures (unknown method,
/// invalid params).
pub async fn call_memory_tool(
    socket_path: &Path,
    tool_name: &str,
    arguments: Value,
) -> Result<Value, McpError> {
    rpc_call(
        socket_path,
        "hoangsa-memory.call",
        json!({ "name": tool_name, "arguments": arguments }),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn missing_socket_is_unreachable() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("missing.sock");
        let err = call_memory_tool(&sock, "memory_show", json!({}))
            .await
            .expect_err("must fail when socket absent");
        assert!(matches!(err, McpError::Unreachable(_)), "got {err:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn round_trip_against_fake_daemon() {
        // Spin up a one-shot listener that echoes a canned result, then
        // exercise call_memory_tool end-to-end.
        let dir = tempdir().unwrap();
        let sock = dir.path().join("mcp.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (r, mut w) = stream.into_split();
            let mut reader = BufReader::new(r);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(req["method"], "hoangsa-memory.call");
            assert_eq!(req["params"]["name"], "memory_show");
            let resp = json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "result": {
                    "data": { "memory_md": "hello", "lessons_md": null, "user_md": null },
                    "text": "rendered",
                    "isError": false
                }
            });
            let mut bytes = serde_json::to_vec(&resp).unwrap();
            bytes.push(b'\n');
            w.write_all(&bytes).await.unwrap();
            w.flush().await.unwrap();
        });

        let out = call_memory_tool(&sock, "memory_show", json!({})).await.unwrap();
        assert_eq!(out["data"]["memory_md"], "hello");
        assert_eq!(out["isError"], false);
        server.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rpc_error_surfaces_as_typed_variant() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("mcp.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (r, mut w) = stream.into_split();
            let mut reader = BufReader::new(r);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            let resp = json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "error": { "code": -32601, "message": "method not found: bogus" }
            });
            let mut bytes = serde_json::to_vec(&resp).unwrap();
            bytes.push(b'\n');
            w.write_all(&bytes).await.unwrap();
        });

        let err = call_memory_tool(&sock, "bogus", json!({}))
            .await
            .expect_err("must fail");
        match err {
            McpError::RpcError { code, message } => {
                assert_eq!(code, -32601);
                assert!(message.contains("method not found"));
            }
            other => panic!("expected RpcError, got {other:?}"),
        }
    }
}
