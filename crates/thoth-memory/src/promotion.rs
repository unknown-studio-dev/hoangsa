//! Auto-promote / auto-demote engine for lesson enforcement tiers.
//!
//! See DESIGN-SPEC REQ-13 / REQ-14 and the T-12 task context.
//!
//! Policy:
//!
//! - A lesson whose `failure_count` reaches `EnforcementConfig::promote_threshold`
//!   escalates one tier along the chain `Advise → Require → Block`. If
//!   `suggested_enforcement` is set and is stricter than the next natural tier,
//!   the lesson is promoted straight to the suggested tier instead.
//! - A lesson whose `success_count` reaches `EnforcementConfig::demote_threshold`
//!   de-escalates one tier along the chain `Block → Require → Advise`.
//! - Promotion and demotion are each gated by the
//!   [`EnforcementConfig::auto_promote`] / [`EnforcementConfig::auto_demote`]
//!   bool toggles — when off this module is a no-op.
//! - `Enforcement::RequireRecall { .. }` and `Enforcement::WorkflowGate` are
//!   structural tiers, not rungs on the Advise/Require/Block ladder; they are
//!   left untouched by the auto-engine.
//! - Counters are *not* reset after a tier change: a lesson that keeps failing
//!   will be evaluated against the same thresholds on every call, so the engine
//!   is naturally idempotent once it reaches `Block`.

use thoth_core::memory::{Enforcement, Lesson};

use crate::EnforcementConfig;

/// Outcome of evaluating a single [`Lesson`] against the promotion engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionAction {
    /// No tier change — either thresholds not met, toggles disabled, or the
    /// lesson is already at the extreme of its ladder.
    NoChange,
    /// Lesson should be promoted from `from` to `to`.
    Promote {
        /// Previous enforcement tier.
        from: Enforcement,
        /// New (stricter) enforcement tier.
        to: Enforcement,
    },
    /// Lesson should be demoted from `from` to `to`.
    Demote {
        /// Previous enforcement tier.
        from: Enforcement,
        /// New (looser) enforcement tier.
        to: Enforcement,
    },
}

/// Return the next stricter tier on the Advise → Require → Block ladder, or
/// `None` if already at `Block` / on a non-ladder tier.
fn next_up(tier: &Enforcement) -> Option<Enforcement> {
    match tier {
        Enforcement::Advise => Some(Enforcement::Require),
        Enforcement::Require => Some(Enforcement::Block),
        Enforcement::Block => None,
        // Structural tiers — not on the promote ladder.
        Enforcement::RequireRecall { .. } | Enforcement::WorkflowGate => None,
    }
}

/// Return the next looser tier on the Block → Require → Advise ladder, or
/// `None` if already at `Advise` / on a non-ladder tier.
fn next_down(tier: &Enforcement) -> Option<Enforcement> {
    match tier {
        Enforcement::Block => Some(Enforcement::Require),
        Enforcement::Require => Some(Enforcement::Advise),
        Enforcement::Advise => None,
        Enforcement::RequireRecall { .. } | Enforcement::WorkflowGate => None,
    }
}

/// Rank a ladder tier for strictness comparison. Higher = stricter.
/// Non-ladder tiers return `None`.
fn ladder_rank(tier: &Enforcement) -> Option<u8> {
    match tier {
        Enforcement::Advise => Some(0),
        Enforcement::Require => Some(1),
        Enforcement::Block => Some(2),
        Enforcement::RequireRecall { .. } | Enforcement::WorkflowGate => None,
    }
}

