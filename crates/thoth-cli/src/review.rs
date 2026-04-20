//! Background review runner.
//!
//! Orchestrates the full background review lifecycle:
//!
//! 1. Build context from on-disk state (episodes, gate log, memory).
//! 2. Enrich with `git diff --stat`.
//! 3. Select backend: `claude` CLI (subscription) or Anthropic API.
//! 4. Send a single prompt → parse structured JSON response.
//! 5. Persist facts/lessons/skills via [`background_review::persist_review`].
//! 6. Bump the `.last-review` watermark.

use std::path::Path;

use anyhow::{Context, bail};
use thoth_memory::background_review::{
    ReviewReport, build_review_context, parse_review_response, persist_review, render_prompt,
};

/// Run the background review end-to-end.
///
/// `backend` is one of `"auto"`, `"cli"`, or `"api"`. On `"auto"` the
/// function checks `ANTHROPIC_API_KEY` and falls back to `claude` CLI.
/// `model` is passed through to the backend (e.g. `claude-haiku-4-5`);
/// empty string means "let the backend pick its default".
pub async fn run_review(root: &Path, backend: &str, model: &str) -> anyhow::Result<ReviewReport> {
    // 1. Build context.
    let mut ctx = build_review_context(root)
        .await
        .context("failed to build review context")?;

    // 2. Enrich with git diff --stat (best-effort).
    ctx.git_stat = git_diff_stat().await.unwrap_or_default();

    // 3. Render prompt.
    let prompt = render_prompt(&ctx);

    // 4. Call LLM.
    let response = call_backend(&prompt, backend, model).await?;

    // 5. Parse response.
    let output = parse_review_response(&response).map_err(|e| anyhow::anyhow!("{e}"))?;

    // 6. Persist.
    let report = persist_review(root, output)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // 7. Bump watermark.
    if let Err(e) = thoth_memory::mark_last_review(root).await {
        tracing::warn!(error = %e, "background-review: failed to bump watermark");
    }

    Ok(report)
}

// ------------------------------------------------------------------ backend

/// Dispatch a single prompt through the resolved backend. Re-used by
/// `thoth review` and `thoth compact` so both paths honour the same
/// backend/model config (and the same `--model` override semantics).
pub async fn call_backend(prompt: &str, backend: &str, model: &str) -> anyhow::Result<String> {
    match resolve_backend(backend) {
        #[cfg(feature = "anthropic")]
        Backend::Api(key) => call_api(prompt, &key, model).await,
        Backend::Cli => call_cli(prompt, model).await,
    }
}

enum Backend {
    Cli,
    #[cfg(feature = "anthropic")]
    Api(String),
}

fn resolve_backend(requested: &str) -> Backend {
    match requested {
        #[cfg(feature = "anthropic")]
        "api" => match std::env::var("ANTHROPIC_API_KEY") {
            Ok(k) if !k.is_empty() => Backend::Api(k),
            _ => {
                tracing::warn!(
                    "background-review: api backend requested but ANTHROPIC_API_KEY not set, falling back to cli"
                );
                Backend::Cli
            }
        },
        #[cfg(feature = "anthropic")]
        "auto" => match std::env::var("ANTHROPIC_API_KEY") {
            Ok(k) if !k.is_empty() => Backend::Api(k),
            _ => Backend::Cli,
        },
        _ => Backend::Cli,
    }
}

// --------------------------------------------------------- backend: claude CLI

async fn call_cli(prompt: &str, model: &str) -> anyhow::Result<String> {
    use tokio::io::AsyncWriteExt;

    // Pipe the prompt via stdin instead of a CLI arg — prompts can
    // exceed the OS argument-length limit (macOS ARG_MAX ≈ 256 KiB).
    // `--dangerously-skip-permissions` skips interactive permission
    // prompts and the project's PreToolUse hooks, so the review
    // doesn't recurse into Thoth's own discipline loop.
    //
    // `--model` is critical: without it the subprocess inherits the
    // user's current session default (often Opus), so every review
    // burns Opus tokens for a task that Haiku handles fine.
    let mut cmd = tokio::process::Command::new("claude");
    cmd.args([
        "--print",
        "--output-format",
        "text",
        "--dangerously-skip-permissions",
    ]);
    if !model.is_empty() {
        cmd.args(["--model", model]);
    }
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn `claude` CLI — is it installed and in PATH?")?;

    // Write prompt to stdin, then close it so claude starts processing.
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes()).await?;
        // drop closes the pipe
    }

    let output = child
        .wait_with_output()
        .await
        .context("claude CLI process failed")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("claude CLI exited with {}: {stderr}", output.status);
    }

    String::from_utf8(output.stdout).context("claude CLI output is not valid UTF-8")
}

// ------------------------------------------------------- backend: Anthropic API

#[cfg(feature = "anthropic")]
async fn call_api(prompt: &str, api_key: &str, model: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::new();
    // Empty model string → fall back to the Haiku snapshot that
    // matches [`DisciplineConfig::default`].
    let model = if model.is_empty() {
        "claude-haiku-4-5"
    } else {
        model
    };
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1024,
        "messages": [
            { "role": "user", "content": prompt }
        ]
    });

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .context("Anthropic API request failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        bail!("Anthropic API returned {status}: {text}");
    }

    let json: serde_json::Value = resp.json().await.context("failed to parse API response")?;

    json.get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|block| block.get("text"))
        .and_then(|t| t.as_str())
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("unexpected API response shape"))
}

// ---------------------------------------------------------- cmd_review handler

/// `thoth review` CLI handler — delegates to `run_review` with config fallback.
pub async fn cmd_review(root: &Path, backend: &str, model: &str) -> anyhow::Result<()> {
    if !root.exists() {
        println!("(no .thoth/ at {} — nothing to review)", root.display());
        return Ok(());
    }
    // Flag overrides, else fall back to config values so the hook-spawned
    // and user-invoked paths agree on model/backend.
    let disc = thoth_memory::DisciplineConfig::load_or_default(root).await;
    let backend = if backend.is_empty() {
        disc.background_review_backend.as_str()
    } else {
        backend
    };
    let model = if model.is_empty() {
        disc.background_review_model.as_str()
    } else {
        model
    };
    match run_review(root, backend, model).await {
        Ok(report) => {
            let total = report.facts_added + report.lessons_added + report.skills_proposed;
            if total > 0 {
                eprintln!(
                    "thoth: background review added {} facts, {} lessons, {} skill proposals",
                    report.facts_added, report.lessons_added, report.skills_proposed,
                );
            } else {
                eprintln!("thoth: background review — nothing worth saving");
            }
        }
        Err(e) => eprintln!("thoth: background review failed: {e}"),
    }
    Ok(())
}

// ------------------------------------------------------------------ helpers

async fn git_diff_stat() -> anyhow::Result<String> {
    let output = tokio::process::Command::new("git")
        .args(["diff", "--stat", "HEAD"])
        .output()
        .await?;

    if !output.status.success() {
        return Ok(String::new());
    }

    String::from_utf8(output.stdout).context("git output not UTF-8")
}
