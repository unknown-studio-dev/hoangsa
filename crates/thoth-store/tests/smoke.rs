//! End-to-end smoke tests for `thoth-store`.
//!
//! Run with `cargo test -p thoth-store`.

use std::path::PathBuf;

use tempfile::tempdir;
use thoth_core::{Event, Fact, Lesson, MemoryKind, MemoryMeta};
use thoth_store::{
    ChunkDoc, EdgeRow, EpisodeLog, FtsIndex, KvStore, MarkdownStore, NodeRow, StoreRoot, SymbolRow,
};
use time::OffsetDateTime;
use uuid::Uuid;

#[tokio::test]
async fn store_root_opens_all_backends() {
    let dir = tempdir().unwrap();
    let root = StoreRoot::open(dir.path()).await.unwrap();

    assert!(root.path.exists());
    // DESIGN §7 flat layout — index files live at the root now.
    assert!(root.path.join("graph.redb").exists());
    assert!(root.path.join("fts.tantivy").exists());
    assert!(root.path.join("episodes.db").exists());
    assert!(root.path.join("skills").exists());
}

#[tokio::test]
async fn store_root_migrates_legacy_index_layout() {
    use tokio::fs;
    let dir = tempdir().unwrap();
    let legacy = dir.path().join("index");
    fs::create_dir_all(&legacy).await.unwrap();
    // Seed real backend files at the legacy paths so the migration moves
    // genuine databases rather than placeholder bytes that would fail to
    // reopen on the new paths.
    {
        let kv = KvStore::open(legacy.join("kv.redb")).await.unwrap();
        kv.put_meta("migration-marker", b"present").await.unwrap();
        drop(kv);

        let fts = FtsIndex::open(legacy.join("fts")).await.unwrap();
        drop(fts);

        let eps = EpisodeLog::open(legacy.join("episodes.sqlite"))
            .await
            .unwrap();
        drop(eps);
    }

    let root = StoreRoot::open(dir.path()).await.unwrap();

    // New-layout files exist at the root after migration …
    assert!(root.path.join("graph.redb").exists());
    assert!(root.path.join("fts.tantivy").exists());
    assert!(root.path.join("episodes.db").exists());
    // … and carry the legacy data (the kv marker survives the rename).
    assert_eq!(
        root.kv
            .get_meta("migration-marker")
            .await
            .unwrap()
            .as_deref(),
        Some(&b"present"[..])
    );
    // Legacy dir is pruned after a successful migration.
    assert!(!root.path.join("index").exists());
}

#[tokio::test]
async fn kv_roundtrip_symbols_nodes_edges() {
    let dir = tempdir().unwrap();
    let kv = KvStore::open(dir.path().join("kv.redb")).await.unwrap();

    // Meta
    kv.put_meta("cursor", b"abc").await.unwrap();
    assert_eq!(
        kv.get_meta("cursor").await.unwrap().as_deref(),
        Some(&b"abc"[..])
    );

    // Symbol
    let sym = SymbolRow {
        fqn: "demo::auth::verify".to_string(),
        path: PathBuf::from("src/auth.rs"),
        start_line: 10,
        end_line: 30,
        kind: "function".into(),
    };
    kv.put_symbol(sym.clone()).await.unwrap();
    let got = kv.get_symbol(&sym.fqn).await.unwrap().unwrap();
    assert_eq!(got.fqn, sym.fqn);
    assert_eq!(got.start_line, 10);

    // Also insert a second symbol so prefix search has something to do.
    kv.put_symbol(SymbolRow {
        fqn: "demo::auth::sign".to_string(),
        path: PathBuf::from("src/auth.rs"),
        start_line: 40,
        end_line: 50,
        kind: "function".into(),
    })
    .await
    .unwrap();
    kv.put_symbol(SymbolRow {
        fqn: "other::thing".to_string(),
        path: PathBuf::from("src/other.rs"),
        start_line: 1,
        end_line: 5,
        kind: "function".into(),
    })
    .await
    .unwrap();

    let hits = kv.symbols_with_prefix("demo::auth::").await.unwrap();
    assert_eq!(hits.len(), 2);

    // Node + edge
    kv.put_node(NodeRow {
        id: "demo::auth::verify".into(),
        kind: "function".into(),
        payload: serde_json::json!({"public": true}),
    })
    .await
    .unwrap();
    kv.put_edge(EdgeRow {
        src: "demo::auth::verify".into(),
        dst: "demo::auth::sign".into(),
        kind: "calls".into(),
        payload: serde_json::json!({}),
    })
    .await
    .unwrap();

    let n = kv.get_node("demo::auth::verify").await.unwrap().unwrap();
    assert_eq!(n.kind, "function");

    let outs = kv.edges_from("demo::auth::verify").await.unwrap();
    assert_eq!(outs.len(), 1);
    assert_eq!(outs[0].dst, "demo::auth::sign");
}

