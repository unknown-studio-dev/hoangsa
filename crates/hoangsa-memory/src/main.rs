//! The `hoangsa-memory` command-line interface.

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use hoangsa_memory_core::Synthesizer;
use hoangsa_memory_retrieve::ChromaConfig;
use hoangsa_memory_store::{ChromaStore, StoreRoot};

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
    /// Path to the `.hoangsa-memory/` data directory. Resolved via:
    /// `--root` > `$HOANGSA_MEMORY_ROOT` > `./.hoangsa-memory/` >
    /// `~/.hoangsa-memory/projects/{slug}/`.
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    /// Emit machine-readable JSON for subcommands that support it.
    #[arg(long, global = true)]
    json: bool,

    /// Mode::Full: LLM synthesizer. Requires the `anthropic` Cargo feature.
    /// The API key is read from `ANTHROPIC_API_KEY`.
    #[arg(long, global = true, value_enum)]
    synth: Option<SynthKind>,

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
    /// Initialize a bare `.thoth/` directory — seed MEMORY.md / LESSONS.md /
    /// config.toml. Idempotent. Hoangsa install handles higher-level setup.
    Init,

    /// Parse + index a source tree.
    Index {
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Query the memory.
    Query {
        #[arg(short = 'k', long, default_value_t = 8)]
        top_k: usize,
        #[arg(required = true)]
        text: Vec<String>,
    },

    /// Watch a source tree and re-index on change.
    Watch {
        #[arg(default_value = ".")]
        path: PathBuf,
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
        #[arg(required = true)]
        fqn: String,
        #[arg(long, value_enum, default_value_t = ImpactDir::Up)]
        direction: ImpactDir,
        #[arg(short = 'd', long, default_value_t = 3)]
        depth: usize,
    },

    /// 360-degree context for a single symbol.
    Context {
        #[arg(required = true)]
        fqn: String,
        #[arg(long, default_value_t = 32)]
        limit: usize,
    },

    /// Change-impact analysis over a unified diff.
    Changes {
        #[arg(long)]
        from: Option<String>,
        #[arg(short = 'd', long, default_value_t = 2)]
        depth: usize,
    },

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
        Cmd::Query { text, top_k } => {
            query_cmd::run_query(&root, text.join(" "), top_k, cli.json, cli.synth).await?
        }
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
/// is passed.
pub(crate) fn build_synth(kind: Option<SynthKind>) -> anyhow::Result<Option<Arc<dyn Synthesizer>>> {
    let Some(kind) = kind else {
        return Ok(None);
    };
    match kind {
        #[cfg(feature = "anthropic")]
        SynthKind::Anthropic => {
            let s = thoth_synth::anthropic::AnthropicSynthesizer::from_env()?;
            Ok(Some(Arc::new(s)))
        }
        #[cfg(not(feature = "anthropic"))]
        SynthKind::Anthropic => Err(anyhow::anyhow!(
            "--synth anthropic requires `--features anthropic` at build time"
        )),
    }
}

pub(crate) async fn open_chroma(store: &StoreRoot) -> Option<Arc<hoangsa_memory_store::ChromaCol>> {
    let cfg = ChromaConfig::load_or_default(&store.path).await;
    if !cfg.enabled {
        return None;
    }
    let path = cfg.data_path.unwrap_or_else(|| {
        StoreRoot::chroma_path(&store.path)
            .to_string_lossy()
            .to_string()
    });
    // enabled=true → user wants embeddings, so a failure here is *not* a
    // silent "feature off" — it's a missing dependency or misconfiguration
    // the operator needs to fix. Surface the underlying error on stderr
    // instead of dropping `Err` on the floor with `.ok()?`.
    let chroma = match ChromaStore::open(&path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "hoangsa-memory: chroma enabled in config but failed to start — embeddings disabled for this run.\n  cause: {e}\n  hint:  `pip install chromadb` into the python at $HOANGSA_MEMORY_PYTHON \
                 or ~/.hoangsa-memory/sidecar-venv/bin/python3, or set `[chroma] enabled = false` to silence this warning."
            );
            return None;
        }
    };
    match chroma.ensure_collection("thoth_code").await {
        Ok((col, _info)) => Some(Arc::new(col)),
        Err(e) => {
            eprintln!(
                "hoangsa-memory: chroma sidecar started but `ensure_collection(thoth_code)` failed: {e}"
            );
            None
        }
    }
}
