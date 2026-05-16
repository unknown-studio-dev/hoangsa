//! # hoangsa-memory-core
//!
//! Public API, traits, and core types for **Hoangsa Memory** — long-term memory for
//! coding agents.
//!
//! This crate is intentionally small: it defines the stable surface every
//! other crate in the workspace depends on (types, traits, errors) and
//! nothing more. Downstream crates (`hoangsa-memory-store`, `hoangsa-memory-retrieve`,
//! `hoangsa-memory-policy`, …) compose these types directly.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

pub mod error;
pub mod event;
pub mod io;
pub mod memory;
pub mod mode;
pub mod names;
pub mod projects;
pub mod provider;
pub mod query;

pub use error::{Error, Result};
pub use event::{Event, EventId, Outcome, UserSignal};
pub use memory::{
    Enforcement, Fact, FactScope, Lesson, LessonTrigger, MemoryKind, MemoryMeta, Preference, Skill,
};
pub use mode::Mode;
pub use names::{LESSONS_MD, MEMORY_MD, USER_MD};
pub use projects::{
    Project, REGISTRY_VERSION, Registry, RegistryError, default_hoangsa_home,
    discover_orphan_slugs, home_dir, is_populated_root, project_slug, registry_path, resolve_root,
};
pub use provider::{NudgeProposal, Prompt, Synthesis, Synthesizer};
pub use query::{
    Chunk, ChunkContext, Citation, Query, QueryScope, RenderOptions, Retrieval, RetrievalSource,
    SymbolRef,
};
