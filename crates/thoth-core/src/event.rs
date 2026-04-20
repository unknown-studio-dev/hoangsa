//! Observable events — the input stream to Thoth's perception layer.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use time::OffsetDateTime;
use uuid::Uuid;

/// Identifier for a single appended event.
pub type EventId = Uuid;

/// An observation that Thoth can learn from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// A source file changed on disk.
    FileChanged {
        /// Absolute path.
        path: PathBuf,
        /// Optional commit hash this change is associated with.
        commit: Option<String>,
        /// Timestamp.
        at: OffsetDateTime,
    },
    /// A source file was deleted.
    FileDeleted {
        /// Absolute path.
        path: PathBuf,
        /// Timestamp.
        at: OffsetDateTime,
    },
    /// A query was issued by the agent or user.
    QueryIssued {
        /// Correlation id linking to future answers and outcomes.
        id: EventId,
        /// The question text.
        text: String,
        /// Timestamp.
        at: OffsetDateTime,
    },
    /// An answer was returned for an earlier query.
    AnswerReturned {
        /// Correlation id matching the originating `QueryIssued.id`.
        id: EventId,
        /// Chunk ids that were used.
        chunk_ids: Vec<String>,
        /// Whether an LLM synthesized the answer (Mode::Full).
        synthesized: bool,
        /// Timestamp.
        at: OffsetDateTime,
    },
    /// An outcome was observed.
    OutcomeObserved {
        /// Correlation id of the query whose answer produced this outcome.
        related_to: EventId,
        /// The outcome itself.
        outcome: Outcome,
        /// Timestamp.
        at: OffsetDateTime,
    },
    /// The agent expanded the `thoth.nudge` prompt. Recorded so the
    /// strict-mode gate can require a real nudge pass (not just a
    /// perfunctory recall) before mutating code.
    NudgeInvoked {
        /// Correlation id so follow-up events can tie back to the nudge.
        id: EventId,
        /// One-sentence intent the agent supplied when expanding the
        /// prompt (e.g. "refactor the retry wrapper").
        intent: String,
        /// Timestamp.
        at: OffsetDateTime,
    },
}

impl Event {
    /// Short kind label for display (e.g. `"file_changed"`).
    pub fn kind_str(&self) -> &'static str {
        match self {
            Event::FileChanged { .. } => "file_changed",
            Event::FileDeleted { .. } => "file_deleted",
            Event::QueryIssued { .. } => "query_issued",
            Event::AnswerReturned { .. } => "answer_returned",
            Event::OutcomeObserved { .. } => "outcome_observed",
            Event::NudgeInvoked { .. } => "nudge_invoked",
        }
    }

    /// One-line human-readable summary for the background review prompt.
    pub fn one_line_summary(&self) -> String {
        match self {
            Event::FileChanged { path, .. } => {
                format!("{}", path.display())
            }
            Event::FileDeleted { path, .. } => {
                format!("deleted {}", path.display())
            }
            Event::QueryIssued { text, .. } => {
                let short: String = text.chars().take(80).collect();
                format!("query: {short}")
            }
            Event::AnswerReturned {
                chunk_ids,
                synthesized,
                ..
            } => {
                format!("{} chunks, synth={}", chunk_ids.len(), synthesized,)
            }
            Event::OutcomeObserved { outcome, .. } => match outcome {
                Outcome::Test { passed, suite } => {
                    format!("test {suite}: {}", if *passed { "pass" } else { "FAIL" })
                }
                Outcome::Commit { sha, files } => {
                    format!(
                        "commit {} ({} files)",
                        &sha[..7.min(sha.len())],
                        files.len()
                    )
                }
                Outcome::Revert { sha, .. } => format!("revert {}", &sha[..7.min(sha.len())]),
                Outcome::UserFeedback { signal, note } => {
                    let s = match signal {
                        UserSignal::Accept => "accept",
                        UserSignal::Edit => "edit",
                        UserSignal::Reject => "reject",
                    };
                    match note {
                        Some(n) => format!("feedback: {s} — {n}"),
                        None => format!("feedback: {s}"),
                    }
                }
                Outcome::Error { summary, .. } => format!("error: {summary}"),
            },
            Event::NudgeInvoked { intent, .. } => {
                format!("nudge: {intent}")
            }
        }
    }
}

/// Outcome of applying an answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// Tests ran; pass/fail reported.
    Test {
        /// Whether the test suite passed.
        passed: bool,
        /// Name of the suite.
        suite: String,
    },
    /// A commit was made.
    Commit {
        /// Commit SHA.
        sha: String,
        /// Paths touched.
        files: Vec<PathBuf>,
    },
    /// A revert was issued.
    Revert {
        /// SHA that was reverted.
        sha: String,
        /// Optional reason.
        reason: Option<String>,
    },
    /// Explicit user feedback.
    UserFeedback {
        /// The signal.
        signal: UserSignal,
        /// Optional free-form note.
        note: Option<String>,
    },
    /// An error surfaced from a tool or command.
    Error {
        /// Short error summary.
        summary: String,
        /// Captured stderr or stack trace.
        detail: Option<String>,
    },
}

/// User feedback signal.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserSignal {
    /// User accepted the answer as-is.
    Accept,
    /// User edited the answer before applying.
    Edit,
    /// User rejected the answer.
    Reject,
}
