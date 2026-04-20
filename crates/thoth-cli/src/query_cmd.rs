//! `thoth query` and `thoth eval` — recall and evaluation subcommands.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use thoth_core::{Query, Synthesizer};
use thoth_retrieve::{RetrieveConfig, Retriever};
use thoth_store::StoreRoot;
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
                "thoth_recall",
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
    // pre-satisfies `thoth-gate`, matching the daemon path (which logs
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
    let output_cfg = thoth_retrieve::OutputConfig::load_or_default(root).await;
    print!("{}", out.render_with(&output_cfg.render_options()));
    Ok(())
}

// ------------------------------------------------------------------ eval types

/// Gold set file schema (TOML):
///
/// ```toml
/// [[query]]
/// q = "hybrid recall RRF fusion"
/// # any of: substring matches against the *path* of returned chunks…
/// expect_path  = ["retrieve/src/hybrid"]
/// # …or against the rendered preview/body.
/// expect_text  = ["reciprocal rank fusion"]
/// ```
#[derive(Debug, serde::Deserialize)]
pub struct GoldSet {
    pub query: Vec<GoldQuery>,
}

#[derive(Debug, serde::Deserialize)]
pub struct GoldQuery {
    pub q: String,
    #[serde(default)]
    pub expect_path: Vec<String>,
    #[serde(default)]
    pub expect_text: Vec<String>,
}

/// Per-query, per-mode measurement. `rank` is 1-indexed; `0` means no
/// chunk in the top-k matched any of the gold's `expect_*` clauses.
pub struct QueryRun {
    pub rank: usize,
    pub returned: usize,
    pub elapsed_us: u128,
}

impl QueryRun {
    pub fn hit(&self) -> bool {
        self.rank > 0
    }
}

/// Aggregate metrics for one mode over the whole gold set. Computed
/// lazily from the per-query `runs` so the raw data is also serializable.
pub struct ModeReport {
    pub label: &'static str,
    pub runs: Vec<QueryRun>,
}

impl ModeReport {
    pub fn hits(&self) -> usize {
        self.runs.iter().filter(|r| r.hit()).count()
    }

    pub fn precision_at_k(&self) -> f64 {
        if self.runs.is_empty() {
            0.0
        } else {
            self.hits() as f64 / self.runs.len() as f64
        }
    }

    /// Mean reciprocal rank. Misses contribute `0`, which matches the
    /// standard definition and makes MRR comparable across runs even
    /// when hit rates differ.
    pub fn mrr(&self) -> f64 {
        if self.runs.is_empty() {
            return 0.0;
        }
        let sum: f64 = self
            .runs
            .iter()
            .map(|r| if r.hit() { 1.0 / r.rank as f64 } else { 0.0 })
            .sum();
        sum / self.runs.len() as f64
    }

    /// `p` in `[0.0, 1.0]`. Uses the nearest-rank method (ceil), which
    /// is deterministic for small N and avoids the interpolation
    /// ambiguity in percentile definitions.
    pub fn latency_percentile_us(&self, p: f64) -> u128 {
        if self.runs.is_empty() {
            return 0;
        }
        let mut v: Vec<u128> = self.runs.iter().map(|r| r.elapsed_us).collect();
        v.sort_unstable();
        let idx = ((v.len() as f64 * p).ceil().max(1.0) as usize - 1).min(v.len() - 1);
        v[idx]
    }
}

/// 1-indexed rank of the first chunk that satisfies the gold clause, or
/// `0` if none do. Matching rules mirror the original implementation:
/// `expect_path` substring-matches the chunk path, `expect_text`
/// substring-matches `preview + body`; a chunk qualifies if *either*
/// non-empty bucket matches (any-of).
pub fn match_rank(gold: &GoldQuery, out: &thoth_core::Retrieval) -> usize {
    for (i, c) in out.chunks.iter().enumerate() {
        let p = c.path.to_string_lossy().to_lowercase();
        let body = format!("{} {}", c.preview, c.body).to_lowercase();
        let path_ok = gold
            .expect_path
            .iter()
            .any(|s| p.contains(&s.to_lowercase()));
        let text_ok = gold
            .expect_text
            .iter()
            .any(|s| body.contains(&s.to_lowercase()));
        if (!gold.expect_path.is_empty() && path_ok) || (!gold.expect_text.is_empty() && text_ok) {
            return i + 1;
        }
    }
    0
}

