//! End-to-end test: index a small project → run hybrid recall → sanity-check
//! that the chunks we expect come back.

use tempfile::{TempDir, tempdir};
use thoth_core::Query;
use thoth_parse::LanguageRegistry;
use thoth_retrieve::{Indexer, Retriever};
use thoth_store::StoreRoot;

/// Open a fresh in-memory-equivalent store backed by a temp directory.
/// Returns the `TempDir` guard (must stay alive) and the `StoreRoot`.
async fn make_store() -> (TempDir, StoreRoot) {
    let dir = tempdir().unwrap();
    let store = StoreRoot::open(dir.path()).await.unwrap();
    (dir, store)
}

/// Build an `Indexer` for `store` using the default language registry.
fn make_indexer(store: StoreRoot) -> Indexer {
    Indexer::new(store, LanguageRegistry::new())
}

const AUTH_RS: &str = r#"
use std::collections::HashMap;

/// Verifies a JWT token signed with RS256.
pub fn verify_token(jwt: &str) -> bool {
    // stub
    !jwt.is_empty()
}

/// Signs a token with RS256.
pub fn sign_token(claims: &str) -> String {
    format!("jwt::{claims}")
}
"#;

const USERS_RS: &str = r#"
pub struct User {
    pub id: u64,
    pub name: String,
}

impl User {
    pub fn new(id: u64, name: String) -> Self {
        Self { id, name }
    }
}
"#;

