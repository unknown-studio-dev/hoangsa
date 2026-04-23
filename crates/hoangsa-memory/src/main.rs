//! The `hoangsa-memory` command-line interface.

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use hoangsa_memory_core::Synthesizer;
use hoangsa_memory_retrieve::VectorStoreConfig;
use hoangsa_memory_store::{
    fastembed_cache_dir, prefetch_model, EmbeddedVectorStore, StoreRoot, VectorCol, VectorStore,
};

mod archive_cmd;
mod daemon;
mod daemon_cmd;
mod index_cmd;
mod init_cmd;
mod memory_cmd;
mod query_cmd;
mod resolve;
mod watch_cmd;

// ------------------------------------------------------------------ CLI spec

#[derive(Parser, Debug)]
#[command(name = "hoangsa-memory", version, about = "Long-term memory for coding agents.")]
struct Cli {
    /// Path to the `.hoangsa/memory/` data directory. Resolved via:
    /// `--root` > `$HOANGSA_MEMORY_ROOT` > `./.hoangsa/memory/` >
    /// `~/.hoangsa/memory/projects/{slug}/`.
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    /// Emit machine-readable JSON for subcommands that support it.
    #[arg(long, global = true)]
    json: bool,

    /// Show internal debug logs. Without this the CLI only prints
    /// user-facing output; `tracing` events are hidden. Overrides `RUST_LOG`
    /// when passed. Repeat for more detail (`-v` = debug, `-vv` = trace).
    #[arg(short = 'v', long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SynthKind {
    Anthropic,
}

/// CLI-facing subset of [`hoangsa_memory_graph::BlastDir`] so clap can derive
/// ValueEnum without leaking the dependency across crate boundaries.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum ImpactDir {
    Up,
    Down,
    Both,
}

impl ImpactDir {
    fn as_str(self) -> &'static str {
        match self {
            ImpactDir::Up => "up",
            ImpactDir::Down => "down",
            ImpactDir::Both => "both",
        }
    }
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Initialize a bare `.hoangsa/memory/` directory — seed MEMORY.md / LESSONS.md /
    /// config.toml. Idempotent. Hoangsa install handles higher-level setup.
    Init,

    /// Parse + index a source tree.
    Index {
        /// Source tree to index. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Query the memory.
    Query {
        /// Maximum number of chunks to return.
        #[arg(short = 'k', long, default_value_t = 8)]
        top_k: usize,
        /// Query text (joined with spaces if multiple words).
        #[arg(required = true)]
        text: Vec<String>,
        /// Optional LLM synthesizer to summarise the retrieved chunks.
        /// Requires the `anthropic` Cargo feature and `ANTHROPIC_API_KEY`.
        #[arg(long, value_enum)]
        synth: Option<SynthKind>,
    },

    /// Watch a source tree and re-index on change.
    Watch {
        /// Source tree to watch. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Debounce window in milliseconds between re-index passes.
        #[arg(long, default_value_t = 300)]
        debounce_ms: u64,
    },

    /// Inspect or edit memory files.
    Memory {
        #[command(subcommand)]
        cmd: memory_cmd::MemoryCmd,
    },

    /// Verbatim conversation archive — ingest, search, manage sessions.
    Archive {
        #[command(subcommand)]
        cmd: archive_cmd::ArchiveCmd,
    },

    /// Blast-radius analysis for a symbol FQN.
    Impact {
        /// Fully-qualified symbol name (e.g. `crate::module::Type::method`).
        #[arg(required = true)]
        fqn: String,
        /// Direction to walk the graph: `up` = callers, `down` = callees.
        #[arg(long, value_enum, default_value_t = ImpactDir::Up)]
        direction: ImpactDir,
        /// Maximum traversal depth from the starting symbol.
        #[arg(short = 'd', long, default_value_t = 3)]
        depth: usize,
    },

    /// 360-degree context for a single symbol.
    Context {
        /// Fully-qualified symbol name to inspect.
        #[arg(required = true)]
        fqn: String,
        /// Maximum number of related edges to include per direction.
        #[arg(long, default_value_t = 32)]
        limit: usize,
    },

    /// Change-impact analysis over a unified diff.
    Changes {
        /// Path to a unified-diff file. Reads stdin when omitted.
        #[arg(long)]
        from: Option<String>,
        /// Maximum caller-graph depth walked from each changed symbol.
        #[arg(short = 'd', long, default_value_t = 2)]
        depth: usize,
    },

    /// Download the default embedding model into the shared fastembed
    /// cache dir. Used by the installer so the first `index` / `query` /
    /// `archive ingest` call doesn't stall on a 118 MB HuggingFace fetch.
    /// No-op on subsequent runs (fastembed short-circuits when the
    /// weights are already present).
    PrefetchEmbed,
}

// --------------------------------------------------------------------- entry