#[tokio::test]
async fn fts_indexes_and_ranks_hits() {
    let dir = tempdir().unwrap();
    let fts = FtsIndex::open(dir.path()).await.unwrap();

    fts.index_chunk(ChunkDoc {
        id: "a:1-5".into(),
        path: "src/a.rs".into(),
        symbol: Some("verify_token".into()),
        body: "fn verify_token(jwt: &str) -> bool { /* RS256 */ true }".into(),
        start_line: 1,
        end_line: 5,
        language: "rust".into(),
    })
    .await
    .unwrap();

    fts.index_chunk(ChunkDoc {
        id: "b:1-5".into(),
        path: "src/b.rs".into(),
        symbol: Some("fetch_users".into()),
        body: "async fn fetch_users(db: &Db) -> Vec<User> { todo!() }".into(),
        start_line: 1,
        end_line: 5,
        language: "rust".into(),
    })
    .await
    .unwrap();

    fts.commit().await.unwrap();

    let hits = fts.search("verify jwt", 5).await.unwrap();
    assert!(!hits.is_empty(), "expected at least one hit");
    assert_eq!(hits[0].id, "a:1-5");
}

#[tokio::test]
async fn episodes_append_and_search() {
    let dir = tempdir().unwrap();
    let log = EpisodeLog::open(dir.path().join("episodes.sqlite"))
        .await
        .unwrap();

    let id = Uuid::new_v4();
    log.append(&Event::QueryIssued {
        id,
        text: "where is the JWT signer".to_string(),
        at: OffsetDateTime::now_utc(),
    })
    .await
    .unwrap();

    log.append(&Event::FileChanged {
        path: PathBuf::from("src/auth.rs"),
        commit: None,
        at: OffsetDateTime::now_utc(),
    })
    .await
    .unwrap();

    assert_eq!(log.count().await.unwrap(), 2);

    let recent = log.recent(10).await.unwrap();
    assert_eq!(recent.len(), 2);

    let hits = log.search("jwt", 5).await.unwrap();
    assert!(
        hits.iter().any(|h| h.kind == "query_issued"),
        "expected the JWT query to come back: {hits:?}"
    );
}

#[tokio::test]
async fn markdown_facts_and_lessons_roundtrip() {
    let dir = tempdir().unwrap();
    let md = MarkdownStore::open(dir.path()).await.unwrap();

    let fact = Fact {
        meta: MemoryMeta::new(MemoryKind::Semantic),
        text: "auth uses JWT with RS256".into(),
        tags: vec!["auth".into(), "security".into()],
        scope: Default::default(),
    };
    md.append_fact(&fact).await.unwrap();

    let facts = md.read_facts().await.unwrap();
    assert_eq!(facts.len(), 1);
    assert!(facts[0].text.starts_with("auth uses JWT"));
    assert_eq!(facts[0].tags, vec!["auth", "security"]);

    let lesson = Lesson {
        meta: MemoryMeta::new(MemoryKind::Reflective),
        trigger: "when editing migrations".into(),
        advice: "Always run `sqlx prepare` afterwards.".into(),
        success_count: 0,
        failure_count: 0,
        enforcement: Default::default(),
        suggested_enforcement: None,
        block_message: None,
    };
    md.append_lesson(&lesson).await.unwrap();

    let lessons = md.read_lessons().await.unwrap();
    assert_eq!(lessons.len(), 1);
    assert_eq!(lessons[0].trigger, "when editing migrations");
    assert!(lessons[0].advice.contains("sqlx prepare"));

    // Regression guard (2026-04-17): canonical append_fact + append_lesson
    // MUST write an `op="append"` entry to memory-history.jsonl. Without
    // this, the reflection-debt counter in thoth-memory silently hides
    // every auto-mode remember, so debt monotonically grows until the
    // gate hard-blocks the agent for "0 remembers" even when it has
    // remembered plenty. Verify both kinds landed in the log.
    let history = md.read_history().await.unwrap();
    let append_entries: Vec<_> = history.iter().filter(|h| h.op == "append").collect();
    assert_eq!(
        append_entries.len(),
        2,
        "expected 1 append for fact + 1 for lesson, got: {history:?}"
    );
    assert!(
        append_entries.iter().any(|h| h.kind == "fact"),
        "missing fact-append history entry: {history:?}"
    );
    assert!(
        append_entries.iter().any(|h| h.kind == "lesson"),
        "missing lesson-append history entry: {history:?}"
    );
}

#[tokio::test]
async fn markdown_lists_skills_with_frontmatter() {
    let dir = tempdir().unwrap();
    let md = MarkdownStore::open(dir.path()).await.unwrap();

    let skill_dir = dir.path().join("skills").join("auth-jwt");
    tokio::fs::create_dir_all(&skill_dir).await.unwrap();
    tokio::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: auth-jwt\ndescription: Wire a JWT auth flow through thoth-auth.\n---\n# steps\n1. mint key\n",
    )
    .await
    .unwrap();

    let skills = md.list_skills().await.unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].slug, "auth-jwt");
    assert!(skills[0].description.contains("JWT"));
}
