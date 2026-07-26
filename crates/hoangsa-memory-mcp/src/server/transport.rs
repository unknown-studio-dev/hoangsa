//! Transports: stdio (newline-delimited JSON-RPC) and the Unix-socket
//! sidecar used by the CLI thin-client.

use std::path::Path;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::debug;

use crate::proto::{RpcError, RpcIncoming, RpcResponse, error_codes};

use super::Server;

// ===========================================================================
// Stdio transport
// ===========================================================================

/// Run the server on stdin/stdout until EOF or ctrl-c.
///
/// Each JSON-RPC message is expected on its own line. Responses are emitted
/// as newline-terminated JSON on stdout; all logging goes to stderr via
/// `tracing`.
pub async fn run_stdio(server: Server) -> anyhow::Result<()> {
    let mut reader = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        let n = tokio::select! {
            res = reader.read_line(&mut line) => res?,
            _ = tokio::signal::ctrl_c() => {
                debug!("ctrl-c; shutting down mcp");
                0
            }
        };
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<RpcIncoming>(trimmed) {
            Ok(msg) => server.handle(msg).await,
            Err(e) => Some(RpcResponse::err(
                Value::Null,
                RpcError::new(error_codes::PARSE_ERROR, format!("parse error: {e}")),
            )),
        };

        if let Some(resp) = response {
            let text = serde_json::to_string(&resp)?;
            stdout.write_all(text.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

/// Canonical path for the Unix domain socket that the CLI connects to.
pub fn socket_path(root: &Path) -> std::path::PathBuf {
    root.join("mcp.sock")
}

// ===========================================================================
// Stdio → socket relay (second instance on the same project)
// ===========================================================================

/// True when `sock` has a live listener behind it.
///
/// A leftover socket *file* from a killed daemon refuses connections, so
/// `connect()` succeeding is the same liveness signal [`run_socket`] uses
/// before deciding another daemon owns the project.
pub async fn daemon_alive(sock: &Path) -> bool {
    tokio::net::UnixStream::connect(sock).await.is_ok()
}

/// Poll [`daemon_alive`] until it answers or `attempts` × `interval` elapse.
///
/// Used to settle the startup race: when two instances launch together,
/// the loser's redb open fails a few milliseconds before the winner has
/// finished binding its socket.
pub async fn wait_for_daemon(
    sock: &Path,
    attempts: u32,
    interval: std::time::Duration,
) -> bool {
    for _ in 0..attempts {
        if daemon_alive(sock).await {
            return true;
        }
        tokio::time::sleep(interval).await;
    }
    false
}

/// True when `err` is redb refusing a second exclusive lock on the store.
///
/// The message is carried as a string inside
/// `hoangsa_memory_core::Error::Store`, so this has to match on text —
/// redb offers no distinguishable typed variant by the time it reaches us.
pub fn is_store_lock_conflict(err: &anyhow::Error) -> bool {
    let text = format!("{err:#}");
    text.contains("Cannot acquire lock") || text.contains("Database already open")
}

/// Why [`run_stdio_proxy`] stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum RelayEnd {
    /// Our client closed stdin (or ctrl-c) — an ordinary shutdown.
    StdinClosed,
    /// The owning daemon dropped the connection. Our stdin read is still
    /// parked in a blocking-pool thread that runtime shutdown would join
    /// forever, so the caller must exit the process rather than return.
    OwnerGone,
}

/// Relay this process's stdio to a daemon that already owns the project.
///
/// Both transports speak the same newline-delimited JSON-RPC, so this is a
/// line pump in each direction — no parsing, which keeps notifications
/// (requests with no response) correct for free.
///
/// Returns when stdin closes, ctrl-c arrives, or the daemon drops the
/// connection. The caller must NOT unlink the socket afterwards: it
/// belongs to the other process.
pub async fn run_stdio_proxy(sock: &Path) -> anyhow::Result<RelayEnd> {
    let stream = tokio::net::UnixStream::connect(sock).await?;
    debug!(path = %sock.display(), "relaying stdio to existing daemon");
    let (sock_reader, mut sock_writer) = stream.into_split();

    // Daemon → stdout.
    let down = tokio::spawn(async move {
        let mut reader = BufReader::new(sock_reader);
        let mut stdout = tokio::io::stdout();
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if stdout.write_all(line.as_bytes()).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });

    // Stdin → daemon. `down` is consumed by the select, so it is held in an
    // Option: polling a completed JoinHandle again panics.
    let mut down = Some(down);
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut line = String::new();
    loop {
        line.clear();
        let n = match down.as_mut() {
            Some(handle) => tokio::select! {
                res = stdin.read_line(&mut line) => res?,
                _ = tokio::signal::ctrl_c() => 0,
                _ = handle => return Ok(RelayEnd::OwnerGone),
            },
            None => tokio::select! {
                res = stdin.read_line(&mut line) => res?,
                _ = tokio::signal::ctrl_c() => 0,
            },
        };
        if n == 0 {
            break;
        }
        sock_writer.write_all(line.as_bytes()).await?;
        sock_writer.flush().await?;
    }
    if let Some(handle) = down {
        handle.abort();
    }
    Ok(RelayEnd::StdinClosed)
}

/// Run a Unix-socket sidecar alongside the stdio transport.
///
/// Binds `.hoangsa/memory/mcp.sock` and accepts connections in a loop. Each
/// connection is a short-lived JSON-RPC session (one line in → one line
/// out, then close). The socket is removed on clean shutdown.
///
/// This is the "thin-client" entry point: when the CLI detects the socket
/// it forwards requests here instead of opening the store directly,
/// avoiding the redb exclusive-lock conflict.
pub async fn run_socket(server: Server) -> anyhow::Result<()> {
    use tokio::net::{UnixListener, UnixStream};

    let sock = socket_path(&server.inner.root);

    // Try binding first. Only if it fails with `AddrInUse` do we probe
    // the existing socket and, if nothing is listening, unlink and retry.
    // This avoids the race where two daemons start at the same time, and
    // the "remove stale and rebind" pattern of the previous version would
    // happily overwrite an actively-used socket.
    let listener = match UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            // Peer responsive? Then another daemon owns the socket — bail.
            if UnixStream::connect(&sock).await.is_ok() {
                return Err(anyhow::anyhow!(
                    "another hoangsa-memory-mcp is already listening on {}",
                    sock.display()
                ));
            }
            // Stale socket file — safe to remove and retry.
            let _ = std::fs::remove_file(&sock);
            UnixListener::bind(&sock)?
        }
        Err(e) => return Err(e.into()),
    };
    debug!(path = %sock.display(), "mcp socket listening");

    loop {
        let (stream, _) = listener.accept().await?;
        let server = server.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_socket_conn(server, stream).await {
                debug!(error = %e, "socket connection error");
            }
        });
    }
}

