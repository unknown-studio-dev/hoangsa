//! # thoth-memory
//!
//! The memory lifecycle layer — the "policy core" of Thoth.
//!
//! It owns:
//!
//! - The markdown source of truth (`MEMORY.md`, `LESSONS.md`, `skills/*/`)
//! - TTL-based forgetting for episodic memory
//! - Confidence evolution for lessons (reinforcement from outcomes)
//! - The **nudge** flow in Mode::Full: at session end, ask the
//!   [`Synthesizer`](thoth_core::Synthesizer) whether any new fact,
//!   lesson, or skill should be persisted
//!
//! Design goals (per `DESIGN.md` §5 and §9):
//! - Deterministic in Mode::Zero (TTL + hard delete only).
//! - LLM-curated in Mode::Full (nudge instead of algorithmic salience
//!   scoring — see Hermes).
//! - Markdown files remain first-class so a human can review in git.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

pub mod cap;
pub mod config;
pub mod lesson_matcher;
pub mod manager;
pub mod rules;
pub mod text_sim;
pub mod working;

// Re-export the most commonly used items at the crate root.
pub use cap::{
    CapExceededError, ContentPolicyError, GuardedAppendError, MarkdownStoreMemoryExt,
    MemoryEntryPreview, MemoryKind, check_content_policy,
};
pub use config::{
    ActorPolicyConfig, DisciplineConfig, EnforcementConfig, MemoryConfig, gate_defaults,
};
pub use manager::{ForgetReport, MemoryManager, NudgeReport};
pub use working::{WorkingMemory, WorkingNote};

use time::OffsetDateTime;

/// Effective retention score per DESIGN §9:
/// `salience · exp(-λ·days_idle) · ln(e + access_count)`.
///
/// Using `ln(e + access_count)` instead of the literal `ln(1 + …)` from
/// the design text so that a fresh item (access_count = 0) evaluates to
/// `salience · decay`, i.e. time-scaled salience — not zero. Retrieved
/// items then earn a multiplicative boost of `ln(e + N) ≥ 1`.
pub fn effective_retention_score(
    salience: f32,
    access_count: u64,
    last_accessed_at: OffsetDateTime,
    now: OffsetDateTime,
    lambda: f32,
) -> f32 {
    let days_idle = ((now - last_accessed_at).whole_seconds() as f64 / 86_400.0).max(0.0);
    let decay = (-(lambda as f64) * days_idle).exp() as f32;
    let usage = (std::f64::consts::E + access_count as f64).ln() as f32;
    salience * decay * usage
}