/// Which retrieval mode(s) `thoth eval` should exercise.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EvalMode {
    /// Lexical-only recall (BM25 + symbol + graph + markdown, RRF-fused).
    #[default]
    Zero,
    /// Full recall: also runs the vector stage and (if `--synth` is set) the
    /// synthesizer. Requires `--embedder` and/or `--synth`.
    Full,
    /// Run the gold set under both modes and print a side-by-side report.
    Both,
}

pub async fn run_eval(
    root: &Path,
    gold_path: &Path,
    top_k: usize,
    mode: EvalMode,
    synth_kind: Option<SynthKind>,
    json: bool,
) -> Result<()> {
    let raw = tokio::fs::read_to_string(gold_path).await?;
    let gold: GoldSet = toml::from_str(&raw)?;
    if gold.query.is_empty() {
        anyhow::bail!("gold set is empty");
    }

    let want_zero = matches!(mode, EvalMode::Zero | EvalMode::Both);
    let want_full = matches!(mode, EvalMode::Full | EvalMode::Both);

    if want_full && synth_kind.is_none() {
        anyhow::bail!(
            "--mode {} requires --synth (Mode::Full needs a synthesizer)",
            if mode == EvalMode::Full {
                "full"
            } else {
                "both"
            }
        );
    }

    // For pure-Zero runs we prefer the running daemon so `thoth eval` works
    // even when Claude Code is holding the redb lock. Any Full mode needs
    // direct store access (the daemon is Mode::Zero only), so if the daemon
    // is up we bail with a clear message instead of fighting for the lock.
    let daemon_for_zero = if want_zero && !want_full {
        crate::daemon::DaemonClient::try_connect(root).await
    } else {
        None
    };

    if want_full
        && crate::daemon::DaemonClient::try_connect(root)
            .await
            .is_some()
    {
        anyhow::bail!(
            "thoth-mcp daemon is running; stop it before running `thoth eval --mode {}` \
             (Mode::Full would fight for the redb exclusive lock)",
            if mode == EvalMode::Full {
                "full"
            } else {
                "both"
            }
        );
    }

    // Open the store once and clone `StoreRoot` into each retriever (it's
    // cheap — the inner handles are Arc'd).
    let store = if want_full || daemon_for_zero.is_none() {
        Some(StoreRoot::open(root).await?)
    } else {
        None
    };

    let retrieve_cfg = RetrieveConfig::load_or_default(root).await;
    let chroma = match store.as_ref() {
        Some(s) => open_chroma(s).await,
        None => None,
    };

    let zero_retriever = if want_zero && daemon_for_zero.is_none() {
        Some(
            Retriever::new(store.as_ref().unwrap().clone())
                .with_chroma(chroma.clone())
                .with_markdown_boost(retrieve_cfg.rerank_markdown_boost),
        )
    } else {
        None
    };

    let full_retriever: Option<Retriever> = if want_full {
        let synth: Option<Arc<dyn Synthesizer>> = build_synth(synth_kind)?;
        Some(
            Retriever::with_full(store.as_ref().unwrap().clone(), chroma.clone(), synth)
                .with_markdown_boost(retrieve_cfg.rerank_markdown_boost),
        )
    } else {
        None
    };

    let mut daemon_client = daemon_for_zero;
    let mut zero_runs: Vec<QueryRun> = Vec::new();
    let mut full_runs: Vec<QueryRun> = Vec::new();

    for gq in &gold.query {
        if want_zero {
            let (out, elapsed_us) = if let Some(c) = daemon_client.as_mut() {
                let start = Instant::now();
                let result = c
                    .call(
                        "thoth_recall",
                        serde_json::json!({ "query": gq.q, "top_k": top_k }),
                    )
                    .await?;
                let elapsed_us = start.elapsed().as_micros();
                if crate::daemon::tool_is_error(&result) {
                    anyhow::bail!("{}", crate::daemon::tool_text(&result));
                }
                let out: thoth_core::Retrieval =
                    serde_json::from_value(crate::daemon::tool_data(&result))?;
                (out, elapsed_us)
            } else {
                let r = zero_retriever.as_ref().unwrap();
                let start = Instant::now();
                let out = r
                    .recall(&Query {
                        text: gq.q.clone(),
                        top_k,
                        ..Query::text("")
                    })
                    .await?;
                (out, start.elapsed().as_micros())
            };
            zero_runs.push(QueryRun {
                rank: match_rank(gq, &out),
                returned: out.chunks.len(),
                elapsed_us,
            });
        }

        if want_full {
            let r = full_retriever.as_ref().unwrap();
            let start = Instant::now();
            let out = r
                .recall_full(&Query {
                    text: gq.q.clone(),
                    top_k,
                    ..Query::text("")
                })
                .await?;
            let elapsed_us = start.elapsed().as_micros();
            full_runs.push(QueryRun {
                rank: match_rank(gq, &out),
                returned: out.chunks.len(),
                elapsed_us,
            });
        }
    }

    let mut reports: Vec<ModeReport> = Vec::new();
    if want_zero {
        reports.push(ModeReport {
            label: "zero",
            runs: zero_runs,
        });
    }
    if want_full {
        reports.push(ModeReport {
            label: "full",
            runs: full_runs,
        });
    }

    if json {
        let modes_json: Vec<_> = reports
            .iter()
            .map(|rep| {
                serde_json::json!({
                    "mode": rep.label,
                    "total": rep.runs.len(),
                    "hits": rep.hits(),
                    "precision_at_k": rep.precision_at_k(),
                    "mrr": rep.mrr(),
                    "latency_us": {
                        "p50": rep.latency_percentile_us(0.50),
                        "p95": rep.latency_percentile_us(0.95),
                    },
                    "queries": gold.query.iter().zip(rep.runs.iter()).map(|(gq, r)| {
                        serde_json::json!({
                            "q": gq.q,
                            "hit": r.hit(),
                            "rank": r.rank,
                            "returned": r.returned,
                            "elapsed_us": r.elapsed_us,
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
        let v = serde_json::json!({
            "top_k": top_k,
            "modes": modes_json,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        // Per-query line. In both-mode we prefix with `Z:` / `F:` marks so
        // wins/losses are visually aligned; in single-mode we drop the
        // prefix to stay close to the original terse format.
        let show_prefix = want_zero && want_full;
        let zero_runs = reports.iter().find(|r| r.label == "zero");
        let full_runs = reports.iter().find(|r| r.label == "full");
        for (i, gq) in gold.query.iter().enumerate() {
            let parts: Vec<String> = [("Z", zero_runs), ("F", full_runs)]
                .iter()
                .filter_map(|(tag, rep)| rep.map(|rep| (*tag, &rep.runs[i])))
                .map(|(tag, run)| {
                    let mark = if run.hit() {
                        format!("✓@{}", run.rank)
                    } else {
                        "✗   ".to_string()
                    };
                    if show_prefix {
                        format!("{tag}:{mark}")
                    } else {
                        format!("{mark}  [{:>2}]", run.returned)
                    }
                })
                .collect();
            println!("{}  {}", parts.join(" "), gq.q);
        }
        println!();
        for rep in &reports {
            println!(
                "[{label}] P@{top_k}={hits}/{total}={p:.3}  MRR={mrr:.3}  \
                 latency p50={p50}µs p95={p95}µs",
                label = rep.label,
                hits = rep.hits(),
                total = rep.runs.len(),
                p = rep.precision_at_k(),
                mrr = rep.mrr(),
                p50 = rep.latency_percentile_us(0.50),
                p95 = rep.latency_percentile_us(0.95),
            );
        }
    }

    // Non-zero exit if any active mode missed a query, so CI can gate on
    // eval regressions just like before — now across whichever mode(s)
    // were requested.
    let any_miss = reports.iter().any(|rep| rep.runs.iter().any(|r| !r.hit()));
    if any_miss {
        std::process::exit(1);
    }
    Ok(())
}