#[tokio::test]
async fn index_and_recall_returns_relevant_chunks() {
    // Arrange: a tiny source tree.
    let src_dir = tempdir().unwrap();
    tokio::fs::write(src_dir.path().join("auth.rs"), AUTH_RS)
        .await
        .unwrap();
    tokio::fs::write(src_dir.path().join("users.rs"), USERS_RS)
        .await
        .unwrap();

    // Open a `.thoth/` alongside.
    let (_thoth_dir, store) = make_store().await;

    // Index.
    let idx = make_indexer(store.clone());
    let stats = idx.index_path(src_dir.path()).await.unwrap();
    assert!(stats.files >= 2, "indexed files: {stats:?}");
    assert!(stats.chunks >= 4, "indexed chunks: {stats:?}");
    assert!(stats.symbols >= 3, "indexed symbols: {stats:?}");

    // Recall.
    let r = Retriever::new(store);
    let q = Query::text("verify jwt token");
    let out = r.recall(&q).await.unwrap();

    assert!(!out.chunks.is_empty(), "no chunks returned");
    // The top hit should be from auth.rs — either the function symbol or the
    // BM25 body match.
    let top = &out.chunks[0];
    assert!(
        top.path.ends_with("auth.rs"),
        "top chunk should be from auth.rs: {top:?}"
    );
    // And at least one chunk in the result should reference verify_token.
    assert!(
        out.chunks.iter().any(|c| c
            .symbol
            .as_deref()
            .map(|s| s.contains("verify_token"))
            .unwrap_or(false)
            || c.body.contains("verify_token")),
        "verify_token missing from {:?}",
        out.chunks
            .iter()
            .map(|c| c.symbol.clone().unwrap_or_default())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn recall_enriches_top_chunks_with_graph_context() {
    // A small source tree where `caller` calls `callee` in the same
    // file. After indexing, a recall for `caller` should come back with
    // `callee` surfaced under `context.callees` and a docstring lifted
    // from the `///` block above the function.
    let src = r#"
/// Do the heavy lifting.
///
/// This is the second paragraph.
pub fn do_heavy_lifting(x: i32) -> i32 {
    helper(x) + 1
}

/// Internal helper.
pub fn helper(x: i32) -> i32 {
    x * 2
}
"#;
    let src_dir = tempdir().unwrap();
    tokio::fs::write(src_dir.path().join("work.rs"), src)
        .await
        .unwrap();

    let (_thoth_dir, store) = make_store().await;
    make_indexer(store.clone())
        .index_path(src_dir.path())
        .await
        .unwrap();

    let r = Retriever::new(store);
    let out = r.recall(&Query::text("do_heavy_lifting")).await.unwrap();

    // Find the chunk whose symbol is do_heavy_lifting.
    let heavy = out
        .chunks
        .iter()
        .find(|c| {
            c.symbol
                .as_deref()
                .is_some_and(|s| s.contains("do_heavy_lifting"))
        })
        .expect("do_heavy_lifting chunk present");

    let ctx = heavy
        .context
        .as_ref()
        .expect("graph context populated on top chunks");

    // Docstring lifted from the `///` block.
    let doc = ctx.doc.as_deref().unwrap_or("");
    assert!(
        doc.contains("Do the heavy lifting."),
        "doc should contain the first line: {doc:?}"
    );

    // `helper` should appear as a callee or a sibling (depending on
    // whether the parser produced a Calls edge — both are acceptable
    // signals).
    let referenced = ctx
        .callees
        .iter()
        .chain(ctx.siblings.iter())
        .any(|s| s.fqn.contains("helper"));
    assert!(
        referenced,
        "helper should be surfaced as callee or sibling; got callees={:?} siblings={:?}",
        ctx.callees, ctx.siblings
    );
}

#[tokio::test]
async fn retrieval_render_surfaces_enriched_sections() {
    // Whatever the graph produces, the rendered text should at least
    // include the chunk header and full body — and when a docstring is
    // present, its first line.
    let src = r#"
/// Handle login.
pub fn login() {}
"#;
    let src_dir = tempdir().unwrap();
    tokio::fs::write(src_dir.path().join("auth.rs"), src)
        .await
        .unwrap();

    let (_thoth_dir, store) = make_store().await;
    make_indexer(store.clone())
        .index_path(src_dir.path())
        .await
        .unwrap();

    let r = Retriever::new(store);
    let out = r.recall(&Query::text("login")).await.unwrap();
    let rendered = out.render();

    assert!(rendered.contains("login"), "rendered: {rendered}");
    assert!(rendered.contains("auth.rs"), "rendered: {rendered}");
    // Docstring gutter marker from `Chunk::render_into`.
    assert!(
        rendered.contains("Handle login."),
        "doc line missing from: {rendered}"
    );
}

#[tokio::test]
async fn recall_fuses_markdown_memory_hits() {
    let (thoth_dir, store) = make_store().await;

    // Seed MEMORY.md directly.
    tokio::fs::write(
        thoth_dir.path().join("MEMORY.md"),
        "\n### auth uses JWT with RS256\nSigning keys live in Vault.\ntags: auth, security\n",
    )
    .await
    .unwrap();

    let r = Retriever::new(store);
    let out = r.recall(&Query::text("auth jwt vault")).await.unwrap();

    assert!(
        out.chunks.iter().any(|c| c.path.ends_with("MEMORY.md")),
        "expected a markdown-sourced chunk: {:?}",
        out.chunks
    );
}

#[tokio::test]
async fn markdown_boost_lifts_lesson_score_above_code() {
    // Without a boost, a code chunk whose symbol literally matches the
    // query will out-rank a lesson that only matches on prose. This
    // verifies the knob does what it says: Markdown hits scale, Code
    // hits don't, and boost=2.0 produces a strictly higher Markdown
    // score than boost=1.0 on the same input.
    let src = r#"
/// Handles database migration runs.
pub fn migration_runner() -> &'static str {
    "runs"
}
"#;
    let src_dir = tempdir().unwrap();
    tokio::fs::write(src_dir.path().join("migrations.rs"), src)
        .await
        .unwrap();

    let (thoth_dir, store) = make_store().await;
    make_indexer(store.clone())
        .index_path(src_dir.path())
        .await
        .unwrap();

    tokio::fs::write(
        thoth_dir.path().join("LESSONS.md"),
        "# LESSONS.md\n\
         \n\
         ### when editing database migrations\n\
         Always run `sqlx prepare` after changing SQL — skipping breaks CI.\n\
         \n",
    )
    .await
    .unwrap();

    let query = Query::text("database migrations sqlx prepare");

    let baseline = Retriever::new(store.clone()).recall(&query).await.unwrap();
    let boosted = Retriever::new(store)
        .with_markdown_boost(2.0)
        .recall(&query)
        .await
        .unwrap();

    let baseline_md = baseline
        .chunks
        .iter()
        .find(|c| c.path.ends_with("LESSONS.md"))
        .expect("baseline must still surface the lesson");
    let boosted_md = boosted
        .chunks
        .iter()
        .find(|c| c.path.ends_with("LESSONS.md"))
        .expect("boosted must still surface the lesson");
    assert!(
        boosted_md.score > baseline_md.score,
        "boost should lift markdown score: baseline={} boosted={}",
        baseline_md.score,
        boosted_md.score,
    );

    // Code hits must be untouched — only Markdown scales.
    let baseline_code = baseline
        .chunks
        .iter()
        .find(|c| c.path.ends_with("migrations.rs"));
    let boosted_code = boosted
        .chunks
        .iter()
        .find(|c| c.path.ends_with("migrations.rs"));
    if let (Some(b), Some(bo)) = (baseline_code, boosted_code) {
        assert!(
            (b.score - bo.score).abs() < 1e-6,
            "code score must not change: baseline={} boosted={}",
            b.score,
            bo.score
        );
    }
}

#[tokio::test]
async fn recall_surfaces_lessons_by_trigger() {
    // Reflective memory (LESSONS.md) must be surfaced by recall, not just
    // MEMORY.md. Without this, `thoth_lesson_outcome` bumps counters in a
    // file nobody reads — the learn-from-mistakes loop stays open.
    let (thoth_dir, store) = make_store().await;

    tokio::fs::write(
        thoth_dir.path().join("LESSONS.md"),
        "# LESSONS.md\n\
         \n\
         ### when editing sqlx migrations\n\
         Always run `sqlx prepare` after changing SQL. Skipping this breaks CI.\n\
         \n",
    )
    .await
    .unwrap();

    let r = Retriever::new(store);
    // Query contains tokens from both the trigger ("sqlx", "migrations")
    // and the advice ("prepare"). Any of them should fire the lesson.
    let out = r
        .recall(&Query::text("sqlx migrations prepare"))
        .await
        .unwrap();

    let lesson_hit = out
        .chunks
        .iter()
        .find(|c| c.path.ends_with("LESSONS.md"))
        .unwrap_or_else(|| panic!("expected a LESSONS.md chunk: {:?}", out.chunks));
    assert!(
        lesson_hit.preview.contains("sqlx prepare")
            || lesson_hit.preview.contains("sqlx migrations"),
        "preview should surface trigger or advice: {:?}",
        lesson_hit.preview
    );
}

/// The indexer writes an `Extends` edge for every `impl Trait for Type`
/// block. Checking this end-to-end proves both that the parser's new
/// `extract_extends` path fires *and* that the retrieve layer routes
/// the resulting `(child, parent)` pair through `write_extends_edges`
/// into the graph.
#[tokio::test]
async fn indexer_writes_extends_edge_for_rust_impl_trait() {
    let src = r#"
pub trait Greet {
    fn hello(&self) -> String;
}

pub struct English;

impl Greet for English {
    fn hello(&self) -> String { "hello".into() }
}
"#;
    let src_dir = tempdir().unwrap();
    tokio::fs::write(src_dir.path().join("greet.rs"), src)
        .await
        .unwrap();

    let (_thoth_dir, store) = make_store().await;
    make_indexer(store.clone())
        .index_path(src_dir.path())
        .await
        .unwrap();

    // `greet::English` extends `greet::Greet`. The aliases map in this
    // file is empty (no `use` statements), so the parent name `Greet`
    // is written through unresolved — the Extends edge stores the bare
    // name. Accept either the bare or resolved form to stay robust to
    // future resolution upgrades.
    let graph = thoth_graph::Graph::new(store.kv.clone());
    let extends = graph
        .outgoing("greet::English")
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == thoth_graph::EdgeKind::Extends)
        .collect::<Vec<_>>();
    assert!(
        extends
            .iter()
            .any(|e| e.to == "Greet" || e.to == "greet::Greet"),
        "expected an Extends edge English -> Greet; got {extends:?}"
    );
}

