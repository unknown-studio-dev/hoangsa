//! Error and result types for Thoth.

use thiserror::Error;

/// Thoth's result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Top-level error enum. Library crates convert into this.
#[derive(Debug, Error)]
pub enum Error {
    /// Configuration invalid or missing.
    #[error("config error: {0}")]
    Config(String),

    /// A provider (embedder / synthesizer) returned an error.
    #[error("provider error: {0}")]
    Provider(String),

    /// A store (redb / lance / tantivy / sqlite / fs) returned an error.
    #[error("store error: {0}")]
    Store(String),

    /// Parsing a source file failed.
    #[error("parse error: {0}")]
    Parse(String),

    /// IO error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// JSON (de)serialization.
    #[error("serde_json: {0}")]
    Json(#[from] serde_json::Error),

    /// Anything else, escaped through `anyhow`.
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}
