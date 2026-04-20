//! The `thoth` command-line interface.

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use thoth_core::Synthesizer;
use thoth_retrieve::ChromaConfig;
use thoth_store::{ChromaStore, StoreRoot};

mod archive_cmd;
mod compact;
mod daemon;
mod daemon_cmd;
mod hooks;
mod index_cmd;
mod memory_cmd;
mod migrate;
mod mine_cmd;
mod query_cmd;
mod resolve;
mod review;
mod rule_cmd;
mod setup;
mod watch_cmd;

// ------------------------------------------------------------------ CLI spec

#[derive(Parser, Debug)]
#[command(name = "thoth", version, about = "Long-term memory for coding agents.")]
struct Cli {
    /// Path to the `.thoth/` data directory. Resolved via:
    /// `--root` > `$THOTH_ROOT` > `./.thoth/` > `~/.thoth/projects/{slug}/`.
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

/// CLI-facing subset of [`thoth_graph::BlastDir`] so clap can derive
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
    /// One-shot bootstrap — run this first.
    Setup {
        #[arg(long)]
        status: bool,
        #[arg(long, alias = "accept-defaults")]
        yes: bool,
        #[arg(long, conflicts_with = "local")]
        global: bool,
        #[arg(long, conflicts_with = "global")]
        local: bool,
    },

    /// Initialize a bare `.thoth/` directory. Prefer `thoth setup`.
    #[command(hide = true)]
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

    /// Manage installed skills.
    Skills {
        #[command(subcommand)]
        cmd: hooks::SkillsCmd,
    },

    /// Install, remove, or dispatch Claude Code hooks.
    #[command(hide = true)]
    Hooks {
        #[command(subcommand)]
        cmd: hooks::HooksCmd,
    },

    /// Register the Thoth MCP server.
    #[command(hide = true)]
    Mcp {
        #[command(subcommand)]
        cmd: hooks::McpCmd,
    },

    /// Install skill + hooks + MCP server in one go.
    #[command(hide = true)]
    Install {
        #[arg(long, value_enum, default_value = "project")]
        scope: hooks::Scope,
    },

    /// Remove skill + hooks + MCP server from `settings.json`.
    Uninstall {
        #[arg(long, value_enum, default_value = "project")]
        scope: hooks::Scope,
    },

