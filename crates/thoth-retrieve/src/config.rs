//! User-facing indexer config.
//!
//! Loaded from `<root>/config.toml` under the `[index]` table. Anything the
//! user can tweak without touching code lives here — most importantly the
//! `ignore` list, so users can exclude build outputs, vendored trees, or
//! generated code from retrieval without writing Rust.
//!
//! Example:
//!
//! ```toml
//! [index]
//! ignore = [
//!     "target/",
//!     "node_modules/",
//!     "*.generated.rs",
//!     "docs/internal/",
//! ]
//! # Optional: raise/lower the max file size (bytes). 2 MiB by default.
//! max_file_size = 2_097_152
//! # Optional: recurse into dotfile dirs. Off by default.
//! include_hidden = false
//! ```
//!
//! Unknown keys are rejected (so typos surface loudly). If the file is
//! missing, malformed, or has no `[index]` table, the defaults kick in.
//!
//! The config is read by the CLI / library entrypoint and wired into the
//! [`Indexer`](crate::Indexer) via [`Indexer::with_ignore_patterns`] and the
//! walker's [`WalkOptions`](thoth_parse::walk::WalkOptions).

use std::path::Path;

use serde::Deserialize;

/// Indexer-side config. Mirrors the `[index]` table in `<root>/config.toml`.
///
/// See the module docs for an annotated example.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IndexConfig {
    /// Gitignore-syntax patterns to exclude from indexing. Applied on top
    /// of `.gitignore`, `.ignore`, and `.thothignore`. Supports negation
    /// with a leading `!`.
    pub ignore: Vec<String>,
    /// Max file size (bytes) considered for indexing. Files larger than
    /// this are skipped with a `debug!` line. Default: 2 MiB.
    pub max_file_size: u64,
    /// Whether to descend into hidden (dotfile) directories. Default: off.
    pub include_hidden: bool,
    /// Whether to follow symlinks. Default: off — symlinks into other
    /// projects would otherwise balloon the index.
    pub follow_symlinks: bool,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            ignore: Vec::new(),
            max_file_size: 2 * 1024 * 1024,
            include_hidden: false,
            follow_symlinks: false,
        }
    }
}

/// Retrieval-output display limits. Mirrors the `[output]` table in
/// `<root>/config.toml`. Bounds the per-chunk body length and the total
/// rendered size of a recall, and sets the threshold above which
/// `thoth_impact` switches from a per-node listing to a file-grouped
/// summary.
///
/// The values here feed [`thoth_core::RenderOptions`] via
/// [`Self::render_options`]. The underlying [`thoth_core::Retrieval`] is
/// never truncated — only the text surface honours these caps, so
/// structured JSON (CLI `--json`, MCP `data`) still sees every chunk.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputConfig {
    /// Maximum body lines rendered per recall chunk. `0` disables
    /// truncation. Default: 200.
    pub max_body_lines: usize,
    /// Soft cap on total rendered bytes per recall. `0` disables the
    /// size budget. Default: 32 KiB.
    pub max_total_bytes: usize,
    /// Node count above which `thoth_impact` groups results by file
    /// rather than listing every node. `0` disables grouping. Default: 50.
    pub impact_group_threshold: usize,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            max_body_lines: 200,
            max_total_bytes: 32 * 1024,
            impact_group_threshold: 50,
        }
    }
}

impl OutputConfig {
    /// Load `<root>/config.toml` if it exists, returning the `[output]`
    /// table (or [`Self::default`] if the file / table are missing).
    ///
    /// A malformed file emits a `warn!` and falls back to defaults —
    /// a typo in one output key must not turn recall into a broken
    /// wall of JSON.
    pub async fn load_or_default(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "output: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        match toml::from_str::<ConfigFile>(&text) {
            Ok(cf) => cf.output.unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "output: config.toml parse error, using defaults");
                Self::default()
            }
        }
    }

    /// Convert to the render-time options consumed by
    /// [`thoth_core::Retrieval::render_with`]. The `impact_group_threshold`
    /// is used directly by `thoth_impact`, not via `RenderOptions`.
    pub fn render_options(&self) -> thoth_core::RenderOptions {
        thoth_core::RenderOptions {
            max_body_lines: self.max_body_lines,
            max_total_bytes: self.max_total_bytes,
        }
    }
}