/// Handle one Unix-socket connection: read lines, dispatch, respond.
///
/// **There is deliberately no idle timeout here.** An earlier version closed
/// any connection silent for 5 minutes, on the stated premise that "per-tool
/// RPCs land within seconds" for a real MCP client. That premise is false:
/// a relayed client (see [`run_stdio_proxy`]) forwards only what its own
/// client sends, and Claude Code sends no keep-alive on this path — during a
/// long agent turn it can legitimately go many minutes between tool calls.
/// The timer reaped those live sessions, and the MCP server "randomly"
/// disconnected mid-task.
///
/// The timer also never did the job it was added for. A dead peer is already
/// reaped by the `n == 0` arm below: when the relay process exits, its end of
/// the `AF_UNIX` pair closes and `read_line` returns EOF. The only connection
/// a timer could collect is one that is alive, connected, and merely quiet —
/// exactly the one that must be kept. Do not re-add it.
pub(crate) async fn handle_socket_conn(
    server: Server,
    stream: tokio::net::UnixStream,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<RpcIncoming>(trimmed) {
            Ok(msg) => server.handle(msg).await,
            Err(e) => Some(RpcResponse::err(
                Value::Null,
                RpcError::new(error_codes::PARSE_ERROR, format!("parse error: {e}")),
            )),
        };

        if let Some(resp) = response {
            let text = serde_json::to_string(&resp)?;
            writer.write_all(text.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::handle_socket_conn;
    use crate::Server;

    async fn open_server() -> (Server, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let srv = Server::open(tmp.path()).await.expect("Server::open");
        (srv, tmp)
    }

    /// One `initialize` request, so the test does not depend on any tool.
    const PING: &str =
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;

    /// A quiet connection must survive an arbitrarily long gap between
    /// requests. Time is paused, so the ten virtual minutes cost nothing —
    /// and any idle timer re-added to `handle_socket_conn` would fire during
    /// the `advance` and make the second request go unanswered.
    #[tokio::test(start_paused = true)]
    async fn idle_socket_connection_is_not_closed() {
        let (srv, _tmp) = open_server().await;
        let (client, server_end) = tokio::net::UnixStream::pair().expect("socketpair");
        let task = tokio::spawn(handle_socket_conn(srv, server_end));

        let (rx, mut tx) = client.into_split();
        let mut rx = BufReader::new(rx);
        let mut line = String::new();

        tx.write_all(format!("{PING}\n").as_bytes()).await.expect("write 1");
        rx.read_line(&mut line).await.expect("read 1");
        assert!(!line.is_empty(), "first request answered");

        // Ten minutes of silence — twice the 5 min timer this test exists
        // to keep out.
        tokio::time::advance(std::time::Duration::from_secs(600)).await;

        line.clear();
        tx.write_all(format!("{PING}\n").as_bytes()).await.expect("write 2");
        rx.read_line(&mut line).await.expect("read 2 — connection was closed while idle");
        assert!(
            line.contains("\"id\":1"),
            "second request answered on the same connection: {line}"
        );

        drop(tx);
        task.await.expect("join").expect("handler ok");
    }

    /// The mechanism that makes the missing timer safe: a peer that goes away
    /// yields EOF, so the handler returns on its own.
    #[tokio::test]
    async fn socket_connection_ends_when_peer_drops() {
        let (srv, _tmp) = open_server().await;
        let (client, server_end) = tokio::net::UnixStream::pair().expect("socketpair");
        let task = tokio::spawn(handle_socket_conn(srv, server_end));

        drop(client);

        let ended = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("handler must return on peer drop, not hang");
        ended.expect("join").expect("handler ok");
    }
}
