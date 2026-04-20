//! # thoth-core
//!
//! Public API, traits, and core types for **Thoth** — long-term memory for
//! coding agents.
//!
//! This crate is intentionally small: it defines the stable surface every
//! other crate in the workspace depends on (types, traits, errors) and
//! nothing more. The top-level [`CodeMemory`] façade lives in the
//! [`thoth`](https://docs.rs/thoth) umbrella crate, which wires the
//! individual stores + the retriever + the memory manager behind a single
//! entry point.
//!
//! See [`../../DESIGN.md`](../../../DESIGN.md) for the architecture document.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

pub mod error;
pub mod event;
pub mod memory;
pub mod mode;
pub mod provider;
pub mod query;

pub use error::{Error, Result};
pub use event::{Event, EventId, Outcome, UserSignal};
pub use memory::{
    Enforcement, Fact, FactScope, Lesson, LessonTrigger, MemoryKind, MemoryMeta, Skill,
};
pub use mode::Mode;
pub use provider::{NudgeProposal, Prompt, Synthesis, Synthesizer};
pub use query::{
    Chunk, ChunkContext, Citation, Query, QueryScope, RenderOptions, Retrieval, RetrievalSource,
    SymbolRef,
};