/// Decide what, if anything, to do with `lesson` given the current `config`.
///
/// Pure function — does not mutate. Callers (e.g. the outcome harvester) use
/// [`apply`] to commit the resulting action onto the lesson.
#[allow(clippy::collapsible_if)]
pub fn evaluate(lesson: &Lesson, config: &EnforcementConfig) -> PromotionAction {
    // Promotion has priority over demotion: if both thresholds fire in the
    // same evaluation, we treat the failure signal as dominant — the lesson
    // is causing real damage and the escalation is the louder signal.
    if config.auto_promote && lesson.failure_count >= u64::from(config.promote_threshold) {
        if let Some(default_next) = next_up(&lesson.enforcement) {
            // Respect `suggested_enforcement` when it's stricter than the
            // natural next rung — the proposer already flagged this as a
            // hard rule and the evidence agrees.
            let target = match (
                lesson.suggested_enforcement.as_ref(),
                ladder_rank(&lesson.enforcement),
            ) {
                (Some(sugg), Some(cur_rank)) => match ladder_rank(sugg) {
                    Some(sr) if sr > cur_rank => sugg.clone(),
                    _ => default_next,
                },
                _ => default_next,
            };

            if target != lesson.enforcement {
                return PromotionAction::Promote {
                    from: lesson.enforcement.clone(),
                    to: target,
                };
            }
        }
    }

    if config.auto_demote && lesson.success_count >= u64::from(config.demote_threshold) {
        if let Some(target) = next_down(&lesson.enforcement) {
            return PromotionAction::Demote {
                from: lesson.enforcement.clone(),
                to: target,
            };
        }
    }

    PromotionAction::NoChange
}

/// Apply a decided [`PromotionAction`] in place on `lesson`.
///
/// Returns `true` if the tier changed. A no-op action returns `false`.
pub fn apply(lesson: &mut Lesson, action: &PromotionAction) -> bool {
    match action {
        PromotionAction::NoChange => false,
        PromotionAction::Promote { to, .. } | PromotionAction::Demote { to, .. } => {
            lesson.enforcement = to.clone();
            true
        }
    }
}

