//! End-to-end coverage for the dream pass.
//!
//! The model is stubbed with `/bin/sh -c '<script>'`, which lets these
//! tests drive the real subprocess + parse + apply path without an LLM.
//! The dream pass appends the prompt as the final argv entry; under
//! `sh -c` that lands in `$0` and is ignored, so the stub emits a fixed
//! verdict array regardless of what we asked.

use std::path::Path;

use hoangsa_memory_policy::{DreamOpts, dream_pass};
use hoangsa_memory_store::markdown::MarkdownStore;
use tempfile::tempdir;

/// Write a `config.toml` wiring the dream pass to a stub that prints
/// `verdicts_json` on stdout.
async fn write_config(root: &Path, mode: &str, verdicts_json: &str) {
    let script = format!("cat <<'HOANGSA_EOF'\n{verdicts_json}\nHOANGSA_EOF");
    let cfg = format!(
        r#"
[dream]
enabled = true
mode = "{mode}"
min_interval_hours = 0
min_entries = 0
command = ["/bin/sh", "-c", {script:?}]
"#
    );
    tokio::fs::write(root.join("config.toml"), cfg)
        .await
        .unwrap();
}

/// Seed two facts and one lesson through the real store so the on-disk
/// shape matches what the parser expects.
async fn seed(root: &Path) -> MarkdownStore {
    use hoangsa_memory_core::{Fact, Lesson, MemoryKind, MemoryMeta};
    let md = MarkdownStore::open(root).await.unwrap();
    for text in [
        "retry logic lives in crates/net/retry.rs",
        "the CLI entry point is src/main.rs",
    ] {
        md.append_fact(&Fact {
            meta: MemoryMeta::new(MemoryKind::Semantic),
            text: text.to_string(),
            tags: Vec::new(),
            scope: Default::default(),
        })
        .await
        .unwrap();
    }
    md.append_lesson(&Lesson {
        meta: MemoryMeta::new(MemoryKind::Reflective),
        trigger: "when editing migrations".into(),
        advice: "run sqlx prepare afterwards".into(),
        success_count: 3,
        failure_count: 1,
        enforcement: Default::default(),
        suggested_enforcement: None,
        block_message: None,
    })
    .await
    .unwrap();
    md
}

/// `mode = "auto"`: a drop verdict must remove the fact from `MEMORY.md`,
/// archive it with the model's reason, and log the op to history.
#[tokio::test]
async fn auto_mode_drops_fact_and_archives_it() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let md = seed(root).await;
    write_config(
        root,
        "auto",
        r#"[{"surface":"memory","index":1,"action":"drop","reason":"src/main.rs no longer exists"}]"#,
    )
    .await;

    let report = dream_pass(root, DreamOpts::default()).await.unwrap();
    assert!(!report.was_skipped(), "skipped: {:?}", report.skipped);
    assert!(report.applied);
    assert_eq!(report.dropped, 1);

    let facts = md.read_facts().await.unwrap();
    assert_eq!(facts.len(), 1, "the dropped fact should be gone");
    assert!(facts[0].text.contains("retry logic"));

    let archive = tokio::fs::read_to_string(root.join("MEMORY.dropped.md"))
        .await
        .expect("dropped entries must be archived, not deleted");
    assert!(archive.contains("the CLI entry point"));
    assert!(
        archive.contains("no longer exists"),
        "the model's reason must survive into the archive: {archive}"
    );

    let history = md.read_history().await.unwrap();
    assert!(
        history
            .iter()
            .any(|h| h.op == "dream_drop" && h.actor.as_deref() == Some("dream")),
        "the drop must be auditable in memory-history.jsonl"
    );
}

/// A lesson rewrite swaps the wording but must not reset the success and
/// failure counters — those are earned state, not part of the text.
#[tokio::test]
async fn auto_mode_rewrite_preserves_lesson_counters() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let md = seed(root).await;
    write_config(
        root,
        "auto",
        r#"[{"surface":"lessons","index":0,"action":"rewrite","reason":"tightened","new_text":"when editing sqlx migrations\nrun sqlx prepare before committing"}]"#,
    )
    .await;

    let report = dream_pass(root, DreamOpts::default()).await.unwrap();
    assert_eq!(report.rewritten, 1);

    let lessons = md.read_lessons().await.unwrap();
    assert_eq!(lessons.len(), 1);
    assert_eq!(lessons[0].trigger, "when editing sqlx migrations");
    assert!(lessons[0].advice.contains("before committing"));
    assert_eq!(
        lessons[0].success_count, 3,
        "counters must survive a rewrite"
    );
    assert_eq!(
        lessons[0].failure_count, 1,
        "counters must survive a rewrite"
    );
}

