//! A second `hoangsa-memory-mcp` on the same project must relay to the
//! first, not die on redb's exclusive lock.
//!
//! Two Claude Code sessions on one repo spawn two stdio servers. The
//! loser used to exit immediately with "Database already open. Cannot
//! acquire lock.", the client would respawn it, and the resulting crash
//! loop looked like a runaway CPU hog. These tests spawn the real binary
//! twice — the failure only exists across processes, so an in-process
//! test cannot see it.

use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};

const BIN: &str = env!("CARGO_BIN_EXE_hoangsa-memory-mcp");

fn spawn_instance(root: &std::path::Path) -> Child {
    Command::new(BIN)
        .env("HOANGSA_MEMORY_ROOT", root)
        .env("RUST_LOG", "info")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("binary spawns")
}

/// Wait until `sock` accepts connections. 100 × 50 ms = 5 s worst case —
/// generous because the first instance opens redb/tantivy on startup.
async fn wait_for_socket(sock: &std::path::Path) -> bool {
    for _ in 0..100 {
        if UnixStream::connect(sock).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// Send one JSON-RPC request on a child's stdin and read one line back.
async fn roundtrip(child: &mut Child, request: Value) -> Option<Value> {
    let mut line = serde_json::to_string(&request).expect("serializable");
    line.push('\n');
    let stdin = child.stdin.as_mut().expect("stdin piped");
    stdin.write_all(line.as_bytes()).await.ok()?;
    stdin.flush().await.ok()?;

    let stdout = child.stdout.as_mut().expect("stdout piped");
    let mut reader = BufReader::new(stdout);
    let mut buf = String::new();
    match tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut buf)).await {
        Ok(Ok(n)) if n > 0 => serde_json::from_str(buf.trim()).ok(),
        _ => None,
    }
}

fn initialize(id: i32) -> Value {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "relay-test", "version": "1" }
        }
    })
}

#[tokio::test]
async fn second_instance_relays_instead_of_dying_on_the_store_lock() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let mut first = spawn_instance(root);
    let sock = root.join("mcp.sock");
    assert!(
        wait_for_socket(&sock).await,
        "first instance never bound {}",
        sock.display()
    );

    // The first instance answers on its own stdio.
    let resp = roundtrip(&mut first, initialize(1)).await;
    assert_eq!(
        resp.as_ref()
            .and_then(|r| r["result"]["serverInfo"]["name"].as_str()),
        Some("hoangsa-memory-mcp"),
        "first instance did not answer initialize: {resp:?}"
    );

    // The second instance must serve the same request by relaying — this
    // is the request that used to get nothing because the process was
    // already dead.
    let mut second = spawn_instance(root);
    let resp = roundtrip(&mut second, initialize(2)).await;
    assert_eq!(
        resp.as_ref()
            .and_then(|r| r["result"]["serverInfo"]["name"].as_str()),
        Some("hoangsa-memory-mcp"),
        "second instance did not relay initialize: {resp:?}"
    );
    assert_eq!(
        resp.as_ref().map(|r| r["id"].clone()),
        Some(json!(2)),
        "relayed response carried the wrong id: {resp:?}"
    );

    // And it is still alive, rather than having exited with the lock error.
    assert!(
        second.try_wait().expect("try_wait").is_none(),
        "second instance exited instead of relaying"
    );

    let _ = second.kill().await;
    let _ = first.kill().await;
}

#[tokio::test]
async fn relayed_instance_does_not_unlink_the_owner_socket() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let mut first = spawn_instance(root);
    let sock = root.join("mcp.sock");
    assert!(wait_for_socket(&sock).await, "first instance never bound");

    // Relay, then end the relay by closing its stdin (EOF = normal exit).
    let mut second = spawn_instance(root);
    assert!(
        roundtrip(&mut second, initialize(1)).await.is_some(),
        "second instance did not relay"
    );
    drop(second.stdin.take());
    let _ = tokio::time::timeout(Duration::from_secs(10), second.wait()).await;

    // The owner's socket must survive — the relay does not own it.
    assert!(
        UnixStream::connect(&sock).await.is_ok(),
        "relay removed the owner's socket on exit"
    );
    assert!(
        roundtrip(&mut first, initialize(2)).await.is_some(),
        "owner stopped answering after the relay exited"
    );

    let _ = first.kill().await;
}

/// When the owner dies the relay has nothing left to serve — it must exit,
/// not linger. Its stdin read is parked in a blocking-pool thread that
/// runtime shutdown joins forever, so returning normally left a live
/// process answering nothing until its client happened to write a byte.
#[tokio::test]
async fn relay_exits_when_the_owner_dies() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let mut first = spawn_instance(root);
    let sock = root.join("mcp.sock");
    assert!(wait_for_socket(&sock).await, "first instance never bound");

    let mut second = spawn_instance(root);
    assert!(
        roundtrip(&mut second, initialize(1)).await.is_some(),
        "second instance did not relay"
    );

    first.kill().await.expect("kill owner");
    let _ = first.wait().await;

    let status = tokio::time::timeout(Duration::from_secs(10), second.wait())
        .await
        .expect("relay must exit once the owner is gone, not hang")
        .expect("wait relay");
    assert!(
        status.success(),
        "relay should exit cleanly, got {status:?}"
    );
}
