//! Memory kinds and their metadata.
//!
//! See `DESIGN.md` §5 for the full taxonomy.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

pub use enforcement::{Enforcement, LessonTrigger};

/// The five kinds of memory Thoth tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// In-process, session-scoped scratchpad.
    Working,
    /// Facts derived from the code itself (symbols, graph edges, ...).
    Semantic,
    /// Append-only log of queries, answers, and outcomes.
    Episodic,
    /// Reusable skill / playbook stored on disk.
    Procedural,
    /// Lesson learned from a past mistake.
    Reflective,
}

/// Universal metadata attached to every memory record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMeta {
    /// Globally unique id.
    pub id: Uuid,
    /// Which kind of memory.
    pub kind: MemoryKind,
    /// Creation timestamp.
    pub created_at: OffsetDateTime,
    /// Last access timestamp (for decay).
    pub last_accessed_at: OffsetDateTime,
    /// How many times this has been retrieved.
    pub access_count: u64,
    /// Salience in `[0.0, 1.0]` — how important.
    pub salience: f32,
    /// Confidence in `[0.0, 1.0]` — for lessons / skills.
    pub confidence: f32,
    /// Optional TTL in seconds.
    pub ttl_seconds: Option<u64>,
    /// Upstream source events that produced this memory.
    pub sources: Vec<Uuid>,
    /// Memories superseded by this one (chain of evolution).
    pub supersedes: Option<Uuid>,
    /// Memories contradicted by this one.
    pub contradicts: Vec<Uuid>,
}

impl MemoryMeta {
    /// Construct a fresh metadata record for the given kind.
    pub fn new(kind: MemoryKind) -> Self {
        let now = OffsetDateTime::now_utc();
        Self {
            id: Uuid::new_v4(),
            kind,
            created_at: now,
            last_accessed_at: now,
            access_count: 0,
            salience: 0.5,
            confidence: 0.5,
            ttl_seconds: None,
            sources: Vec::new(),
            supersedes: None,
            contradicts: Vec::new(),
        }
    }
}

/// Whether a fact is always injected at session start or only retrieved
/// on demand. Inspired by MemPalace's L0/L1 layered memory stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FactScope {
    /// Always included in `thoth_wakeup` output — core identity / essential context.
    #[default]
    Always,
    /// Only surfaced when a `thoth_recall` query matches.
    OnDemand,
}

/// A fact recorded in `MEMORY.md` (or its derived index).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    /// Metadata.
    pub meta: MemoryMeta,
    /// Human-readable text of the fact.
    pub text: String,
    /// Optional tags for filtering.
    pub tags: Vec<String>,
    /// Whether this fact is always injected or only on-demand.
    #[serde(default)]
    pub scope: FactScope,
}

/// A lesson learned from a mistake, stored in `LESSONS.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    /// Metadata.
    pub meta: MemoryMeta,
    /// Trigger pattern — when this lesson should be injected as context.
    pub trigger: String,
    /// The advice / rule / warning itself.
    pub advice: String,
    /// How many retrievals this lesson has been helpful on.
    pub success_count: u64,
    /// How many retrievals this lesson has hurt on.
    pub failure_count: u64,
    /// Actual enforcement tier applied at runtime. Per REQ-03, newly created
    /// lessons always start at [`Enforcement::Advise`] regardless of
    /// `suggested_enforcement`; promotion happens via evidence-driven
    /// auto-promote in the outcome harvester.
    ///
    /// Defaults to [`Enforcement::Advise`] for backwards compat with lessons
    /// serialized before this field existed.
    #[serde(default)]
    pub enforcement: Enforcement,
    /// Tier Claude suggested at `thoth_remember_lesson` time — audit-only,
    /// NOT applied. Stored so the curator can later see what the proposer
    /// thought and validate against violation evidence.
    #[serde(default)]
    pub suggested_enforcement: Option<Enforcement>,
    /// Message shown to Claude Code via stderr when this lesson blocks a
    /// tool call (only used when `enforcement == Block`).
    #[serde(default)]
    pub block_message: Option<String>,
}

/// A procedural skill — stored as a directory under `.thoth/skills/`,
/// compatible with the `agentskills.io` standard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Metadata.
    pub meta: MemoryMeta,
    /// Human-readable slug (e.g. `auth-jwt-pattern`).
    pub slug: String,
    /// One-line description (from SKILL.md frontmatter).
    pub description: String,
    /// Relative path to the skill directory inside `.thoth/skills/`.
    pub path: std::path::PathBuf,
}

#[cfg(test)]
pub mod lesson {
    //! Lesson-related test modules, grouped by concern. Tests for extended
    //! Lesson fields (enforcement, suggested_enforcement, block_message) and
    //! their backwards-compat serde behavior.