    /// Run a precision@k evaluation over a gold query set (TOML).
    Eval {
        #[arg(long)]
        gold: PathBuf,
        #[arg(short = 'k', long, default_value_t = 8)]
        top_k: usize,
        #[arg(long, value_enum, default_value_t = query_cmd::EvalMode::Zero)]
        mode: query_cmd::EvalMode,
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

    /// Maintenance pass over memory (forget pass + reflection debt report).
    Curate {
        #[arg(long)]
        quiet: bool,
    },

    /// Run a background memory review via LLM.
    Review {
        #[arg(long, default_value = "")]
        backend: String,
        #[arg(long, default_value = "")]
        model: String,
    },

    /// Compact MEMORY.md / LESSONS.md by merging near-duplicate entries.
    Compact {
        #[arg(long, default_value = "")]
        backend: String,
        #[arg(long, default_value = "")]
        model: String,
        #[arg(long)]
        dry_run: bool,
    },

    /// Inspect and edit the merged enforcement rule set.
    Rule {
        #[command(subcommand)]
        cmd: rule_cmd::RuleCmd,
    },

    /// Ingest Claude Code conversation JSONL into episodic memory.
    Mine {
        /// Path to a .jsonl file or directory containing them.
        #[arg(required = true)]
        source: PathBuf,
    },

    /// Manage the global project registry.
    Projects {
        #[command(subcommand)]
        cmd: resolve::ProjectsCmd,
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
        Cmd::Init => setup::cmd_init(&root).await?,
        Cmd::Setup {
            status,
            yes,
            global,
            local,
        } => {
            let setup_root = if local {
                PathBuf::from(".thoth")
            } else if global {
                resolve::global_root_for_cwd()?
            } else {
                root.clone()
            };
            setup::run(&setup_root, status, yes).await?;
        }
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
            memory_cmd::MemoryCmd::Forget => memory_cmd::run_forget(&root, cli.json).await?,
            memory_cmd::MemoryCmd::Nudge { window } => {
                memory_cmd::run_nudge(&root, window, cli.json, cli.synth).await?
            }
            memory_cmd::MemoryCmd::Pending => memory_cmd::run_pending(&root, cli.json).await?,
            memory_cmd::MemoryCmd::Promote { kind, index } => {
                memory_cmd::run_promote(&root, &kind, index, cli.json).await?
            }
            memory_cmd::MemoryCmd::Reject {
                kind,
                index,
                reason,
            } => memory_cmd::run_reject(&root, &kind, index, reason.as_deref(), cli.json).await?,
            memory_cmd::MemoryCmd::Log { limit } => {
                memory_cmd::run_log(&root, limit, cli.json).await?
            }
            memory_cmd::MemoryCmd::Migrate {
                yes,
                llm,
                llm_backend,
                llm_model,
            } => {
                migrate::run(
                    &root,
                    yes,
                    if llm {
                        Some(migrate::LlmOpts {
                            backend: llm_backend,
                            model: llm_model,
                        })
                    } else {
                        None
                    },
                )
                .await?;
            }
        },
        Cmd::Skills { cmd } => match cmd {
            hooks::SkillsCmd::List => hooks::cmd_skills_list(&root, cli.json).await?,
            hooks::SkillsCmd::Install { path, scope } => match path {
                Some(p) => hooks::promote_skill_draft(scope, &root, &p).await?,
                None => hooks::skills_install(scope, &root).await?,
            },
        },
        Cmd::Hooks { cmd } => match cmd {
            hooks::HooksCmd::Install { scope } => hooks::install(scope, &root).await?,
            hooks::HooksCmd::Uninstall { scope } => hooks::uninstall(scope).await?,
            hooks::HooksCmd::Exec { event } => hooks::exec(event, &root).await?,
        },
        Cmd::Mcp { cmd } => match cmd {
            hooks::McpCmd::Install { scope } => hooks::mcp_install(scope, &root).await?,
            hooks::McpCmd::Uninstall { scope } => hooks::mcp_uninstall(scope).await?,
        },
        Cmd::Install { scope } => hooks::install_all(scope, &root).await?,
        Cmd::Uninstall { scope } => hooks::uninstall_all(scope, &root).await?,
        Cmd::Eval { gold, top_k, mode } => {
            query_cmd::run_eval(&root, &gold, top_k, mode, cli.synth, cli.json).await?
        }
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
        Cmd::Curate { quiet } => memory_cmd::run_curate(&root, quiet).await?,
        Cmd::Review { backend, model } => review::cmd_review(&root, &backend, &model).await?,
        Cmd::Compact {
            backend,
            model,
            dry_run,
        } => compact::cmd_compact(&root, &backend, &model, dry_run).await?,
        Cmd::Rule { cmd } => match cmd {
            rule_cmd::RuleCmd::List { layer } => rule_cmd::cmd_list(&root, layer, cli.json).await?,
            rule_cmd::RuleCmd::Disable { id, project } => {
                rule_cmd::cmd_disable(&root, &id, project, cli.json).await?
            }
            rule_cmd::RuleCmd::Enable { id, project } => {
                rule_cmd::cmd_enable(&root, &id, project, cli.json).await?
            }
            rule_cmd::RuleCmd::Override { id, tier, project } => {
                rule_cmd::cmd_override(&root, &id, tier, project, cli.json).await?
            }
            rule_cmd::RuleCmd::Add {
                id,
                from_lesson,
                inline,
                tool,
                path_glob,
                cmd_regex,
                content_regex,
                natural,
                message,
                enforcement,
                project,
            } => {
                rule_cmd::cmd_add(
                    &root,
                    id,
                    from_lesson,
                    inline,
                    tool,
                    path_glob,
                    cmd_regex,
                    content_regex,
                    natural,
                    message,
                    enforcement,
                    project,
                    cli.json,
                )
                .await?
            }
            rule_cmd::RuleCmd::Diff => rule_cmd::cmd_diff(&root, cli.json).await?,
            rule_cmd::RuleCmd::Check {
                tool,
                path,
                cmd,
                content,
            } => rule_cmd::cmd_check(&root, tool, path, cmd, content, cli.json).await?,
            rule_cmd::RuleCmd::Compile => rule_cmd::cmd_compile(&root, cli.json).await?,
        },
        Cmd::Archive { cmd } => match cmd {
            archive_cmd::ArchiveCmd::Ingest { project, topic } => {
                archive_cmd::cmd_archive_ingest(&root, project.as_deref(), topic.as_deref()).await?
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
            archive_cmd::ArchiveCmd::Curate {
                backend,
                model,
                max_sessions,
            } => archive_cmd::cmd_archive_curate(&root, &backend, &model, max_sessions).await?,
        },
        Cmd::Mine { source } => mine_cmd::run_mine(&root, &source, cli.json).await?,
        Cmd::Projects { cmd } => match cmd {
            resolve::ProjectsCmd::List => resolve::cmd_projects_list()?,
            resolve::ProjectsCmd::Which => resolve::cmd_projects_which(&root)?,
            resolve::ProjectsCmd::Migrate { dry_run, rm_local } => {
                resolve::cmd_projects_migrate(dry_run, rm_local).await?;
            }
            resolve::ProjectsCmd::MigrateSlugs { dry_run } => {
                resolve::cmd_projects_migrate_slugs(dry_run).await?;
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

pub(crate) async fn open_chroma(store: &StoreRoot) -> Option<Arc<thoth_store::ChromaCol>> {
    let cfg = ChromaConfig::load_or_default(&store.path).await;
    if !cfg.enabled {
        return None;
    }
    let path = cfg.data_path.unwrap_or_else(|| {
        StoreRoot::chroma_path(&store.path)
            .to_string_lossy()
            .to_string()
    });
    let chroma = ChromaStore::open(&path).await.ok()?;
    let (col, _info) = chroma.ensure_collection("thoth_code").await.ok()?;
    Some(Arc::new(col))
}
