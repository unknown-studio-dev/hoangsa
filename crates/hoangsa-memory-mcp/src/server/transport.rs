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

/// Idle ceiling on a single socket connection. A client that opens the
/// socket and goes silent (no read, no close) would otherwise pin an
/// `Arc<Server>` clone — and through it, defer eviction of any data the
/// dispatch chain might touch — for the daemon's lifetime. 5 min is well
/// above the cadence of any real MCP client (Claude Code keep-alive +
/// per-tool RPCs land within seconds) while trimming zombie connections
/// in bounded time.
const SOCKET_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Handle one Unix-socket connection: read lines, dispatch, respond.
pub(crate) async fn handle_socket_conn(
    server: Server,
    stream: tokio::net::UnixStream,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let n = match tokio::time::timeout(
            SOCKET_IDLE_TIMEOUT,
            reader.read_line(&mut line),
        )
        .await
        {
            Ok(res) => res?,
            Err(_) => {
                debug!(
                    idle_secs = SOCKET_IDLE_TIMEOUT.as_secs(),
                    "socket connection idle; closing"
                );
                break;
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
            writer.write_all(text.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
    }
    Ok(())
}
