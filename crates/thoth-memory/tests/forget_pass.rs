//! Integration tests for the lesson-confidence slice of
//! [`MemoryManager::forget_pass`].

use tempfile::tempdir;
use thoth_core::{Event, EventId, Lesson, MemoryKind, MemoryMeta};
use thoth_memory::MemoryManager;
use time::{Duration, OffsetDateTime};

fn lesson(trigger: &str, advice: &str, success: u64, failure: u64) -> Lesson {
    Lesson {
        meta: MemoryMeta::new(MemoryKind::Reflective),
        trigger: trigger.to_string(),
        advice: advice.to_string(),
        success_count: success,
        failure_count: failure,
        enforcement: Default::default(),
        suggested_enforcement: None,
        block_message: None,
    }
}

#[tokio::test]
async fn forget_pass_drops_lessons_below_floor_after_min_attempts() {
    let dir = tempdir().unwrap();
    let mgr = MemoryManager::open(dir.path()).await.unwrap();

    // `lesson_floor = 0.2` and `lesson_min_attempts = 3` by default.
    // `keep`: high ratio — 4/5 → 0.66.
    // `drop`: low ratio — 0/5 → 0.0, plenty of attempts.
    // `young`: low ratio but below min_attempts — must be kept.
    mgr.md
        .rewrite_lessons(&[
            lesson("keep", "solid rule", 4, 1),
            lesson("drop", "misleading rule", 0, 5),
            lesson("young", "newborn rule", 0, 1),
        ])
        .await
        .unwrap();

    let report = mgr.forget_pass().await.unwrap();
    assert_eq!(report.lessons_dropped, 1);

    let remaining: Vec<_> = mgr
        .md
        .read_lessons()
        .await
        .unwrap()
        .into_iter()
        .map(|l| l.trigger)
        .collect();
    assert!(remaining.contains(&"keep".to_string()));
    assert!(remaining.contains(&"young".to_string()));
    assert!(!remaining.contains(&"drop".to_string()));
}

/// DESIGN §9 decay pass — episodes whose effective retention score falls
/// below `decay_floor` must be dropped by `forget_pass`, and episodes that
/// are either fresh or recently accessed must survive. We pin the contract
/// by appending two episodes and manually aging one of them past the decay
/// horizon via the SQL column.
#[tokio::test]
async fn forget_pass_evicts_decayed_episodes() {
    let dir = tempdir().unwrap();
    let mgr = MemoryManager::open(dir.path()).await.unwrap();

    // Append two events — one will remain "fresh" (access bumps it), one
    // will be aged via a direct SQL poke below.
    let fresh_id: EventId = uuid::Uuid::new_v4();
    let stale_id: EventId = uuid::Uuid::new_v4();
    mgr.episodes
        .append(&Event::QueryIssued {
            id: fresh_id,
            text: "fresh retrieval".to_string(),
            at: OffsetDateTime::now_utc(),
        })
        .await
        .unwrap();
    mgr.episodes
        .append(&Event::QueryIssued {
            id: stale_id,
            text: "stale retrieval".to_string(),
            at: OffsetDateTime::now_utc(),
        })
        .await
        .unwrap();

    // Inspect current rows to discover their IDs, then mutate the "stale"
    // one via `iter_with_decay_meta`-style access through a raw sqlite
    // update. We reach back through the episode log's open() path using a
    // fresh connection to the same file — `rusqlite` multiple handles on
    // WAL mode are fine.
    let db_path = dir.path().join("episodes.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    // Age the "stale" row by 10 years and zero its salience floor.
    let long_ago =
        (OffsetDateTime::now_utc() - Duration::days(3_650)).unix_timestamp_nanos() as i64;
    conn.execute(
        "UPDATE episodes SET salience = 0.01, last_accessed_ns = ?1 \
         WHERE payload LIKE '%stale retrieval%'",
        rusqlite::params![long_ago],
    )
    .unwrap();
    // Close the ad-hoc handle before the forget pass so we don't hold the
    // write lock over WAL.
    drop(conn);

    // Confirm both rows exist before the pass.
    let before = mgr.episodes.count().await.unwrap();
    assert_eq!(
        before, 2,
        "both events should be present before forget_pass"
    );

    let report = mgr.forget_pass().await.unwrap();

    // Exactly the stale row should have been decay-evicted. TTL won't
    // fire because the row's `at_unix_ns` is recent; capacity won't fire
    // because we're far below `max_episodes`.
    assert_eq!(
        report.episodes_decayed, 1,
        "expected one decay eviction; got {report:?}",
    );
    let after = mgr.episodes.count().await.unwrap();
    assert_eq!(after, 1, "only fresh row should remain");
}

#[tokio::test]
async fn bump_then_rewrite_preserves_counters_via_footer() {
    let dir = tempdir().unwrap();
    let mgr = MemoryManager::open(dir.path()).await.unwrap();

    mgr.md
        .append_lesson(&lesson("when editing migrations", "run sqlx prepare", 0, 0))
        .await
        .unwrap();

    let bumped = mgr
        .md
        .bump_lesson_success(&["when editing migrations".to_string()])
        .await
        .unwrap();
    assert_eq!(bumped, 1);

    // Re-read: the counter footer round-trips through the parser.
    let after = mgr.md.read_lessons().await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].success_count, 1);
    assert_eq!(after[0].failure_count, 0);

    // And a failure bump compounds.
    mgr.md
        .bump_lesson_failure(&["when editing migrations".to_string()])
        .await
        .unwrap();
    let after2 = mgr.md.read_lessons().await.unwrap();
    assert_eq!(after2[0].success_count, 1);
    assert_eq!(after2[0].failure_count, 1);
}