fn init_tracing(verbose: u8) {
    let filter = match verbose {
        0 => tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error")),
        1 => tracing_subscriber::EnvFilter::new("info"),
        2 => tracing_subscriber::EnvFilter::new("debug"),
        _ => tracing_subscriber::EnvFilter::new("trace"),
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .without_time()
        .with_target(false)
        .compact()
        .init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let root = resolve::resolve_root(cli.root.as_deref());

    match cli.cmd {
        Cmd::Init => init_cmd::cmd_init(&root).await?,
        Cmd::Index { path } => index_cmd::run_index(&root, &path, cli.json).await?,
        Cmd::Query {
            text,
            top_k,
            synth,
        } => query_cmd::run_query(&root, text.join(" "), top_k, cli.json, synth).await?,
        Cmd::Watch { path, debounce_ms } => {
            watch_cmd::run_watch(&root, &path, std::time::Duration::from_millis(debounce_ms))
                .await?
        }
        Cmd::Memory { cmd } => match cmd {
            memory_cmd::MemoryCmd::Show => memory_cmd::run_show(&root).await?,
            memory_cmd::MemoryCmd::Edit => memory_cmd::run_edit(&root).await?,
            memory_cmd::MemoryCmd::Fact { tags, text } => {
                memory_cmd::run_fact(&root, text.join(" "), tags).await?
            }
            memory_cmd::MemoryCmd::Lesson { when, advice } => {
                memory_cmd::run_lesson(&root, when, advice.join(" ")).await?
            }
        },
        Cmd::Impact {
            fqn,
            direction,
            depth,
        } => daemon_cmd::cmd_impact(&root, &fqn, direction.as_str(), depth, cli.json).await?,
        Cmd::Context { fqn, limit } => {
            daemon_cmd::cmd_context(&root, &fqn, limit, cli.json).await?
        }
        Cmd::Changes { from, depth } => {
            daemon_cmd::cmd_changes(&root, from.as_deref(), depth, cli.json).await?
        }
        Cmd::PrefetchEmbed => {
            let cache = fastembed_cache_dir();
            eprintln!(
                "hoangsa-memory: prefetching `multilingual-e5-small` into {}",
                cache.display()
            );
            prefetch_model().await?;
            eprintln!("hoangsa-memory: prefetch complete");
        }
        Cmd::Archive { cmd } => match cmd {
            archive_cmd::ArchiveCmd::Ingest {
                project,
                topic,
                refresh,
                limit,
            } => {
                archive_cmd::cmd_archive_ingest(
                    &root,
                    project.as_deref(),
                    topic.as_deref(),
                    refresh,
                    limit,
                )
                .await?
            }
            archive_cmd::ArchiveCmd::Status => {
                archive_cmd::cmd_archive_status(&root, cli.json).await?
            }
            archive_cmd::ArchiveCmd::Topics { project } => {
                archive_cmd::cmd_archive_topics(&root, project.as_deref(), cli.json).await?
            }
            archive_cmd::ArchiveCmd::Search {
                top_k,
                project,
                topic,
                text,
            } => {
                archive_cmd::cmd_archive_search(
                    &root,
                    &text.join(" "),
                    top_k,
                    project.as_deref(),
                    topic.as_deref(),
                    cli.json,
                )
                .await?
            }
            archive_cmd::ArchiveCmd::Purge {
                older_than,
                all,
                dry_run,
            } => {
                archive_cmd::cmd_archive_purge(
                    &root,
                    older_than.as_deref(),
                    all,
                    dry_run,
                    cli.json,
                )
                .await?
            }
        },
    }

    Ok(())
}

// ------------------------------------------------------- provider constructors

/// Build a synthesizer from the CLI flag. Returns `Ok(None)` when no flag
/// is passed. No synth providers are currently bundled, so passing `--synth`
/// always errors.
pub(crate) fn build_synth(kind: Option<SynthKind>) -> anyhow::Result<Option<Arc<dyn Synthesizer>>> {
    match kind {
        None => Ok(None),
        Some(SynthKind::Anthropic) => Err(anyhow::anyhow!(
            "no synthesizer provider is bundled in this build"
        )),
    }
}

/// Process-wide advisory flock serialising any CLI subcommand that
/// loads the embedder (`archive ingest`, `index`, `watch`). The ONNX
/// model fastembed pulls in is ~130 MB RSS when hot, and the embedder
/// itself holds a `&mut self` lock — running two concurrently would
/// either deadlock or double the footprint. Before Phase 2 this same
/// lock fenced off the Python ChromaDB sidecar; see
/// `.hoangsa/sessions/fix/memory-4bugs/RESEARCH.md` for the incident
/// that motivated it.
///
/// Returns `Ok(Some(file))` when this process owns the lock (caller
/// keeps the handle alive until vector work finishes), `Ok(None)` when
/// another process already holds it, and `Err` when the lockfile
/// itself can't be opened.
///
/// Read paths (`query`) intentionally do *not* acquire this lock: the
/// point is preventing embedder pile-up on write-heavy commands, and
/// queries already serialise through the in-process embedder mutex.
pub(crate) fn acquire_vector_lock() -> anyhow::Result<Option<std::fs::File>> {
    use anyhow::Context;
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("cannot determine home directory for vector lock")?;
    let dir = PathBuf::from(home).join(".hoangsa").join("memory");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create {} for vector lock", dir.display()))?;
    // Keep the old filename so an in-flight hook from before the upgrade
    // still serialises against us.
    let path = dir.join("archive-ingest.lock");

    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open vector lock {}", path.display()))?;

    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(_) => Ok(None),
    }
}

