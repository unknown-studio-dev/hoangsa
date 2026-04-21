//! `thoth` daemon-facing helpers — shared logic for commands that prefer
//! the running MCP daemon and fall back to in-process dispatch.
//!
//! This module re-exports the shared `call_mcp_tool` and `emit_output`
//! helpers extracted from `main.rs` so subcommand modules can import them
//! from a single place without circular dependencies.

use std::path::Path;

use anyhow::Result;

/// Invoke an MCP tool over the daemon socket when one is running; else
/// spin up an in-process server and dispatch through it. Returns
/// `(text, data, is_error)` so the caller can pick which to surface
/// based on `--json`.
pub async fn call_mcp_tool(
    root: &Path,
    tool: &str,
    arguments: serde_json::Value,
) -> Result<(String, serde_json::Value, bool)> {
    if let Some(mut d) = crate::daemon::DaemonClient::try_connect(root).await {
        let result = d.call(tool, arguments).await?;
        let is_error = crate::daemon::tool_is_error(&result);
        let text = crate::daemon::tool_text(&result).to_string();
        let data = crate::daemon::tool_data(&result);
        return Ok((text, data, is_error));
    }
    // In-process: reuse the Server so we don't duplicate tool bodies.
    // This also means the CLI and daemon paths share test coverage.
    let server = thoth_mcp::Server::open(root).await?;
    let params = serde_json::json!({
        "name": tool,
        "arguments": arguments,
    });
    let msg = thoth_mcp::proto::RpcIncoming {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::Value::Number(1.into())),
        method: "hoangsa-memory.call".to_string(),
        params,
    };
    let resp = server.handle(msg).await;
    let Some(response) = resp else {
        anyhow::bail!("server returned no response for {tool}");
    };
    if let Some(err) = response.error {
        anyhow::bail!("{}: {}", err.code, err.message);
    }
    // `ToolOutput` is Serialize-only (the server never deserialises its
    // own output), so pull fields from the raw JSON instead of
    // round-tripping through from_value.
    let result = response.result.unwrap_or_default();
    let text = result
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let is_error = result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let data = result.get("data").cloned().unwrap_or_default();
    Ok((text, data, is_error))
}

// ------------------------------------------- graph / diff CLI subcommands

/// `thoth impact <fqn>` — forwards to the `memory_impact` MCP tool.
///
/// The daemon path is preferred (keeps us working when Claude Code is
/// holding the redb lock); if unavailable we fall back to opening the
/// store directly and calling the graph API in-process. Exit code is
/// non-zero when the graph can't find the symbol, so shell pipelines
/// can gate on missing FQNs.
pub async fn cmd_impact(
    root: &Path,
    fqn: &str,
    direction: &str,
    depth: usize,
    json: bool,
) -> Result<()> {
    let args = serde_json::json!({
        "fqn": fqn,
        "direction": direction,
        "depth": depth,
    });
    let (text, data, is_error) = call_mcp_tool(root, "memory_impact", args).await?;
    emit_output(text, data, is_error, json)
}

/// `thoth context <fqn>` — forwards to the `memory_symbol_context` tool.
pub async fn cmd_context(root: &Path, fqn: &str, limit: usize, json: bool) -> Result<()> {
    let args = serde_json::json!({ "fqn": fqn, "limit": limit });
    let (text, data, is_error) = call_mcp_tool(root, "memory_symbol_context", args).await?;
    emit_output(text, data, is_error, json)
}

/// `thoth changes` — feed a unified diff through the `memory_detect_changes`
/// tool. Diff source order of preference: `--from <file>` > `--from -`
/// (stdin) > `git diff HEAD`.
pub async fn cmd_changes(root: &Path, from: Option<&str>, depth: usize, json: bool) -> Result<()> {
    let diff = match from {
        Some("-") => {
            use tokio::io::AsyncReadExt;
            let mut buf = String::new();
            tokio::io::stdin().read_to_string(&mut buf).await?;
            buf
        }
        Some(path) => tokio::fs::read_to_string(path).await?,
        None => {
            // Default: diff of the current working tree against HEAD.
            // `git diff HEAD` includes both staged and unstaged changes,
            // which matches the "what am I about to commit?" intuition.
            let output = tokio::process::Command::new("git")
                .args(["diff", "HEAD"])
                .output()
                .await
                .map_err(|e| anyhow::anyhow!("failed to run git diff: {e}"))?;
            if !output.status.success() {
                anyhow::bail!(
                    "`git diff HEAD` exited non-zero: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            String::from_utf8(output.stdout)
                .map_err(|e| anyhow::anyhow!("git diff output not UTF-8: {e}"))?
        }
    };
    if diff.trim().is_empty() {
        println!("(no diff — working tree matches HEAD)");
        return Ok(());
    }
    let args = serde_json::json!({ "diff": diff, "depth": depth });
    let (text, data, is_error) = call_mcp_tool(root, "memory_detect_changes", args).await?;
    emit_output(text, data, is_error, json)
}

/// Emit either the rendered text or a pretty-printed JSON dump of the
/// structured `data` half. When `is_error` is set the process exits
/// non-zero so shell pipelines can gate on missing FQNs / malformed
/// diffs.
pub fn emit_output(
    text: String,
    data: serde_json::Value,
    is_error: bool,
    json: bool,
) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&data)?);
    } else if !text.is_empty() {
        print!("{text}");
        if !text.ends_with('\n') {
            println!();
        }
    }
    if is_error {
        std::process::exit(1);
    }
    Ok(())
}