/// Convenience: evaluate and apply in one shot, returning the action taken.
pub fn evaluate_and_apply(lesson: &mut Lesson, config: &EnforcementConfig) -> PromotionAction {
    let action = evaluate(lesson, config);
    apply(lesson, &action);
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use thoth_core::memory::{MemoryKind, MemoryMeta};

    fn mk_lesson(enforcement: Enforcement) -> Lesson {
        Lesson {
            meta: MemoryMeta::new(MemoryKind::Reflective),
            trigger: "trig".into(),
            advice: "advice".into(),
            success_count: 0,
            failure_count: 0,
            enforcement,
            suggested_enforcement: None,
            block_message: None,
        }
    }

    #[test]
    fn no_change_when_counters_are_zero() {
        let l = mk_lesson(Enforcement::Advise);
        assert_eq!(
            evaluate(&l, &EnforcementConfig::default()),
            PromotionAction::NoChange
        );
    }

    #[test]
    fn advise_promotes_to_require_at_threshold() {
        let mut l = mk_lesson(Enforcement::Advise);
        l.failure_count = 2;
        let action = evaluate(&l, &EnforcementConfig::default());
        assert_eq!(
            action,
            PromotionAction::Promote {
                from: Enforcement::Advise,
                to: Enforcement::Require,
            }
        );
    }

    #[test]
    fn require_promotes_to_block_at_threshold() {
        let mut l = mk_lesson(Enforcement::Require);
        l.failure_count = 2;
        let action = evaluate(&l, &EnforcementConfig::default());
        assert_eq!(
            action,
            PromotionAction::Promote {
                from: Enforcement::Require,
                to: Enforcement::Block,
            }
        );
    }

    #[test]
    fn block_stays_at_block_on_further_failures() {
        let mut l = mk_lesson(Enforcement::Block);
        l.failure_count = 99;
        assert_eq!(
            evaluate(&l, &EnforcementConfig::default()),
            PromotionAction::NoChange
        );
    }

    #[test]
    fn suggested_enforcement_stricter_jumps_straight_to_it() {
        let mut l = mk_lesson(Enforcement::Advise);
        l.failure_count = 2;
        l.suggested_enforcement = Some(Enforcement::Block);
        let action = evaluate(&l, &EnforcementConfig::default());
        assert_eq!(
            action,
            PromotionAction::Promote {
                from: Enforcement::Advise,
                to: Enforcement::Block,
            }
        );
    }

    #[test]
    fn suggested_enforcement_weaker_is_ignored() {
        // Suggested = Advise but current is already Require — natural next
        // (Block) wins, we never demote via the suggested path.
        let mut l = mk_lesson(Enforcement::Require);
        l.failure_count = 2;
        l.suggested_enforcement = Some(Enforcement::Advise);
        let action = evaluate(&l, &EnforcementConfig::default());
        assert_eq!(
            action,
            PromotionAction::Promote {
                from: Enforcement::Require,
                to: Enforcement::Block,
            }
        );
    }

    #[test]
    fn block_demotes_to_require_at_threshold() {
        let mut l = mk_lesson(Enforcement::Block);
        l.success_count = 2;
        let action = evaluate(&l, &EnforcementConfig::default());
        assert_eq!(
            action,
            PromotionAction::Demote {
                from: Enforcement::Block,
                to: Enforcement::Require,
            }
        );
    }

    #[test]
    fn require_demotes_to_advise_at_threshold() {
        let mut l = mk_lesson(Enforcement::Require);
        l.success_count = 2;
        let action = evaluate(&l, &EnforcementConfig::default());
        assert_eq!(
            action,
            PromotionAction::Demote {
                from: Enforcement::Require,
                to: Enforcement::Advise,
            }
        );
    }

    #[test]
    fn advise_stays_at_advise_on_further_successes() {
        let mut l = mk_lesson(Enforcement::Advise);
        l.success_count = 99;
        assert_eq!(
            evaluate(&l, &EnforcementConfig::default()),
            PromotionAction::NoChange
        );
    }

    #[test]
    fn auto_promote_disabled_suppresses_promotion() {
        let mut l = mk_lesson(Enforcement::Advise);
        l.failure_count = 10;
        let cfg = EnforcementConfig {
            auto_promote: false,
            ..EnforcementConfig::default()
        };
        assert_eq!(evaluate(&l, &cfg), PromotionAction::NoChange);
    }

    #[test]
    fn auto_demote_disabled_suppresses_demotion() {
        let mut l = mk_lesson(Enforcement::Block);
        l.success_count = 10;
        let cfg = EnforcementConfig {
            auto_demote: false,
            ..EnforcementConfig::default()
        };
        assert_eq!(evaluate(&l, &cfg), PromotionAction::NoChange);
    }

    #[test]
    fn promotion_wins_when_both_thresholds_crossed() {
        let mut l = mk_lesson(Enforcement::Advise);
        l.failure_count = 2;
        l.success_count = 2;
        let action = evaluate(&l, &EnforcementConfig::default());
        assert!(matches!(action, PromotionAction::Promote { .. }));
    }

    #[test]
    fn require_recall_is_not_on_ladder() {
        let mut l = mk_lesson(Enforcement::RequireRecall {
            recall_within_turns: 3,
        });
        l.failure_count = 10;
        l.success_count = 10;
        assert_eq!(
            evaluate(&l, &EnforcementConfig::default()),
            PromotionAction::NoChange
        );
    }

    #[test]
    fn workflow_gate_is_not_on_ladder() {
        let mut l = mk_lesson(Enforcement::WorkflowGate);
        l.failure_count = 10;
        l.success_count = 10;
        assert_eq!(
            evaluate(&l, &EnforcementConfig::default()),
            PromotionAction::NoChange
        );
    }

    #[test]
    fn apply_mutates_lesson_on_promote() {
        let mut l = mk_lesson(Enforcement::Advise);
        l.failure_count = 2;
        let action = evaluate_and_apply(&mut l, &EnforcementConfig::default());
        assert!(matches!(action, PromotionAction::Promote { .. }));
        assert_eq!(l.enforcement, Enforcement::Require);
    }

    #[test]
    fn apply_mutates_lesson_on_demote() {
        let mut l = mk_lesson(Enforcement::Block);
        l.success_count = 2;
        let action = evaluate_and_apply(&mut l, &EnforcementConfig::default());
        assert!(matches!(action, PromotionAction::Demote { .. }));
        assert_eq!(l.enforcement, Enforcement::Require);
    }

    #[test]
    fn apply_no_change_leaves_lesson_untouched() {
        let mut l = mk_lesson(Enforcement::Advise);
        let before = l.enforcement.clone();
        let action = evaluate_and_apply(&mut l, &EnforcementConfig::default());
        assert_eq!(action, PromotionAction::NoChange);
        assert_eq!(l.enforcement, before);
    }

    #[test]
    fn threshold_boundary_minus_one_does_not_promote() {
        // Edge case per TEST-SPEC: counter == threshold - 1 → no promotion.
        let mut l = mk_lesson(Enforcement::Advise);
        l.failure_count = 1; // default threshold = 2
        assert_eq!(
            evaluate(&l, &EnforcementConfig::default()),
            PromotionAction::NoChange
        );
        assert_eq!(l.enforcement, Enforcement::Advise);
    }

    #[test]
    fn threshold_boundary_exact_promotes() {
        // Companion to the minus-one boundary: exactly at threshold must fire.
        let mut l = mk_lesson(Enforcement::Advise);
        l.failure_count = 2;
        assert!(matches!(
            evaluate(&l, &EnforcementConfig::default()),
            PromotionAction::Promote { .. }
        ));
    }

    #[test]
    fn demote_threshold_boundary_minus_one_does_not_demote() {
        let mut l = mk_lesson(Enforcement::Block);
        l.success_count = 1;
        assert_eq!(
            evaluate(&l, &EnforcementConfig::default()),
            PromotionAction::NoChange
        );
    }

    #[test]
    fn zero_threshold_promotes_on_any_failure() {
        // Degenerate config: promote_threshold=0 should fire immediately
        // since `failure_count >= 0` is always true once the lesson exists.
        let l = mk_lesson(Enforcement::Advise);
        let cfg = EnforcementConfig {
            promote_threshold: 0,
            ..EnforcementConfig::default()
        };
        assert!(matches!(
            evaluate(&l, &cfg),
            PromotionAction::Promote { .. }
        ));
    }

    #[test]
    fn suggested_equal_to_current_falls_back_to_natural_next() {
        // suggested == current rank: not strictly greater, so we take the
        // natural next rung instead of jumping (or stalling).
        let mut l = mk_lesson(Enforcement::Advise);
        l.failure_count = 2;
        l.suggested_enforcement = Some(Enforcement::Advise);
        let action = evaluate(&l, &EnforcementConfig::default());
        assert_eq!(
            action,
            PromotionAction::Promote {
                from: Enforcement::Advise,
                to: Enforcement::Require,
            }
        );
    }

    #[test]
    fn suggested_structural_tier_falls_back_to_natural_next() {
        // suggested = RequireRecall has no ladder rank; we must not promote
        // onto a structural tier via suggestion.
        let mut l = mk_lesson(Enforcement::Advise);
        l.failure_count = 2;
        l.suggested_enforcement = Some(Enforcement::RequireRecall {
            recall_within_turns: 3,
        });
        let action = evaluate(&l, &EnforcementConfig::default());
        assert_eq!(
            action,
            PromotionAction::Promote {
                from: Enforcement::Advise,
                to: Enforcement::Require,
            }
        );
    }

    #[test]
    fn idempotent_at_block_after_many_failures() {
        // Policy doc: engine is naturally idempotent once at Block.
        let mut l = mk_lesson(Enforcement::Block);
        l.failure_count = 1_000;
        for _ in 0..5 {
            let action = evaluate_and_apply(&mut l, &EnforcementConfig::default());
            assert_eq!(action, PromotionAction::NoChange);
            assert_eq!(l.enforcement, Enforcement::Block);
        }
    }

    #[test]
    fn custom_threshold_respected() {
        let mut l = mk_lesson(Enforcement::Advise);
        l.failure_count = 4;
        let cfg = EnforcementConfig {
            promote_threshold: 5,
            ..EnforcementConfig::default()
        };
        assert_eq!(evaluate(&l, &cfg), PromotionAction::NoChange);

        l.failure_count = 5;
        assert!(matches!(
            evaluate(&l, &cfg),
            PromotionAction::Promote { .. }
        ));
    }
}
