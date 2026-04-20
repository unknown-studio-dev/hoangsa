//! Integration test — end-to-end promotion ladder.
//!
//! Simulates the lifecycle where a lesson starts at `Advise`, accrues
//! `failure_count` hits from the outcome harvester, and is auto-promoted
//! through `Require → Block` as thresholds cross.
//!
//! Covers TEST-SPEC `end_to_end_advise_to_block_promotion`
//! (REQ-12, REQ-13).

use thoth_core::memory::{Enforcement, Lesson, LessonTrigger, MemoryKind, MemoryMeta};
use thoth_memory::EnforcementConfig;
use thoth_memory::promotion::{PromotionAction, evaluate_and_apply};

fn mk_lesson() -> Lesson {
    Lesson {
        meta: MemoryMeta::new(MemoryKind::Reflective),
        trigger: "edit test.txt".into(),
        advice: "use trash cli instead".into(),
        success_count: 0,
        failure_count: 0,
        enforcement: Enforcement::Advise,
        suggested_enforcement: None,
        block_message: Some("blocked: use trash".into()),
    }
}

/// Default config: promote_threshold=2, demote_threshold=2.
fn cfg() -> EnforcementConfig {
    EnforcementConfig::default()
}

#[test]
fn advise_stays_until_threshold() {
    let mut l = mk_lesson();
    // One violation is not enough (threshold = 2).
    l.failure_count = 1;
    let action = evaluate_and_apply(&mut l, &cfg());
    assert_eq!(action, PromotionAction::NoChange);
    assert_eq!(l.enforcement, Enforcement::Advise);
}

#[test]
fn e2e_advise_to_require_to_block_walks_ladder() {
    // Simulates the outcome harvester incrementing failure_count and the
    // promotion engine firing on each evaluation.
    let mut l = mk_lesson();

    // First violation: still Advise.
    l.failure_count += 1;
    assert_eq!(
        evaluate_and_apply(&mut l, &cfg()),
        PromotionAction::NoChange
    );
    assert_eq!(l.enforcement, Enforcement::Advise);

    // Second violation: crosses promote_threshold=2 → Require.
    l.failure_count += 1;
    let action = evaluate_and_apply(&mut l, &cfg());
    assert!(matches!(action, PromotionAction::Promote { .. }));
    assert_eq!(l.enforcement, Enforcement::Require);

    // Two more violations bring the counter to 4 — threshold is reached
    // again (counters are not reset by design, see promotion.rs policy).
    l.failure_count += 1;
    l.failure_count += 1;
    let action = evaluate_and_apply(&mut l, &cfg());
    assert!(matches!(action, PromotionAction::Promote { .. }));
    assert_eq!(l.enforcement, Enforcement::Block);

    // One more violation: already at Block, engine is idempotent.
    l.failure_count += 1;
    assert_eq!(
        evaluate_and_apply(&mut l, &cfg()),
        PromotionAction::NoChange
    );
    assert_eq!(l.enforcement, Enforcement::Block);
}

#[test]
fn e2e_suggested_block_jumps_straight_to_block() {
    // If the lesson's `suggested_enforcement` is Block, the engine promotes
    // directly to it instead of walking one rung.
    let mut l = mk_lesson();
    l.suggested_enforcement = Some(Enforcement::Block);
    l.failure_count = 2;
    let action = evaluate_and_apply(&mut l, &cfg());
    assert!(matches!(action, PromotionAction::Promote { .. }));
    assert_eq!(l.enforcement, Enforcement::Block);
}

#[test]
fn e2e_trigger_is_preserved_across_promotion() {
    // Promotion must not mutate the trigger — only the enforcement tier.
    let mut l = Lesson {
        trigger: "edit migrations".into(),
        ..mk_lesson()
    };
    let trigger_before = l.trigger.clone();
    let _glob = LessonTrigger::natural_only("canary"); // sanity

    l.failure_count = 2;
    evaluate_and_apply(&mut l, &cfg());
    assert_eq!(l.enforcement, Enforcement::Require);
    assert_eq!(l.trigger, trigger_before);
}