/// A merge folds one fact into another: the target gets the combined text
/// and the folded entry leaves `MEMORY.md` for the archive.
#[tokio::test]
async fn auto_mode_merge_folds_entries() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let md = seed(root).await;
    write_config(
        root,
        "auto",
        r#"[{"surface":"memory","index":0,"action":"merge","merge_with":[1],"reason":"same subject","new_text":"entry points: retry in crates/net/retry.rs, CLI in src/main.rs"}]"#,
    )
    .await;

    let report = dream_pass(root, DreamOpts::default()).await.unwrap();
    assert_eq!(report.merged, 1);
    assert_eq!(report.rewritten, 1);

    let facts = md.read_facts().await.unwrap();
    assert_eq!(facts.len(), 1);
    assert!(facts[0].text.contains("entry points:"));

    let archive = tokio::fs::read_to_string(root.join("MEMORY.dropped.md"))
        .await
        .unwrap();
    assert!(
        archive.contains("folded into entry [0]"),
        "the archive must say where the folded content went: {archive}"
    );
}

/// `mode = "review"` is the default posture and must be inert: proposals
/// go to `DREAM.md` and every canonical surface is left byte-identical.
#[tokio::test]
async fn review_mode_writes_proposals_without_touching_memory() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let md = seed(root).await;
    let before = tokio::fs::read_to_string(root.join("MEMORY.md"))
        .await
        .unwrap();
    write_config(
        root,
        "review",
        r#"[{"surface":"memory","index":1,"action":"drop","reason":"stale"}]"#,
    )
    .await;

    let report = dream_pass(root, DreamOpts::default()).await.unwrap();
    assert!(!report.applied);
    assert_eq!(report.dropped, 1, "the proposal is still counted");

    let after = tokio::fs::read_to_string(root.join("MEMORY.md"))
        .await
        .unwrap();
    assert_eq!(before, after, "review mode must not mutate MEMORY.md");
    assert_eq!(md.read_facts().await.unwrap().len(), 2);

    let review = tokio::fs::read_to_string(report.review_path.unwrap())
        .await
        .unwrap();
    assert!(review.contains("DROP"));
    assert!(review.contains("the CLI entry point"));
    assert!(!Path::new(&root.join("MEMORY.dropped.md")).exists());
}

/// An unrecognised `mode` must fail closed to review rather than being
/// treated as permission to write.
#[tokio::test]
async fn unknown_mode_falls_back_to_review() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    seed(root).await;
    write_config(
        root,
        "yolo",
        r#"[{"surface":"memory","index":0,"action":"drop","reason":"nope"}]"#,
    )
    .await;

    let report = dream_pass(root, DreamOpts::default()).await.unwrap();
    assert!(!report.applied);
    assert!(report.review_path.is_some());
}

/// `--dry-run` consults the model and reports, but writes nothing at all —
/// not the surfaces, not `DREAM.md`, not the interval marker.
#[tokio::test]
async fn dry_run_writes_nothing() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    seed(root).await;
    write_config(
        root,
        "auto",
        r#"[{"surface":"memory","index":0,"action":"drop","reason":"stale"}]"#,
    )
    .await;

    let report = dream_pass(
        root,
        DreamOpts {
            force: false,
            dry_run: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(report.dropped, 1);
    assert!(!report.applied);
    assert!(report.review_path.is_none());
    assert!(!root.join("DREAM.md").exists());
    assert!(!root.join("MEMORY.dropped.md").exists());
    assert!(!root.join(".dream-last").exists());
}

/// A model that fails outright must leave memory untouched and report a
/// skip — a broken subprocess is not a mandate to delete anything.
#[tokio::test]
async fn failing_model_leaves_memory_intact() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let md = seed(root).await;
    let cfg = r#"
[dream]
enabled = true
mode = "auto"
min_interval_hours = 0
min_entries = 0
command = ["/bin/sh", "-c", "exit 3"]
"#;
    tokio::fs::write(root.join("config.toml"), cfg)
        .await
        .unwrap();

    let report = dream_pass(root, DreamOpts::default()).await.unwrap();
    assert!(report.was_skipped());
    assert_eq!(md.read_facts().await.unwrap().len(), 2);
    assert_eq!(md.read_lessons().await.unwrap().len(), 1);
}

