//! Pluggable provider traits for LLM synthesis.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::event::{Event, Outcome};
use crate::memory::{Fact, Lesson, Skill};
use crate::query::Chunk;

/// LLM synthesis provider — used for answer synthesis, query rewriting,
/// and self-critique.
#[async_trait]
pub trait Synthesizer: Send + Sync {
    /// Synthesize a natural-language answer from retrieved context.
    async fn synthesize(&self, prompt: &Prompt) -> Result<Synthesis>;

    /// Reflect on an outcome and optionally propose a `Lesson` to persist.
    async fn critique(&self, outcome: &Outcome) -> Result<Option<Lesson>>;

    /// End-of-session nudge (DESIGN §9): given a recent slice of the
    /// episodic log, propose any [`Fact`], [`Lesson`], or [`Skill`] that
    /// should be persisted to `MEMORY.md` / `LESSONS.md` / `skills/`.
    ///
    /// The default implementation returns nothing, so plain `critique`-only
    /// synthesizers keep working unchanged.
    async fn propose_session_memory(&self, session: &[Event]) -> Result<NudgeProposal> {
        let _ = session;
        Ok(NudgeProposal::default())
    }

    /// Optionally rewrite the user's query into a form better suited for
    /// retrieval — e.g. expanding abbreviations, HyDE-style hypothesis
    /// generation, or stripping obvious filler.
    ///
    /// Return `Ok(None)` to indicate "use the original query unchanged";
    /// this is the default, so synthesizers that don't need rewriting can
    /// ignore it. The caller must fall back to the original query when the
    /// rewrite is `None` or fails.
    async fn rewrite_query(&self, query: &str) -> Result<Option<String>> {
        let _ = query;
        Ok(None)
    }

    /// Stable identifier of the underlying model.
    fn model_id(&self) -> &str;
}

/// What the synthesizer wants to add to long-term memory at session end.
#[derive(Debug, Clone, Default)]
pub struct NudgeProposal {
    /// Declarative facts to append to `MEMORY.md`.
    pub facts: Vec<Fact>,
    /// Reflective lessons to append to `LESSONS.md`.
    pub lessons: Vec<Lesson>,
    /// Procedural skills to install under `skills/<slug>/`.
    ///
    /// Each skill's `path` should point at a source directory on disk that
    /// contains a `SKILL.md`; the memory manager copies it in place.
    pub skills: Vec<Skill>,
}

/// A synthesis prompt carrying retrieved context plus the user's question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    /// The user's question.
    pub question: String,
    /// Retrieved chunks (already reranked).
    pub chunks: Vec<Chunk>,
    /// Any lessons to inject at the top of the prompt.
    pub lessons: Vec<Lesson>,
    /// Hard token budget for the synthesized answer.
    pub max_tokens: Option<u32>,
}

/// Result of a synthesis call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Synthesis {
    /// Natural-language answer.
    pub answer: String,
    /// Chunks the model chose to cite (by id).
    pub citations: Vec<String>,
    /// Rough token usage, if the provider reports it.
    pub tokens_used: Option<u32>,
}