    #[cfg(test)]
    pub mod extended_fields {
        //! REQ-03 coverage: enforcement / suggested_enforcement / block_message.
        use super::super::*;

        fn base_lesson() -> Lesson {
            Lesson {
                meta: MemoryMeta::new(MemoryKind::Reflective),
                trigger: "edit migrations".into(),
                advice: "run the migration lint first".into(),
                success_count: 0,
                failure_count: 0,
                enforcement: Enforcement::default(),
                suggested_enforcement: None,
                block_message: None,
            }
        }

        #[test]
        fn defaults_are_advise_and_none() {
            let l = base_lesson();
            assert_eq!(l.enforcement, Enforcement::Advise);
            assert!(l.suggested_enforcement.is_none());
            assert!(l.block_message.is_none());
        }

        #[test]
        fn new_fields_roundtrip() {
            let mut l = base_lesson();
            l.enforcement = Enforcement::Block;
            l.suggested_enforcement = Some(Enforcement::Block);
            l.block_message = Some("never edit migrations directly".into());

            let json = serde_json::to_string(&l).unwrap();
            let back: Lesson = serde_json::from_str(&json).unwrap();

            assert_eq!(back.enforcement, Enforcement::Block);
            assert_eq!(back.suggested_enforcement, Some(Enforcement::Block));
            assert_eq!(
                back.block_message.as_deref(),
                Some("never edit migrations directly")
            );
        }

        #[test]
        fn legacy_lesson_without_new_fields_loads() {
            // Emulate a lesson serialized before the enforcement fields existed
            // — only legacy keys present.
            let meta = MemoryMeta::new(MemoryKind::Reflective);
            let meta_json = serde_json::to_value(&meta).unwrap();
            let legacy = serde_json::json!({
                "meta": meta_json,
                "trigger": "legacy trigger",
                "advice": "legacy advice",
                "success_count": 1,
                "failure_count": 0,
            });
            let l: Lesson = serde_json::from_value(legacy).unwrap();
            assert_eq!(l.enforcement, Enforcement::Advise);
            assert!(l.suggested_enforcement.is_none());
            assert!(l.block_message.is_none());
            assert_eq!(l.trigger, "legacy trigger");
        }

        #[test]
        fn suggested_enforcement_is_audit_only() {
            // The struct itself doesn't enforce REQ-03 (that's server-side
            // policy in thoth_remember_lesson), but we verify suggested can
            // differ from actual without error.
            let mut l = base_lesson();
            l.enforcement = Enforcement::Advise;
            l.suggested_enforcement = Some(Enforcement::Block);
            let json = serde_json::to_string(&l).unwrap();
            let back: Lesson = serde_json::from_str(&json).unwrap();
            assert_eq!(back.enforcement, Enforcement::Advise);
            assert_eq!(back.suggested_enforcement, Some(Enforcement::Block));
        }
    }
}

/// Enforcement tier + structured lesson trigger.
///
/// See `DESIGN-SPEC.md` REQ-01 / REQ-02.
pub mod enforcement {
    use serde::{Deserialize, Serialize};

    /// How a lesson or rule is enforced against tool calls.
    ///
    /// Five tiers, in escalating order of strictness:
    /// - [`Enforcement::Advise`] — banner inject only (default, backwards compat).
    /// - [`Enforcement::Require`] — PreToolUse injects lesson body into tool call context.
    /// - [`Enforcement::Block`] — PreToolUse exits 2 with `block_message`.
    /// - [`Enforcement::RequireRecall`] — exit 2 unless a matching `thoth_recall`
    ///   event occurred within `recall_within_turns`.
    /// - [`Enforcement::WorkflowGate`] — exit 2 if tool call diverges from an
    ///   active workflow's expected next step.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    #[serde(rename_all = "PascalCase")]
    pub enum Enforcement {
        /// Text inject into SessionStart banner only (current default).
        #[default]
        Advise,
        /// Force-inject lesson into tool call context via `<lesson-must-apply>`.
        Require,
        /// Hard block — Claude Code must stop.
        Block,
        /// Exit 2 unless `gate.jsonl` has a matching recall within the window.
        RequireRecall {
            /// How many turns back to look for a matching `thoth_recall` event.
            recall_within_turns: u32,
        },
        /// Exit 2 if tool call doesn't match expected workflow step.
        WorkflowGate,
    }