/// Retrieval-time rerank tuning. Mirrors the `[retrieve]` table in
/// `<root>/config.toml`.
///
/// Only one knob today: `rerank_markdown_boost` multiplies the fused
/// score of every Markdown-sourced chunk (MEMORY.md / LESSONS.md).
/// Values > 1.0 lift facts/lessons over code for the same query — useful
/// when a lesson matches on prose but loses the rank to a symbol stage
/// hit with a literal identifier overlap. Values < 1.0 push markdown
/// down; 0.0 effectively hides it. Default 1.0 (no-op).
///
/// Example:
///
/// ```toml
/// [retrieve]
/// rerank_markdown_boost = 1.8  # prefer lessons/facts for prose queries
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetrieveConfig {
    /// Post-fusion multiplier applied to the score of every
    /// `RetrievalSource::Markdown` hit. Default `1.0` (disabled).
    /// Clamped into `[0.0, 10.0]` at load time so a typo (`18.0`
    /// instead of `1.8`) cannot make one markdown line outrank
    /// every code hit.
    pub rerank_markdown_boost: f32,
}

impl Default for RetrieveConfig {
    fn default() -> Self {
        Self {
            rerank_markdown_boost: 1.0,
        }
    }
}

impl RetrieveConfig {
    /// Load `<root>/config.toml` if it exists, returning the `[retrieve]`
    /// table (or [`Self::default`] if the file / table are missing).
    ///
    /// Tolerant, like [`OutputConfig::load_or_default`]: missing or
    /// malformed file → warn + defaults. The returned
    /// `rerank_markdown_boost` is clamped into `[0.0, 10.0]`.
    pub async fn load_or_default(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "retrieve: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        let raw = match toml::from_str::<ConfigFile>(&text) {
            Ok(cf) => cf.retrieve.unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "retrieve: config.toml parse error, using defaults");
                Self::default()
            }
        };
        Self {
            rerank_markdown_boost: raw.rerank_markdown_boost.clamp(0.0, 10.0),
        }
    }
}

/// Background file-watcher config. Mirrors the `[watch]` table in
/// `<root>/config.toml`.
///
/// When `enabled = true`, the MCP server spawns a background file watcher
/// on startup so source changes are reindexed automatically — no separate
/// `thoth watch` process required. Since the MCP daemon already holds the
/// redb exclusive lock, the watcher runs in-process and shares the same
/// `Indexer` instance.
///
/// Example:
///
/// ```toml
/// [watch]
/// enabled = true
/// debounce_ms = 300
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WatchConfig {
    /// Whether to auto-watch the project source tree. Default: `false`.
    pub enabled: bool,
    /// Debounce window in milliseconds. Events arriving within this window
    /// after the first change are batched into a single reindex pass.
    /// Default: 300.
    pub debounce_ms: u64,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            debounce_ms: 300,
        }
    }
}

impl WatchConfig {
    /// Load `<root>/config.toml` if it exists, returning the `[watch]`
    /// table (or [`Self::default`] if the file / table are missing).
    pub async fn load_or_default(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "watch: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        match toml::from_str::<ConfigFile>(&text) {
            Ok(cf) => cf.watch.unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "watch: config.toml parse error, using defaults");
                Self::default()
            }
        }
    }
}

/// ChromaDB integration config (`[chroma]` in `config.toml`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ChromaConfig {
    /// Enable ChromaDB semantic search. Default `false`.
    pub enabled: bool,
    /// Custom ChromaDB data path. When `None`, falls back to
    /// `StoreRoot::chroma_path()`.
    pub data_path: Option<String>,
}

impl Default for ChromaConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            data_path: None,
        }
    }
}

