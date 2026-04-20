//! Synthesizer adapters for thoth-retrieve.
//!
//! Enable providers via Cargo features (e.g. `anthropic`).

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "anthropic")]
pub use anthropic::AnthropicSynthesizer;