/// Advisory flock serialising direct-mode CLIs that would otherwise
/// race on the per-project store backends (`graph.redb`, `kv.redb`,
/// `fts.tantivy/`, …). Scoped per-root: two different projects don't
/// block each other.
///
/// Daemon does NOT acquire this — it opens the store for its lifetime
/// via the backends' own locks. CLIs should prefer the daemon via
/// [`daemon::DaemonClient::connect_or_wait`] and only fall through to
/// direct mode + this lock when no daemon is running. Without it, two
/// direct-mode CLIs (e.g. the session-start bootstrap worker plus a
/// user-typed `hoangsa-memory index .`) would race on redb's exclusive
/// lock and one would bail with the cryptic "Database already open".
///
/// Blocks up to `timeout`. `Ok(None)` means the timeout elapsed while
/// another holder was still active — callers should emit a clear
/// message. `Err` is reserved for filesystem failures.
pub(crate) async fn acquire_store_lock(
    root: &std::path::Path,
    timeout: std::time::Duration,
) -> anyhow::Result<Option<std::fs::File>> {
    use anyhow::Context;
    std::fs::create_dir_all(root)
        .with_context(|| format!("create {} for store lock", root.display()))?;
    let path = root.join("store.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open store lock {}", path.display()))?;

    let deadline = tokio::time::Instant::now() + timeout;
    let mut informed = false;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(_) => {
                if !informed {
                    eprintln!(
                        "hoangsa-memory: another process is indexing this project; waiting…"
                    );
                    informed = true;
                }
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    return Ok(None);
                }
                let step = std::time::Duration::from_millis(250);
                tokio::time::sleep(step.min(deadline - now)).await;
            }
        }
    }
}

pub(crate) async fn open_vector_store(
    store: &StoreRoot,
) -> Option<Arc<dyn VectorCol>> {
    let cfg = VectorStoreConfig::load_or_default(&store.path).await;
    if !cfg.enabled {
        return None;
    }
    let path = cfg
        .data_path
        .map(PathBuf::from)
        .unwrap_or_else(|| StoreRoot::vectors_path(&store.path));
    // enabled=true → user wants embeddings, so a failure here is *not* a
    // silent "feature off" — it's a missing model download or a broken
    // ONNX runtime the operator needs to see. Surface the underlying
    // error on stderr instead of dropping `Err` on the floor.
    let vectors = match EmbeddedVectorStore::open(&path).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "hoangsa-memory: vector_store enabled in config but failed to start — embeddings disabled for this run.\n  cause: {e}\n  hint:  first run downloads the `multilingual-e5-small` ONNX weights (~118MB). \
                 Check network + disk, or set `[vector_store] enabled = false` to silence this warning."
            );
            return None;
        }
    };
    match vectors.ensure_collection("hoangsa_memory_code").await {
        Ok((col, _info)) => Some(col),
        Err(e) => {
            eprintln!(
                "hoangsa-memory: vector store opened but `ensure_collection(hoangsa_memory_code)` failed: {e}"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    #[tokio::test]
    async fn store_lock_serialises_contenders() {
        // Two concurrent acquirers on the same root: the second must
        // wait until the first drops its lock, then acquire
        // successfully.
        let dir = tempdir().unwrap();
        let first = acquire_store_lock(dir.path(), Duration::from_secs(2))
            .await
            .expect("first acquire shouldn't error")
            .expect("first acquire should get the lock");

        let root = dir.path().to_path_buf();
        let started = Instant::now();
        let waiter = tokio::spawn(async move {
            acquire_store_lock(&root, Duration::from_secs(5)).await
        });

        // Give the waiter a moment to enter the try_lock loop.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!waiter.is_finished(), "waiter should still be blocked");

        // Drop the first lock — waiter should then succeed.
        drop(first);
        let result = waiter.await.unwrap().unwrap();
        assert!(result.is_some(), "waiter should get the lock after drop");
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(300),
            "waiter should have blocked; elapsed {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "waiter shouldn't have hit its own timeout; elapsed {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn store_lock_times_out_when_holder_never_drops() {
        let dir = tempdir().unwrap();
        let _held = acquire_store_lock(dir.path(), Duration::from_secs(2))
            .await
            .unwrap()
            .expect("first holder");

        let started = Instant::now();
        let result = acquire_store_lock(dir.path(), Duration::from_millis(600))
            .await
            .unwrap();
        let elapsed = started.elapsed();
        assert!(result.is_none(), "second call should time out to None");
        assert!(
            elapsed >= Duration::from_millis(500),
            "should have waited roughly the full timeout; elapsed {elapsed:?}"
        );
    }
}
