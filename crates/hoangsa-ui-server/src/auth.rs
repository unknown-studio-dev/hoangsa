use axum::{
    body::Body,
    extract::{Query, State},
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};
use rand::RngCore;
use serde::Deserialize;
use std::sync::Arc;

use crate::state::AppState;

/// 32 bytes of CSPRNG → 64-char hex. Shared across browser tabs for the
/// lifetime of the server process; expires when the user Ctrl-Cs the CLI.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[derive(Deserialize)]
pub struct TokenQuery {
    t: Option<String>,
}

/// Reject any `/api/*` request whose `?t=` query param doesn't match the
/// server's launch token. Browser SPA reads the token from its own URL on
/// load and forwards it on each fetch — no cookies.
pub async fn csrf_guard(
    State(state): State<Arc<AppState>>,
    Query(q): Query<TokenQuery>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let supplied = q.t.as_deref().unwrap_or("");
    if supplied != state.token {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(next.run(req).await)
}
