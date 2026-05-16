use std::net::{SocketAddr, TcpListener};

/// Bind a TcpListener to 127.0.0.1 on an OS-assigned free port.
///
/// We hand the bound listener (not just the port) back to the caller because
/// returning a port creates a TOCTOU race — another process can grab the port
/// between our probe and the server's actual bind. Axum's `serve` accepts a
/// `TcpListener` directly, so this avoids the gap.
pub fn bind_loopback() -> std::io::Result<TcpListener> {
    let addr: SocketAddr = "127.0.0.1:0".parse().expect("static addr parses");
    let listener = TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}