/// The interval floor must survive a process restart — it is stored on
/// disk, not in memory.
#[tokio::test]
async fn second_pass_is_rate_limited_by_interval() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    seed(root).await;
    let script = "cat <<'HOANGSA_EOF'\n[]\nHOANGSA_EOF";
    let cfg = format!(
        r#"
[dream]
enabled = true
mode = "auto"
min_interval_hours = 12
min_entries = 0
command = ["/bin/sh", "-c", {script:?}]
"#
    );
    tokio::fs::write(root.join("config.toml"), cfg)
        .await
        .unwrap();

    let first = dream_pass(root, DreamOpts::default()).await.unwrap();
    assert!(!first.was_skipped(), "first pass should run");

    let second = dream_pass(root, DreamOpts::default()).await.unwrap();
    assert!(second.was_skipped(), "second pass should hit the 12h floor");

    let forced = dream_pass(
        root,
        DreamOpts {
            force: true,
            dry_run: false,
        },
    )
    .await
    .unwrap();
    assert!(!forced.was_skipped(), "--force must bypass the floor");
}

/// A fact appended **while the model is thinking** must survive the pass.
///
/// The snapshot is read before the subprocess starts and the apply path
/// rewrites the whole file; driven by the snapshot alone it erased anything
/// written in that window — with no archive row and no history line, because
/// the code never knew the entry existed. That was the one unrecoverable
/// mutation in the dream pass.
#[tokio::test]
async fn concurrent_append_survives_the_pass() {
    use hoangsa_memory_core::{Fact, MemoryKind, MemoryMeta};

    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let md = seed(&root).await;

    // The stub stalls, standing in for a model call that takes minutes.
    let script = "sleep 1; cat <<'HOANGSA_EOF'\n\
                  [{\"surface\":\"memory\",\"index\":1,\"action\":\"drop\",\"reason\":\"stale\"}]\n\
                  HOANGSA_EOF";
    let cfg = format!(
        "\n[dream]\nenabled = true\nmode = \"auto\"\nmin_interval_hours = 0\n\
         min_entries = 0\ncommand = [\"/bin/sh\", \"-c\", {script:?}]\n"
    );
    tokio::fs::write(root.join("config.toml"), cfg)
        .await
        .unwrap();

    let pass_root = root.clone();
    let pass = tokio::spawn(async move { dream_pass(&pass_root, DreamOpts::default()).await });

    // Land the write inside the model's window.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    md.append_fact(&Fact {
        meta: MemoryMeta::new(MemoryKind::Semantic),
        text: "written while the model was thinking".into(),
        tags: Vec::new(),
        scope: Default::default(),
    })
    .await
    .unwrap();

    let report = pass.await.unwrap().unwrap();
    assert!(!report.was_skipped(), "skipped: {:?}", report.skipped);

    let texts: Vec<String> = md
        .read_facts()
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.text)
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("written while the model")),
        "the concurrent append was erased: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("retry logic")),
        "the untouched fact must remain: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("CLI entry point")),
        "the verdict must still apply to the entry it was about: {texts:?}"
    );
}

/// A pass whose model fails must still consume the interval.
///
/// Only the success path stamped the marker, and the daemon's sole gate is
/// that marker — so a `claude` on PATH that exits non-zero (not logged in,
/// rate-limited) turned a twice-a-day pass into an LLM subprocess every
/// 15 minutes, forever, logged only at debug level.
#[tokio::test]
async fn failed_model_run_still_consumes_the_interval() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    seed(root).await;
    let cfg = "\n[dream]\nenabled = true\nmode = \"auto\"\nmin_interval_hours = 12\n\
               min_entries = 0\ncommand = [\"/bin/sh\", \"-c\", \"exit 7\"]\n";
    tokio::fs::write(root.join("config.toml"), cfg)
        .await
        .unwrap();

    let first = dream_pass(root, DreamOpts::default()).await.unwrap();
    assert!(
        first.was_skipped(),
        "a failing model is a skip, not an error"
    );
    assert!(
        root.join(".dream-last").exists(),
        "a failed attempt must stamp the marker or the daemon retries forever"
    );

    // The interval gate must now hold the next attempt off.
    let second = dream_pass(root, DreamOpts::default()).await.unwrap();
    assert!(second.was_skipped());
    assert!(
        second
            .skipped
            .as_deref()
            .unwrap_or("")
            .contains("under 12h"),
        "second attempt should be interval-gated, got {:?}",
        second.skipped
    );
}