/// `use foo::Bar as Baz;` paired with a call to `Baz::make()` should
/// rewrite the call edge's destination from the local alias (`make`)
/// to the resolved path the graph actually knows about. We can't assert
/// the exact resolved FQN without cross-file linking, but we *can*
/// verify that the import-alias pipeline populated the file's Imports
/// edges with the fully qualified target.
#[tokio::test]
async fn indexer_uses_import_aliases_for_imports_edge() {
    let src = r#"
use std::sync::Arc as A;
use std::collections::HashMap;

pub fn noop() {}
"#;
    let src_dir = tempdir().unwrap();
    tokio::fs::write(src_dir.path().join("m.rs"), src)
        .await
        .unwrap();

    let (_thoth_dir, store) = make_store().await;
    make_indexer(store.clone())
        .index_path(src_dir.path())
        .await
        .unwrap();

    let graph = thoth_graph::Graph::new(store.kv.clone());
    let imports = graph
        .outgoing("m")
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == thoth_graph::EdgeKind::Imports)
        .map(|e| e.to)
        .collect::<Vec<_>>();

    assert!(
        imports.iter().any(|t| t == "std::sync::Arc"),
        "expected resolved target std::sync::Arc; got {imports:?}"
    );
    assert!(
        imports.iter().any(|t| t == "std::collections::HashMap"),
        "expected resolved target std::collections::HashMap; got {imports:?}"
    );
}