impl ChromaConfig {
    /// Load `<root>/config.toml` if it exists, returning the `[chroma]`
    /// table (or [`Self::default`] if the file / table are missing).
    pub async fn load_or_default(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "chroma: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        match toml::from_str::<ConfigFile>(&text) {
            Ok(cf) => cf.chroma.unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "chroma: config.toml parse error, using defaults");
                Self::default()
            }
        }
    }

    /// Sync twin of [`Self::load_or_default`].
    pub fn load_or_default_sync(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "chroma: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        match toml::from_str::<ConfigFile>(&text) {
            Ok(cf) => cf.chroma.unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "chroma: config.toml parse error, using defaults");
                Self::default()
            }
        }
    }
}

/// TOML file schema — the outer document. We only care about `[index]`,
/// `[output]`, `[retrieve]`, `[watch]`, and `[chroma]`; other tables
/// (e.g. `[memory]`, read by `thoth-memory`) are left to their owners,
/// so we tolerate them instead of `deny_unknown_fields` here.
#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    index: Option<IndexConfig>,
    #[serde(default)]
    output: Option<OutputConfig>,
    #[serde(default)]
    retrieve: Option<RetrieveConfig>,
    #[serde(default)]
    watch: Option<WatchConfig>,
    #[serde(default)]
    chroma: Option<ChromaConfig>,
}

impl IndexConfig {
    /// Load `<root>/config.toml` if it exists, returning the `[index]`
    /// table (or [`Self::default`] if the file / table are missing).
    ///
    /// A malformed file emits a `warn!` and falls back to defaults — the
    /// user's index must not become unusable because they mistyped one key.
    pub async fn load_or_default(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "index: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        match toml::from_str::<ConfigFile>(&text) {
            Ok(cf) => cf.index.unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "index: config.toml parse error, using defaults");
                Self::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn parses_index_table() {
        let dir = tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("config.toml"),
            r#"
            [index]
            ignore = ["target/", "*.bak"]
            include_hidden = true
            "#,
        )
        .await
        .unwrap();