/// `--dry-run` must report the same deletions the apply path would make.
///
/// `plan_for` treats `rewrite` and `merge` identically and honours
/// `merge_with` for both, but the old preview counted raw verdicts and never
/// looked at `merge_with` for a `rewrite` — so it under-reported exactly the
/// blast radius it exists to show.
#[tokio::test]
async fn dry_run_counts_merges_carried_by_a_rewrite() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    seed(root).await;
    write_config(
        root,
        "auto",
        r#"[{"surface":"memory","index":0,"action":"rewrite","new_text":"consolidated","merge_with":[1],"reason":"duplicate"}]"#,
    )
    .await;

    let report = dream_pass(
        root,
        DreamOpts {
            dry_run: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!report.applied);
    assert_eq!(report.rewritten, 1);
    assert_eq!(
        report.merged, 1,
        "a rewrite carrying merge_with removes entries; the preview must say so"
    );
}

/// `merge_with` REMOVES entries, so it must obey the same snapshot filter as
/// `index`. Filtering only `index` left the model able to delete an entry the
/// prompt budget had dropped — an entry it never saw.
#[tokio::test]
async fn merge_with_cannot_reach_an_entry_outside_the_snapshot() {
    use hoangsa_memory_core::{Fact, MemoryKind, MemoryMeta};

    let dir = tempdir().unwrap();
    let root = dir.path();
    let md = MarkdownStore::open(root).await.unwrap();

    // Enough bulk that the last fact falls outside the per-surface budget.
    for i in 0..30 {
        md.append_fact(&Fact {
            meta: MemoryMeta::new(MemoryKind::Semantic),
            text: format!("fact {i}\n{}", "x".repeat(1000)),
            tags: Vec::new(),
            scope: Default::default(),
        })
        .await
        .unwrap();
    }

    write_config(
        root,
        "auto",
        r#"[{"surface":"memory","index":0,"action":"merge","merge_with":[29],"new_text":"combined","reason":"dup"}]"#,
    )
    .await;

    let before = md.read_facts().await.unwrap().len();
    let report = dream_pass(root, DreamOpts::default()).await.unwrap();
    assert!(!report.was_skipped(), "skipped: {:?}", report.skipped);

    let after = md.read_facts().await.unwrap();
    assert!(
        after.iter().any(|f| f.text.starts_with("fact 29")),
        "fact 29 was never shown to the model and must not be deletable"
    );
    assert_eq!(
        after.len(),
        before,
        "no entry outside the snapshot may be removed"
    );
}

/// Two entries can share a heading. Keying `reconcile` on the heading alone
/// made a concurrent delete of the first hand the survivor the FIRST queued
/// plan slot — so an entry the model explicitly said to KEEP got dropped.
#[tokio::test]
async fn duplicate_headings_do_not_cross_apply_verdicts() {
    use hoangsa_memory_core::{Fact, MemoryKind, MemoryMeta};

    let dir = tempdir().unwrap();
    let root = dir.path();
    let md = MarkdownStore::open(root).await.unwrap();
    for body in ["body FIRST", "body SECOND"] {
        md.append_fact(&Fact {
            meta: MemoryMeta::new(MemoryKind::Semantic),
            text: format!("same heading\n{body}"),
            tags: Vec::new(),
            scope: Default::default(),
        })
        .await
        .unwrap();
    }

    // Model: drop index 0 (FIRST), keep index 1 (SECOND).
    let script = "sleep 1; cat <<'HOANGSA_EOF'\n\
                  [{\"surface\":\"memory\",\"index\":0,\"action\":\"drop\",\"reason\":\"dup\"}]\n\
                  HOANGSA_EOF";
    let cfg = format!(
        "\n[dream]\nenabled = true\nmode = \"auto\"\nmin_interval_hours = 0\n\
         min_entries = 0\ncommand = [\"/bin/sh\", \"-c\", {script:?}]\n"
    );
    tokio::fs::write(root.join("config.toml"), cfg)
        .await
        .unwrap();

    let pass_root = root.to_path_buf();
    let pass = tokio::spawn(async move { dream_pass(&pass_root, DreamOpts::default()).await });

    // Concurrently delete FIRST — the entry the verdict was about.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let remaining: Vec<Fact> = md
        .read_facts()
        .await
        .unwrap()
        .into_iter()
        .filter(|f| !f.text.contains("body FIRST"))
        .collect();
    md.rewrite_facts(&remaining).await.unwrap();

    pass.await.unwrap().unwrap();

    let after = md.read_facts().await.unwrap();
    assert!(
        after.iter().any(|f| f.text.contains("body SECOND")),
        "the entry the model said to KEEP must survive: {after:?}"
    );
}
