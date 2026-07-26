//! Concurrent read-modify-write on a markdown surface must not lose updates.
//!
//! `write_atomic` makes the publish atomic, but load → mutate → save is a
//! lost-update race between writers: the daemon's forget pass, a hook-fired
//! `lesson-feedback`, and an MCP `memory_remember_*` all rewrite these files.
//! Unlocked, 12 concurrent bumps landed 1.

use hoangsa_memory_core::{Enforcement, Lesson, MemoryKind, MemoryMeta};
use hoangsa_memory_store::MarkdownStore;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_lesson_bumps_are_all_recorded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let md = MarkdownStore::open(dir.path()).await.expect("open");
    md.append_lesson(&Lesson {
        meta: MemoryMeta::new(MemoryKind::Reflective),
        trigger: "when editing migrations".into(),
        advice: "run sqlx prepare".into(),
        success_count: 0,
        failure_count: 0,
        enforcement: Enforcement::default(),
        suggested_enforcement: None,
        block_message: None,
    })
    .await
    .expect("append");

    const N: u64 = 12;
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..N {
        let root = dir.path().to_path_buf();
        set.spawn(async move {
            let md = MarkdownStore::open(&root).await.expect("open");
            md.bump_lesson_success(&["when editing migrations".to_string()])
                .await
                .expect("bump");
        });
    }
    while set.join_next().await.is_some() {}

    let lessons = md.read_lessons().await.expect("read");
    assert_eq!(lessons.len(), 1, "the surface must not be corrupted");
    assert_eq!(
        lessons[0].success_count, N,
        "every bump must survive; got {} of {N}",
        lessons[0].success_count
    );
}
