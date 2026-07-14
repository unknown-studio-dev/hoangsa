//! `hoangsa-memory graph` subcommands: query, paths, communities, processes.

use std::path::Path;

use anyhow::Result;
use clap::Subcommand;

use crate::daemon_cmd::{call_mcp_tool, emit_output};

#[derive(Subcommand, Debug)]
pub(crate) enum GraphCmd {
    /// BFS traversal from one or more FQNs.
    Query {
        /// Starting FQN(s).
        #[arg(long = "start", required = true)]
        start: Vec<String>,
        /// Edge direction: out | in | both.
        #[arg(long, default_value = "out")]
        direction: String,
        /// Edge kinds to follow (calls, imports, references, extends, declared_in, emits, subscribes).
        #[arg(long = "kinds")]
        kinds: Vec<String>,
        /// Maximum BFS depth.
        #[arg(long = "depth", default_value_t = 3)]
        depth: usize,
        /// Maximum nodes in output.
        #[arg(long, default_value_t = 500)]
        max_nodes: usize,
        /// Output format: json | dot.
        #[arg(long, default_value = "json")]
        format: String,
    },

    /// Find the shortest path between two FQNs.
    Paths {
        /// Source FQN.
        #[arg(long)]
        from: String,
        /// Destination FQN.
        #[arg(long)]
        to: String,
        /// Edge kinds to restrict the search to.
        #[arg(long = "kinds")]
        kinds: Vec<String>,
        /// Edge direction: out | in | both.
        #[arg(long, default_value = "out")]
        direction: String,
        /// Maximum hop count.
        #[arg(long = "depth", default_value_t = 10)]
        depth: usize,
    },

    /// Detect communities of closely-related symbols.
    Communities {
        /// Drop communities smaller than this size.
        #[arg(long, default_value_t = 3)]
        min_size: usize,
    },

    /// Trace process flows from entry-point symbols.
    Processes {
        /// Maximum DFS depth from each entry point.
        #[arg(long = "depth", default_value_t = 8)]
        depth: usize,
        /// Additional glob patterns for entry-point detection.
        #[arg(long = "entry-glob")]
        entry_globs: Vec<String>,
    },
}

pub(crate) async fn run_graph(root: &Path, cmd: GraphCmd, json: bool) -> Result<()> {
    match cmd {
        GraphCmd::Query {
            start,
            direction,
            kinds,
            depth,
            max_nodes,
            format,
        } => {
            let mut args = serde_json::json!({
                "start": start,
                "direction": direction,
                "max_depth": depth,
                "max_nodes": max_nodes,
                "format": format,
            });
            if !kinds.is_empty() {
                args["edge_kinds"] = serde_json::json!(kinds);
            }
            let (text, data, is_error) =
                call_mcp_tool(root, "memory_graph_query", args).await?;
            emit_output(text, data, is_error, json)
        }

        GraphCmd::Paths {
            from,
            to,
            kinds,
            direction,
            depth,
        } => {
            let mut args = serde_json::json!({
                "from": from,
                "to": to,
                "direction": direction,
                "max_depth": depth,
            });
            if !kinds.is_empty() {
                args["edge_kinds"] = serde_json::json!(kinds);
            }
            let (text, data, is_error) =
                call_mcp_tool(root, "memory_graph_paths", args).await?;
            emit_output(text, data, is_error, json)
        }

        GraphCmd::Communities { min_size } => {
            let args = serde_json::json!({ "min_size": min_size });
            let (text, data, is_error) =
                call_mcp_tool(root, "memory_graph_communities", args).await?;
            emit_output(text, data, is_error, json)
        }

        GraphCmd::Processes { depth, entry_globs } => {
            let args = serde_json::json!({
                "max_depth": depth,
                "entry_globs": entry_globs,
            });
            let (text, data, is_error) =
                call_mcp_tool(root, "memory_graph_processes", args).await?;
            emit_output(text, data, is_error, json)
        }
    }
}