        let cfg = IndexConfig::load_or_default(dir.path()).await;
        assert_eq!(cfg.ignore, vec!["target/".to_string(), "*.bak".to_string()]);
        assert!(cfg.include_hidden);
        assert!(!cfg.follow_symlinks);
        assert_eq!(cfg.max_file_size, 2 * 1024 * 1024);
    }

    #[tokio::test]
    async fn missing_file_falls_back_to_defaults() {
        let dir = tempdir().unwrap();
        let cfg = IndexConfig::load_or_default(dir.path()).await;
        assert!(cfg.ignore.is_empty());
        assert!(!cfg.include_hidden);
    }

    #[tokio::test]
    async fn other_tables_are_tolerated() {
        // `[memory]` belongs to thoth-memory; presence here must not break
        // the index loader. `deny_unknown_fields` is inside IndexConfig, not
        // at the top level.
        let dir = tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("config.toml"),
            r#"
            [memory]
            episodic_ttl_days = 7

            [index]
            ignore = ["dist/"]
            "#,
        )
        .await
        .unwrap();
        let cfg = IndexConfig::load_or_default(dir.path()).await;
        assert_eq!(cfg.ignore, vec!["dist/".to_string()]);
    }

    #[tokio::test]
    async fn malformed_file_falls_back_to_defaults() {
        let dir = tempdir().unwrap();
        tokio::fs::write(dir.path().join("config.toml"), "this is not = toml ][[")
            .await
            .unwrap();
        let cfg = IndexConfig::load_or_default(dir.path()).await;
        // Defaults apply — load_or_default must not panic.
        assert!(cfg.ignore.is_empty());
    }

    #[tokio::test]
    async fn output_config_defaults() {
        let dir = tempdir().unwrap();
        let cfg = OutputConfig::load_or_default(dir.path()).await;
        assert_eq!(cfg.max_body_lines, 200);
        assert_eq!(cfg.max_total_bytes, 32 * 1024);
        assert_eq!(cfg.impact_group_threshold, 50);
        let opts = cfg.render_options();
        assert_eq!(opts.max_body_lines, 200);
        assert_eq!(opts.max_total_bytes, 32 * 1024);
    }

    #[tokio::test]
    async fn output_config_parses_all_keys() {
        let dir = tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("config.toml"),
            r#"
            [output]
            max_body_lines = 100
            max_total_bytes = 8192
            impact_group_threshold = 25
            "#,
        )
        .await
        .unwrap();
        let cfg = OutputConfig::load_or_default(dir.path()).await;
        assert_eq!(cfg.max_body_lines, 100);
        assert_eq!(cfg.max_total_bytes, 8192);
        assert_eq!(cfg.impact_group_threshold, 25);
    }

    #[tokio::test]
    async fn output_config_and_index_config_coexist() {
        // Both tables in one file — each loader ignores the other's
        // table, so an `[output]` typo doesn't break indexing and
        // vice-versa.
        let dir = tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("config.toml"),
            r#"
            [index]
            ignore = ["dist/"]

            [output]
            max_body_lines = 64
            "#,
        )
        .await
        .unwrap();
        let idx = IndexConfig::load_or_default(dir.path()).await;
        let out = OutputConfig::load_or_default(dir.path()).await;
        assert_eq!(idx.ignore, vec!["dist/".to_string()]);
        assert_eq!(out.max_body_lines, 64);
        // Defaults preserved for keys the user didn't override.
        assert_eq!(out.max_total_bytes, 32 * 1024);
    }

    #[tokio::test]
    async fn output_config_malformed_falls_back_to_defaults() {
        let dir = tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("config.toml"),
            r#"
            [output]
            max_body_lines = "not a number"
            "#,
        )
        .await
        .unwrap();
        let cfg = OutputConfig::load_or_default(dir.path()).await;
        assert_eq!(cfg.max_body_lines, 200);
    }

    #[tokio::test]
    async fn retrieve_config_defaults_to_unit_boost() {
        let dir = tempdir().unwrap();
        let cfg = RetrieveConfig::load_or_default(dir.path()).await;
        assert!((cfg.rerank_markdown_boost - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn retrieve_config_parses_markdown_boost() {
        let dir = tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("config.toml"),
            r#"
            [retrieve]
            rerank_markdown_boost = 2.5
            "#,
        )
        .await
        .unwrap();
        let cfg = RetrieveConfig::load_or_default(dir.path()).await;
        assert!((cfg.rerank_markdown_boost - 2.5).abs() < 1e-6);
    }

    #[tokio::test]
    async fn retrieve_config_clamps_runaway_boost() {
        // User mistypes 18.0 instead of 1.8 — clamp stops one markdown
        // line from shadowing the entire code corpus.
        let dir = tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("config.toml"),
            r#"
            [retrieve]
            rerank_markdown_boost = 18.0
            "#,
        )
        .await
        .unwrap();
        let cfg = RetrieveConfig::load_or_default(dir.path()).await;
        assert!((cfg.rerank_markdown_boost - 10.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn retrieve_config_rejects_unknown_keys() {
        // Typos in `[retrieve]` keys must surface (via warn! + defaults)
        // rather than silently ignoring the user's intent.
        let dir = tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("config.toml"),
            r#"
            [retrieve]
            rerank_markdown_boos = 2.0   # typo: boos
            "#,
        )
        .await
        .unwrap();
        let cfg = RetrieveConfig::load_or_default(dir.path()).await;
        // Parse failed → default (1.0), not the intended 2.0.
        assert!((cfg.rerank_markdown_boost - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn retrieve_output_and_index_coexist() {
        let dir = tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("config.toml"),
            r#"
            [index]
            ignore = ["dist/"]

            [output]
            max_body_lines = 80

            [retrieve]
            rerank_markdown_boost = 1.6
            "#,
        )
        .await
        .unwrap();
        let idx = IndexConfig::load_or_default(dir.path()).await;
        let out = OutputConfig::load_or_default(dir.path()).await;
        let ret = RetrieveConfig::load_or_default(dir.path()).await;
        assert_eq!(idx.ignore, vec!["dist/".to_string()]);
        assert_eq!(out.max_body_lines, 80);
        assert!((ret.rerank_markdown_boost - 1.6).abs() < 1e-6);
    }
}
