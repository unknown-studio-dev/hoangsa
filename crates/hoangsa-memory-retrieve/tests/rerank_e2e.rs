//! End-to-end for the LLM rerank pass, with `/bin/sh` standing in for the
//! model so the real subprocess + parse + reorder path runs without an LLM.

use std::path::Path;

use hoangsa_memory_core::{Chunk, RetrievalSource};
use hoangsa_memory_retrieve::config::RerankConfig;
use hoangsa_memory_retrieve::rerank::rerank_chunks;

fn chunk(path: &str) -> Chunk {
    Chunk {
        id: path.to_string(),
        path: path.into(),
        line: 1,
        span: (1, 2),
        symbol: Some(path.to_string()),
        preview: String::new(),
        body: format!("body of {path}"),
        score: 1.0,
        source: RetrievalSource::FullText,
        context: None,
    }
}

/// A stub that prints `stdout` regardless of the prompt it is handed.
fn cfg_emitting(stdout: &str) -> RerankConfig {
    RerankConfig {
        enabled: true,
        candidates: 24,
        timeout_secs: 10,
        command: vec![
            "/bin/sh".into(),
            "-c".into(),
            format!("printf '%s' {}", shell_quote(stdout)),
        ],
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn names(chunks: &[Chunk]) -> Vec<String> {
    chunks.iter().map(|c| c.id.clone()).collect()
}

#[tokio::test]
async fn model_ordering_is_applied() {
    let mut chunks = vec![chunk("a.rs"), chunk("b.rs"), chunk("c.rs")];
    rerank_chunks(&cfg_emitting("[2,0,1]"), Path::new("."), "q", &mut chunks).await;
    assert_eq!(names(&chunks), vec!["c.rs", "a.rs", "b.rs"]);
}

/// Every failure mode must return the fused order untouched — recall is on
/// the agent's hot path and a reranker that can break it is worse than none.
#[tokio::test]
async fn every_failure_mode_keeps_the_fused_order() {
    let original = vec![chunk("a.rs"), chunk("b.rs"), chunk("c.rs")];

    let failures: Vec<(&str, RerankConfig)> = vec![
        (
            "prose instead of JSON",
            cfg_emitting("I think a.rs is best"),
        ),
        ("empty output", cfg_emitting("")),
        (
            "non-zero exit",
            RerankConfig {
                enabled: true,
                candidates: 24,
                timeout_secs: 10,
                command: vec!["/bin/sh".into(), "-c".into(), "exit 7".into()],
            },
        ),
        (
            "binary does not exist",
            RerankConfig {
                enabled: true,
                candidates: 24,
                timeout_secs: 10,
                command: vec!["/nonexistent/hoangsa-no-such-bin".into()],
            },
        ),
        (
            "timeout",
            RerankConfig {
                enabled: true,
                candidates: 24,
                timeout_secs: 1,
                command: vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()],
            },
        ),
    ];

    for (label, cfg) in failures {
        let mut chunks = original.clone();
        rerank_chunks(&cfg, Path::new("."), "q", &mut chunks).await;
        assert_eq!(
            names(&chunks),
            names(&original),
            "{label}: order must be unchanged"
        );
    }
}

/// The model may only reorder. It cannot add, drop, or duplicate a result —
/// so a lazy or hostile answer cannot shrink what recall returns.
#[tokio::test]
async fn the_result_set_is_never_changed() {
    let original = vec![chunk("a.rs"), chunk("b.rs"), chunk("c.rs"), chunk("d.rs")];
    for answer in ["[0]", "[3,3,3]", "[99,100]", "[]", "[2,1,0,3]"] {
        let mut chunks = original.clone();
        rerank_chunks(&cfg_emitting(answer), Path::new("."), "q", &mut chunks).await;
        let mut got = names(&chunks);
        let mut want = names(&original);
        got.sort();
        want.sort();
        assert_eq!(got, want, "answer {answer} changed the set");
        assert_eq!(
            chunks.len(),
            original.len(),
            "answer {answer} changed the length"
        );
    }
}

/// Disabled is a hard no-op: no subprocess, no reordering.
#[tokio::test]
async fn disabled_does_not_run_the_model() {
    let mut chunks = vec![chunk("a.rs"), chunk("b.rs")];
    let cfg = RerankConfig {
        enabled: false,
        // Would fail loudly if it ever ran.
        command: vec!["/nonexistent/should-never-run".into()],
        ..Default::default()
    };
    rerank_chunks(&cfg, Path::new("."), "q", &mut chunks).await;
    assert_eq!(names(&chunks), vec!["a.rs", "b.rs"]);
}