    /// Structured trigger describing when a lesson should fire.
    ///
    /// All structured fields are optional — a `None` is a wildcard for that
    /// dimension. The `natural` field is always required and is what gets
    /// rendered in `LESSONS.md` for human consumption.
    ///
    /// Use [`LessonTrigger::natural_only`] for legacy / text-only lessons that
    /// predate structured triggers.
    #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct LessonTrigger {
        /// Tool name filter — `"Edit"`, `"Write"`, `"Bash"`, `"Any"`, etc.
        /// `None` matches any tool.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tool: Option<String>,
        /// Glob for the file path argument (applies to Edit / Write / Read).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub path_glob: Option<String>,
        /// Regex for the Bash command string.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cmd_regex: Option<String>,
        /// Regex for Edit content (either `old_string` or `new_string`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub content_regex: Option<String>,
        /// Human-readable trigger description — always required.
        pub natural: String,
    }

    impl LessonTrigger {
        /// Construct a legacy-style trigger with only a natural-language
        /// description and no structured matchers. Matches nothing
        /// structurally; relies on text recall only.
        pub fn natural_only(text: impl Into<String>) -> Self {
            Self {
                natural: text.into(),
                ..Default::default()
            }
        }

        /// Returns `true` if this trigger carries at least one structured
        /// matcher (tool / path_glob / cmd_regex / content_regex). Used by
        /// loaders / migration to distinguish legacy lessons.
        pub fn is_structured(&self) -> bool {
            self.tool.is_some()
                || self.path_glob.is_some()
                || self.cmd_regex.is_some()
                || self.content_regex.is_some()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn enforcement_default_is_advise() {
            assert_eq!(Enforcement::default(), Enforcement::Advise);
        }

        #[test]
        fn enforcement_advise_roundtrip() {
            let json = serde_json::to_string(&Enforcement::Advise).unwrap();
            assert_eq!(json, "\"Advise\"");
            let back: Enforcement = serde_json::from_str(&json).unwrap();
            assert_eq!(back, Enforcement::Advise);
        }

        #[test]
        fn enforcement_require_roundtrip() {
            let json = serde_json::to_string(&Enforcement::Require).unwrap();
            assert_eq!(json, "\"Require\"");
            let back: Enforcement = serde_json::from_str(&json).unwrap();
            assert_eq!(back, Enforcement::Require);
        }

        #[test]
        fn enforcement_block_roundtrip() {
            let json = serde_json::to_string(&Enforcement::Block).unwrap();
            assert_eq!(json, "\"Block\"");
            let back: Enforcement = serde_json::from_str(&json).unwrap();
            assert_eq!(back, Enforcement::Block);
        }

        #[test]
        fn enforcement_require_recall_roundtrip() {
            let e = Enforcement::RequireRecall {
                recall_within_turns: 3,
            };
            let json = serde_json::to_string(&e).unwrap();
            let back: Enforcement = serde_json::from_str(&json).unwrap();
            assert_eq!(back, e);
        }

        #[test]
        fn enforcement_workflow_gate_roundtrip() {
            let json = serde_json::to_string(&Enforcement::WorkflowGate).unwrap();
            assert_eq!(json, "\"WorkflowGate\"");
            let back: Enforcement = serde_json::from_str(&json).unwrap();
            assert_eq!(back, Enforcement::WorkflowGate);
        }

        #[test]
        fn all_five_variants_roundtrip() {
            let variants = vec![
                Enforcement::Advise,
                Enforcement::Require,
                Enforcement::Block,
                Enforcement::RequireRecall {
                    recall_within_turns: 5,
                },
                Enforcement::WorkflowGate,
            ];
            for v in variants {
                let json = serde_json::to_string(&v).unwrap();
                let back: Enforcement = serde_json::from_str(&json).unwrap();
                assert_eq!(back, v);
            }
        }

        #[test]
        fn lesson_trigger_natural_only_is_unstructured() {
            let t = LessonTrigger::natural_only("don't edit migrations");
            assert_eq!(t.natural, "don't edit migrations");
            assert!(t.tool.is_none());
            assert!(t.path_glob.is_none());
            assert!(t.cmd_regex.is_none());
            assert!(t.content_regex.is_none());
            assert!(!t.is_structured());
        }

        #[test]
        fn lesson_trigger_default_is_empty() {
            let t = LessonTrigger::default();
            assert_eq!(t.natural, "");
            assert!(!t.is_structured());
        }

        #[test]
        fn lesson_trigger_is_structured_when_tool_set() {
            let t = LessonTrigger {
                tool: Some("Edit".into()),
                natural: "edit guard".into(),
                ..Default::default()
            };
            assert!(t.is_structured());
        }

        #[test]
        fn lesson_trigger_roundtrip_full() {
            let t = LessonTrigger {
                tool: Some("Bash".into()),
                path_glob: None,
                cmd_regex: Some(r"^rm\s+-rf\s+/".into()),
                content_regex: None,
                natural: "rm -rf root".into(),
            };
            let json = serde_json::to_string(&t).unwrap();
            let back: LessonTrigger = serde_json::from_str(&json).unwrap();
            assert_eq!(back, t);
        }

        #[test]
        fn lesson_trigger_roundtrip_minimal_skips_none() {
            let t = LessonTrigger::natural_only("hello");
            let json = serde_json::to_string(&t).unwrap();
            // Only `natural` is serialized; None fields are skipped.
            assert_eq!(json, r#"{"natural":"hello"}"#);
            let back: LessonTrigger = serde_json::from_str(&json).unwrap();
            assert_eq!(back, t);
        }

        #[test]
        fn lesson_trigger_deserializes_missing_fields() {
            let json = r#"{"natural":"legacy"}"#;
            let t: LessonTrigger = serde_json::from_str(json).unwrap();
            assert_eq!(t, LessonTrigger::natural_only("legacy"));
        }

        // --- T-24 gap coverage ----------------------------------------------

        /// `RequireRecall` must serialize with its internal
        /// `recall_within_turns` field under the PascalCase tag name.
        #[test]
        fn enforcement_require_recall_json_shape() {
            let e = Enforcement::RequireRecall {
                recall_within_turns: 7,
            };
            let json = serde_json::to_string(&e).unwrap();
            assert_eq!(json, r#"{"RequireRecall":{"recall_within_turns":7}}"#);
        }

        /// Unknown variants must fail to deserialize — guards against typos
        /// in on-disk rules files silently demoting to a default tier.
        #[test]
        fn enforcement_unknown_variant_rejected() {
            let bad = r#""Nope""#;
            let err = serde_json::from_str::<Enforcement>(bad);
            assert!(err.is_err(), "unknown variant must error, got {:?}", err);
        }

        /// Lowercase variant must fail — tag is strictly PascalCase.
        #[test]
        fn enforcement_lowercase_variant_rejected() {
            let bad = r#""advise""#;
            let err = serde_json::from_str::<Enforcement>(bad);
            assert!(err.is_err(), "lowercase must error, got {:?}", err);
        }

        /// Two `RequireRecall`s with different windows must not compare equal.
        #[test]
        fn enforcement_require_recall_window_distinguishes() {
            let a = Enforcement::RequireRecall {
                recall_within_turns: 3,
            };
            let b = Enforcement::RequireRecall {
                recall_within_turns: 5,
            };
            assert_ne!(a, b);
        }

        /// A trigger that sets every structured matcher must report
        /// `is_structured()` and round-trip all fields.
        #[test]
        fn lesson_trigger_all_fields_structured_and_roundtrip() {
            let t = LessonTrigger {
                tool: Some("Edit".into()),
                path_glob: Some("**/migrations/*.rs".into()),
                cmd_regex: Some(r"drop\s+table".into()),
                content_regex: Some(r"TODO".into()),
                natural: "everything set".into(),
            };
            assert!(t.is_structured());
            let json = serde_json::to_string(&t).unwrap();
            let back: LessonTrigger = serde_json::from_str(&json).unwrap();
            assert_eq!(back, t);
        }

        /// `natural` is a required field — serde must error if it is missing.
        #[test]
        fn lesson_trigger_missing_natural_rejected() {
            let bad = r#"{"tool":"Edit"}"#;
            let err = serde_json::from_str::<LessonTrigger>(bad);
            assert!(err.is_err(), "missing natural must error, got {:?}", err);
        }

        /// `is_structured()` flips to true the moment *any* single matcher
        /// field is set — smoke test each field independently.
        #[test]
        fn lesson_trigger_any_single_field_is_structured() {
            let mk = |mut f: LessonTrigger| {
                f.natural = "n".into();
                f
            };
            assert!(
                mk(LessonTrigger {
                    path_glob: Some("*".into()),
                    ..Default::default()
                })
                .is_structured()
            );
            assert!(
                mk(LessonTrigger {
                    cmd_regex: Some("x".into()),
                    ..Default::default()
                })
                .is_structured()
            );
            assert!(
                mk(LessonTrigger {
                    content_regex: Some("x".into()),
                    ..Default::default()
                })
                .is_structured()
            );
        }

        /// A full `Lesson` carrying `RequireRecall` enforcement must survive
        /// a JSON round-trip with its inner window intact (regression guard
        /// for the tagged-enum #[serde(default)] path).
        #[test]
        fn lesson_roundtrip_with_require_recall_enforcement() {
            use super::super::{Lesson, MemoryKind, MemoryMeta};
            let l = Lesson {
                meta: MemoryMeta::new(MemoryKind::Reflective),
                trigger: "t".into(),
                advice: "a".into(),
                success_count: 0,
                failure_count: 0,
                enforcement: Enforcement::RequireRecall {
                    recall_within_turns: 4,
                },
                suggested_enforcement: None,
                block_message: None,
            };
            let json = serde_json::to_string(&l).unwrap();
            let back: Lesson = serde_json::from_str(&json).unwrap();
            assert_eq!(
                back.enforcement,
                Enforcement::RequireRecall {
                    recall_within_turns: 4
                }
            );
        }
    }
}
