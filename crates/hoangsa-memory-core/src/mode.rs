//! Operating mode for Thoth.

use crate::provider::Synthesizer;

/// Operating mode.
///
/// `Mode::Zero` is fully offline: symbol lookup + graph + BM25 + ChromaDB.
/// `Mode::Full` adds an LLM synthesizer for curated memory and answer synthesis.
pub enum Mode {
    /// Offline-only. Symbol lookup + graph traversal + BM25 + ChromaDB.
    Zero,

    /// Plug-in mode. Supply a synthesizer for LLM-driven flows.
    Full {
        /// LLM synthesis provider. If `None`, retrieval returns raw chunks.
        synthesizer: Option<Box<dyn Synthesizer>>,
    },
}

impl Mode {
    /// Returns true if this mode can run fully offline.
    pub fn is_offline(&self) -> bool {
        matches!(self, Mode::Zero)
    }
}
