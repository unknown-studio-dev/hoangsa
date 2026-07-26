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
//! walker's [`WalkOptions`](hoangsa_memory_parse::walk::WalkOptions).

use std::path::Path;

use serde::Deserialize;

/// Indexer-side config. Mirrors the `[index]` table in `<root>/config.toml`.
///
/// See the module docs for an annotated example.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IndexConfig {
    /// Gitignore-syntax patterns to exclude from indexing. Applied on top
    /// of `.gitignore`, `.ignore`, and `.memoryignore`. Supports negation
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
/// `memory_impact` switches from a per-node listing to a file-grouped
/// summary.
///
/// The values here feed [`hoangsa_memory_core::RenderOptions`] via
/// [`Self::render_options`]. The underlying [`hoangsa_memory_core::Retrieval`] is
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
    /// Node count above which `memory_impact` groups results by file
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
    /// [`hoangsa_memory_core::Retrieval::render_with`]. The `impact_group_threshold`
    /// is used directly by `memory_impact`, not via `RenderOptions`.
    pub fn render_options(&self) -> hoangsa_memory_core::RenderOptions {
        hoangsa_memory_core::RenderOptions {
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
/// `hoangsa-memory watch` process required. Since the MCP daemon already holds the
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

/// Vector store config (`[vector_store]` in `config.toml`; legacy key
/// `[chroma]` still accepted for existing user configs).
///
/// Replaces the old `ChromaConfig`. The `enabled` flag gates opening
/// the `vectors.sqlite` file and loading the fastembed ONNX model —
/// when false, retrieval falls back to BM25 + graph only.
#[derive(Default, Debug, Clone, Deserialize)]
#[serde(default)]
pub struct VectorStoreConfig {
    /// Enable the in-process vector store. **Default `false`.**
    ///
    /// Semantic retrieval costs a ~465 MB model cache and a resident
    /// ONNX session whose CPU arena ratchets to ~150-300 MB, and it is the
    /// single heaviest thing this tool does. BM25 + symbol + graph retrieval
    /// work without it and cover most recalls, so it is opt-in rather than
    /// something a new user pays for before deciding they want it.
    ///
    /// Turn it on per project in `<root>/config.toml`:
    ///
    /// ```toml
    /// [vector_store]
    /// enabled = true
    /// ```
    ///
    /// then run `hoangsa-memory prefetch-embed` once to warm the shared
    /// cache. A global `<install_root>/no-embed` marker still overrides this
    /// to `false` — see [`Self::is_effectively_enabled`].
    pub enabled: bool,
    /// Override the on-disk location of `vectors.sqlite`. When `None`,
    /// falls back to `StoreRoot::vectors_path()`.
    pub data_path: Option<String>,
}



impl VectorStoreConfig {
    /// Load `<root>/config.toml` if it exists, returning the
    /// `[vector_store]` table — or, for backward compatibility, the
    /// legacy `[chroma]` table when `[vector_store]` is absent. Returns
    /// [`Self::default`] if both the file and the relevant tables are
    /// missing or malformed.
    pub async fn load_or_default(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "vector_store: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        Self::from_toml_text(&text, &path)
    }

    /// Sync twin of [`Self::load_or_default`].
    pub fn load_or_default_sync(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "vector_store: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        Self::from_toml_text(&text, &path)
    }

    fn from_toml_text(text: &str, path: &Path) -> Self {
        match toml::from_str::<ConfigFile>(text) {
            Ok(cf) => cf.vector_store.or(cf.chroma).unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "vector_store: config.toml parse error, using defaults");
                Self::default()
            }
        }
    }

    /// Whether the embedder should actually run. This is `enabled` from
    /// `config.toml` AND-ed with the absence of the installer's global
    /// `--no-embed` opt-out (a `no-embed` marker under the install dir).
    ///
    /// Callers gate the vector store on this rather than the raw `enabled`
    /// field so that `--no-embed` at install time durably prevents the
    /// ~465 MB model cache — otherwise a per-project default of
    /// `enabled = true` would pull the weights lazily on first use.
    pub fn is_effectively_enabled(&self) -> bool {
        self.enabled && !hoangsa_memory_store::embeddings_disabled_globally()
    }
}

