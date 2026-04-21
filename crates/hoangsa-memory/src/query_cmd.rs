//! `hoangsa-memory query` — recall subcommand.

use std::path::Path;

use anyhow::Result;
use hoangsa_memory_core::Query;
use hoangsa_memory_retrieve::{RetrieveConfig, Retriever};
use hoangsa_memory_store::StoreRoot;
use tracing::warn;

use crate::{SynthKind, build_synth, open_chroma};

pub async fn run_query(
    root: &Path,
    text: String,
    top_k: usize,
    json: bool,
    synth_kind: Option<SynthKind>,
) -> Result<()> {
    let wants_full = synth_kind.is_some();
    if !wants_full && let Some(mut d) = crate::daemon::DaemonClient::try_connect(root).await {
        let result = d
            .call(
                "memory_recall",
                serde_json::json!({ "query": text, "top_k": top_k }),
            )
            .await?;
        if crate::daemon::tool_is_error(&result) {
            anyhow::bail!("{}", crate::daemon::tool_text(&result));
        }
        if json {
            // The daemon's `data` is a full `Retrieval` — same shape
            // as the direct path below.
            println!(
                "{}",
                serde_json::to_string_pretty(&crate::daemon::tool_data(&result))?
            );
        } else {
            println!("{}", crate::daemon::tool_text(&result));
        }
        return Ok(());
    }

    let store = StoreRoot::open(root).await?;
    // Keep a handle to the episode log before `store` is moved into the
    // retriever — we use it below to log `QueryIssued` so the CLI query
    // pre-satisfies `hoangsa-cli enforce`, matching the daemon path (which logs
    // implicitly because MCP's `tool_recall` defaults `log_event: true`).
    let episodes = store.episodes.clone();

    let synth = build_synth(synth_kind)?;
    let is_full = synth.is_some();

    let chroma = open_chroma(&store).await;

    let retrieve_cfg = RetrieveConfig::load_or_default(root).await;
    let r = if is_full {
        Retriever::with_full(store, chroma, synth)
            .with_markdown_boost(retrieve_cfg.rerank_markdown_boost)
    } else {
        Retriever::new(store)
            .with_chroma(chroma)
            .with_markdown_boost(retrieve_cfg.rerank_markdown_boost)
    };

    let q = Query {
        text: text.clone(),
        top_k,
        ..Query::text("")
    };
    let out = if is_full {
        r.recall_full(&q).await?
    } else {
        r.recall(&q).await?
    };

    // Best-effort: a missing log entry would defeat the gate, but a broken
    // log shouldn't block the user from seeing their results.
    if let Err(e) = episodes.log_query_issued(text).await {
        warn!(error = %e, "failed to log QueryIssued event");
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    // Honour `[output]` in `<root>/config.toml` for body + total-size
    // caps. The daemon path above already got this treatment on the
    // server side, so both routes print the same capped text.
    let output_cfg = hoangsa_memory_retrieve::OutputConfig::load_or_default(root).await;
    print!("{}", out.render_with(&output_cfg.render_options()));
    Ok(())
}

