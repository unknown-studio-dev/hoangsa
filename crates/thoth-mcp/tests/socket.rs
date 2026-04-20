//! Integration tests for the Unix-socket sidecar transport (the "daemon"
//! the CLI thin-client talks to).
//!
//! These spin up a real [`Server`], start [`run_socket`] in a background
//! task, and exercise the socket with a hand-written JSON-RPC client
//! (deliberately not the CLI's `DaemonClient` — we want to pin down the
//! wire format independently of the consumer).

use std::time::Duration;

use serde_json::{Value, json};
use thoth_mcp::{Server, run_socket, socket_path};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Spin up a server + socket listener rooted in `tmp`. Returns the
/// server handle (cloneable), socket path, and a `JoinHandle` holding
/// the listener task. Dropping the handle aborts the listener.
async fn spawn_daemon(
    tmp: &tempfile::TempDir,
) -> (Server, std::path::PathBuf, tokio::task::JoinHandle<()>) {
    let server = Server::open(tmp.path()).await.expect("server opens");
    let sock = socket_path(tmp.path());
    let listener_server = server.clone();
    let handle = tokio::spawn(async move {
        // Intentionally ignore the shutdown error (abort on drop).
        let _ = run_socket(listener_server).await;
    });

    // Wait for the listener to bind. 50 * 20ms = 1s worst case.
    for _ in 0..50 {
        if UnixStream::connect(&sock).await.is_ok() {
            return (server, sock, handle);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("socket never came up at {}", sock.display());
}

/// Send one JSON-RPC request and read one line back.
async fn roundtrip(sock: &std::path::Path, request: Value) -> Value {
    let stream = UnixStream::connect(sock).await.expect("connect");
    let (reader, mut writer) = stream.into_split();
    let mut line = serde_json::to_string(&request).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();

    let mut reader = BufReader::new(reader);
    let mut buf = String::new();
    reader.read_line(&mut buf).await.unwrap();
    serde_json::from_str(buf.trim()).unwrap()
}

// ===========================================================================
// `thoth.call` — the Thoth-private structured RPC
// ===========================================================================

#[tokio::test]
async fn thoth_call_returns_structured_tool_output() {
    let tmp = tempfile::tempdir().unwrap();
    let (_server, sock, _guard) = spawn_daemon(&tmp).await;

    // remember_fact writes to MEMORY.md and returns a structured `data`
    // block with text/tags/path.
    let resp = roundtrip(
        &sock,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "thoth.call",
            "params": {
                "name": "thoth_remember_fact",
                "arguments": {
                    "text": "sockets close after one response",
                    "tags": ["mcp", "transport"],
                }
            }
        }),
    )
    .await;

    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert!(resp.get("error").is_none(), "unexpected error: {resp:?}");

    // The result IS the ToolOutput — no `content` wrapper.
    let result = &resp["result"];
    assert_eq!(result["isError"], false);
    // In the default (auto) discipline config the fact commits straight to
    // MEMORY.md; in review mode it would say "staged (review mode)". Both
    // carry the fact text.
    assert!(
        result["text"]
            .as_str()
            .unwrap()
            .contains("committed to MEMORY.md"),
        "got text: {}",
        result["text"]
    );
    assert_eq!(result["data"]["text"], "sockets close after one response");
    assert_eq!(result["data"]["tags"], json!(["mcp", "transport"]));
    assert_eq!(result["data"]["staged"], false);
    assert!(
        result["data"]["path"]
            .as_str()
            .unwrap()
            .ends_with("MEMORY.md")
    );
}

#[tokio::test]
async fn thoth_call_unknown_tool_surfaces_method_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let (_server, sock, _guard) = spawn_daemon(&tmp).await;

    let resp = roundtrip(
        &sock,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "thoth.call",
            "params": { "name": "no_such_tool" }
        }),
    )
    .await;

    // Unknown tool is a JSON-RPC-level error, not a tool-level `isError`.
    assert!(resp.get("result").is_none());
    assert_eq!(resp["error"]["code"], -32601);
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown tool"),
        "got: {:?}",
        resp["error"]
    );
}

// ===========================================================================
// `tools/call` — the text-only MCP wire format
// ===========================================================================

#[tokio::test]
async fn tools_call_strips_structured_data_keeps_text() {
    let tmp = tempfile::tempdir().unwrap();
    let (_server, sock, _guard) = spawn_daemon(&tmp).await;

    // Seed MEMORY.md.
    tokio::fs::write(tmp.path().join("MEMORY.md"), "### fact\nhello\n")
        .await
        .unwrap();

    let resp = roundtrip(
        &sock,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "thoth_memory_show" }
        }),
    )
    .await;

    let result = &resp["result"];
    // MCP-shaped: content[].text, no top-level `data`.
    assert!(result.get("data").is_none());
    assert_eq!(result["isError"], false);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("hello"), "text was: {text}");
}

// ===========================================================================
// Transport & robustness
// ===========================================================================

#[tokio::test]
async fn empty_lines_between_requests_are_tolerated() {
    let tmp = tempfile::tempdir().unwrap();
    let (_server, sock, _guard) = spawn_daemon(&tmp).await;

    let stream = UnixStream::connect(&sock).await.unwrap();
    let (reader, mut writer) = stream.into_split();

    // Write a blank line, then a real request, on the same connection.
    writer.write_all(b"\n").await.unwrap();
    let req = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "ping",
        "params": {}
    });
    let mut line = serde_json::to_string(&req).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();

    let mut reader = BufReader::new(reader);
    let mut buf = String::new();
    // We should get exactly one response (for the ping).
    reader.read_line(&mut buf).await.unwrap();
    let resp: Value = serde_json::from_str(buf.trim()).unwrap();
    assert_eq!(resp["id"], 4);
    assert!(resp["result"].is_object());
}

#[tokio::test]
async fn stale_socket_file_is_reclaimed() {
    let tmp = tempfile::tempdir().unwrap();
    let sock = socket_path(tmp.path());

    // Create a stale file (not a bound socket) at the expected path. A
    // real daemon crash leaves the inode behind; we emulate that here.
    std::fs::write(&sock, b"stale").unwrap();
    assert!(sock.exists());

    // Starting a fresh daemon should silently clean up and bind.
    let (_server, _sock, _guard) = spawn_daemon(&tmp).await;
    // If we got here without panicking, the bind succeeded. Make one
    // real call just to be sure it's actually responsive.
    let resp = roundtrip(
        &sock,
        json!({"jsonrpc":"2.0","id":5,"method":"ping","params":{}}),
    )
    .await;
    assert_eq!(resp["id"], 5);
}

#[tokio::test]
async fn second_daemon_refuses_to_steal_the_socket() {
    let tmp = tempfile::tempdir().unwrap();
    // Reuse the *same* Server handle for the second bind attempt.
    // Opening a second `Server` on the same path would fail on the redb
    // exclusive lock before `run_socket` even got a turn, which is a
    // different failure mode than what we're testing here.
    let (server, _sock, _guard1) = spawn_daemon(&tmp).await;

    // Try to start a second listener on the same socket. It should get
    // `AddrInUse`, probe the peer, find it responsive, and bail — *not*
    // unlink the socket out from under the running daemon.
    let res = run_socket(server.clone()).await;
    assert!(res.is_err(), "second daemon should not have bound");
    let msg = format!("{}", res.unwrap_err());
    assert!(msg.contains("already listening"), "unexpected error: {msg}");
}
