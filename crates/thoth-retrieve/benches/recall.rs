//! Criterion benchmark for `thoth_retrieve::recall` (Mode::Zero).
//!
//! Measures p50 / p95 latency against the baseline of 83 ms / 117 ms
//! (REQ-08). Run with:
//!
//! ```text
//! cargo bench -p thoth-retrieve -- recall
//! ```
//!
//! Criterion prints a full histogram including p50 and p95 automatically.

use criterion::{Criterion, criterion_group, criterion_main};
use tempfile::tempdir;
use thoth_core::{Mode, Query};
use thoth_parse::LanguageRegistry;
use thoth_retrieve::{Indexer, recall};
use thoth_store::StoreRoot;

// ---------------------------------------------------------------------------
// Representative source snippets (5 synthetic Rust files).
// ---------------------------------------------------------------------------

const SNIPPET_AUTH: &str = r#"
use std::collections::HashMap;

/// Verifies a JWT token signed with RS256.
pub fn verify_token(jwt: &str) -> bool {
    !jwt.is_empty()
}

/// Signs a payload with RS256 and returns a compact JWT.
pub fn sign_token(claims: &str) -> String {
    format!("jwt::{claims}")
}

/// Decodes a token without verifying the signature.
pub fn decode_token(jwt: &str) -> Option<String> {
    jwt.split('.').nth(1).map(|s| s.to_string())
}
"#;

const SNIPPET_USERS: &str = r#"
/// A registered user in the system.
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
}

impl User {
    pub fn new(id: u64, name: String, email: String) -> Self {
        Self { id, name, email }
    }

    /// Returns true when the user's name is non-empty.
    pub fn is_valid(&self) -> bool {
        !self.name.is_empty()
    }
}
"#;

const SNIPPET_STORE: &str = r#"
use std::collections::HashMap;

/// Simple in-memory key-value store.
pub struct KvStore {
    data: HashMap<String, Vec<u8>>,
}

impl KvStore {
    pub fn new() -> Self {
        Self { data: HashMap::new() }
    }

    pub fn put(&mut self, key: &str, value: Vec<u8>) {
        self.data.insert(key.to_string(), value);
    }

    pub fn get(&self, key: &str) -> Option<&Vec<u8>> {
        self.data.get(key)
    }

    pub fn delete(&mut self, key: &str) -> bool {
        self.data.remove(key).is_some()
    }
}
"#;

const SNIPPET_INDEX: &str = r#"
/// Builds a full-text search index over a corpus of documents.
pub struct FtsIndex {
    terms: std::collections::HashMap<String, Vec<usize>>,
}

impl FtsIndex {
    pub fn new() -> Self {
        Self { terms: std::collections::HashMap::new() }
    }

    /// Indexes a document at the given position.
    pub fn insert(&mut self, doc: &str, pos: usize) {
        for token in doc.split_whitespace() {
            self.terms.entry(token.to_lowercase()).or_default().push(pos);
        }
    }

    /// Returns document positions matching the query term.
    pub fn search(&self, term: &str) -> &[usize] {
        self.terms.get(term).map(|v| v.as_slice()).unwrap_or(&[])
    }
}
"#;

const SNIPPET_GRAPH: &str = r#"
use std::collections::{HashMap, HashSet};

/// Directed call graph for symbol impact analysis.
pub struct CallGraph {
    edges: HashMap<String, HashSet<String>>,
}

impl CallGraph {
    pub fn new() -> Self {
        Self { edges: HashMap::new() }
    }

    /// Records that `caller` calls `callee`.
    pub fn add_edge(&mut self, caller: &str, callee: &str) {
        self.edges
            .entry(caller.to_string())
            .or_default()
            .insert(callee.to_string());
    }

    /// Returns direct callees of `symbol`.
    pub fn callees(&self, symbol: &str) -> Vec<&str> {
        self.edges
            .get(symbol)
            .map(|s| s.iter().map(|c| c.as_str()).collect())
            .unwrap_or_default()
    }
}
"#;

// ---------------------------------------------------------------------------
// Benchmark definition
// ---------------------------------------------------------------------------

fn recall_benchmark(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    // --- one-time setup: build a temp store and index the snippets ---
    let src_dir = tempdir().unwrap();
    let thoth_dir = tempdir().unwrap();

    rt.block_on(async {
        let files = [
            ("auth.rs", SNIPPET_AUTH),
            ("users.rs", SNIPPET_USERS),
            ("store.rs", SNIPPET_STORE),
            ("index.rs", SNIPPET_INDEX),
            ("graph.rs", SNIPPET_GRAPH),
        ];
        for (name, content) in &files {
            tokio::fs::write(src_dir.path().join(name), content)
                .await
                .unwrap();
        }

        let store = StoreRoot::open(thoth_dir.path()).await.unwrap();
        let indexer = Indexer::new(store, LanguageRegistry::new());
        indexer.index_path(src_dir.path()).await.unwrap();
    });

    // Open the store once; StoreRoot is Arc-backed so clone() is cheap.
    let store = rt.block_on(StoreRoot::open(thoth_dir.path())).unwrap();

    // --- benchmark group ---
    let mut group = c.benchmark_group("recall_latency");
    // 50 samples gives criterion enough data for stable p50/p95 estimates
    // without burning excessive wall-clock time in CI.
    group.sample_size(50);

    group.bench_function("recall_p50_p95", |b| {
        b.to_async(&rt).iter(|| {
            let store = store.clone();
            async move {
                let q = Query::text("symbol retrieval query");
                recall(store, q, Mode::Zero).await.unwrap()
            }
        });
    });

    group.finish();

    // Keep temp dirs alive until after bench completes.
    drop(src_dir);
    drop(thoth_dir);
}

criterion_group!(benches, recall_benchmark);
criterion_main!(benches);