/// LLM reranking of recall results. Mirrors the `[rerank]` table in
/// `<root>/config.toml`.
///
/// Every ranking stage before this one scores on form — term overlap,
/// identifier equality, graph edges, rank position. None reads a chunk and
/// asks whether it answers the question. This pass does, at the cost of one
/// model round-trip per recall, which is why it is off by default.
///
/// ```toml
/// [rerank]
/// enabled = true
/// # Optional: how many fused results to show the model. Default 24.
/// candidates = 24
/// # Optional: seconds before giving up and keeping the fused order.
/// timeout_secs = 20
/// # Optional: override the model invocation. The prompt is appended as the
/// # final argument. Defaults to `claude -p`, then `codex exec`.
/// command = ["claude", "-p"]
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RerankConfig {
    /// Enable the pass. Default `false` — recall is on the agent's hot path
    /// and must not grow a model call because someone upgraded.
    pub enabled: bool,
    /// How many post-fusion results to hand the model. Clamped to `[2, 100]`
    /// at load: too few and the pass cannot change anything, too many and the
    /// prompt costs more than the ranking is worth.
    pub candidates: usize,
    /// Seconds to wait for the model before falling back to the fused order.
    pub timeout_secs: u64,
    /// Explicit model argv. Empty means "probe PATH for a harness CLI".
    pub command: Vec<String>,
}

impl Default for RerankConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            candidates: 24,
            timeout_secs: 20,
            command: Vec::new(),
        }
    }
}

impl RerankConfig {
    /// Load `<root>/config.toml`, returning the `[rerank]` table or
    /// [`Self::default`]. Tolerant like the other loaders: a malformed file
    /// warns and falls back rather than breaking recall.
    pub async fn load_or_default(root: &Path) -> Self {
        let path = root.join("config.toml");
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "rerank: could not read config.toml, using defaults");
                return Self::default();
            }
        };
        Self::from_toml_text(&text, &path)
    }

    /// Parse from raw TOML text. Split out so tests don't need a file.
    pub fn from_toml_text(text: &str, path: &Path) -> Self {
        let mut cfg = match toml::from_str::<ConfigFile>(text) {
            Ok(cf) => cf.rerank.unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "rerank: config.toml parse error, using defaults");
                Self::default()
            }
        };
        cfg.candidates = cfg.candidates.clamp(2, 100);
        cfg
    }
}

/// TOML file schema — the outer document. We only care about `[index]`,
/// `[output]`, `[retrieve]`, `[watch]`, `[vector_store]`, and the legacy
/// `[chroma]`; other tables (e.g. `[memory]`, read by
/// `hoangsa-memory-policy`) are left to their owners, so we tolerate
/// them instead of `deny_unknown_fields` here.
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
    vector_store: Option<VectorStoreConfig>,
    #[serde(default)]
    rerank: Option<RerankConfig>,
    /// Legacy key — read when `[vector_store]` is absent so existing
    /// `config.toml` files keep working during the Chroma → fastembed
    /// migration.
    #[serde(default)]
    chroma: Option<VectorStoreConfig>,
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
        // `[memory]` belongs to hoangsa-memory-policy; presence here must not break
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

    /// The vector store is opt-in. It costs a ~465 MB download and a resident
    /// ONNX session, so a fresh install must not pay for it before the user
    /// asks — and an explicit `enabled = true` must still turn it on.
    #[test]
    fn vector_store_is_off_until_explicitly_enabled() {
        assert!(
            !VectorStoreConfig::default().enabled,
            "semantic retrieval must be opt-in"
        );
        let path = std::path::Path::new("config.toml");
        assert!(
            !VectorStoreConfig::from_toml_text("", path).enabled,
            "an empty config must not enable it"
        );
        assert!(
            !VectorStoreConfig::from_toml_text("[retrieve]\n", path).enabled,
            "an unrelated table must not enable it"
        );
        assert!(
            VectorStoreConfig::from_toml_text("[vector_store]\nenabled = true\n", path).enabled,
            "explicit opt-in must be honoured"
        );
    }
}
