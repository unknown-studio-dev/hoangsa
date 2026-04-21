//! Handler registry. Resolves `(cmd, subcmd) → handler` with order:
//!   1. Rhai script from project dir
//!   2. Rhai script from global dir
//!   3. Built-in Rust handler
//!
//! Within the same tier, higher `priority` wins. Ties resolve to the
//! first-registered handler (project dirs are loaded before global).

use crate::exec::Captured;

/// Context a handler receives. Mirrors the Rhai `ctx.*` surface.
#[derive(Debug, Clone)]
pub struct ProxyContext {
    pub cmd: String,
    pub subcmd: Option<String>,
    pub args: Vec<String>,
    pub stdout: String,
    pub stderr: String,
    pub exit: i32,
    pub cwd: String,
    /// Lossless-only mode. Handlers MUST skip any operation that drops
    /// data (head/tail/sandwich/line-count caps) when this is true.
    /// Lossless-safe primitives remain: ANSI strip, blank collapse, exact
    /// dedupe, drop of known-noise progress lines.
    pub strict: bool,
}

impl ProxyContext {
    pub fn from_captured(
        cmd: &str,
        args: &[String],
        captured: Captured,
        cwd: &std::path::Path,
        strict: bool,
    ) -> Self {
        let subcmd = args.iter().find(|a| !a.starts_with('-')).cloned();
        Self {
            cmd: cmd.to_string(),
            subcmd,
            args: args.to_vec(),
            stdout: captured.stdout,
            stderr: captured.stderr,
            exit: captured.exit,
            cwd: cwd.to_string_lossy().to_string(),
            strict,
        }
    }
}

/// What a handler returns. Missing fields fall back to passthrough.
#[derive(Debug, Clone, Default)]
pub struct FilterResult {
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub exit: Option<i32>,
}

pub type BuiltinFilter = fn(&ProxyContext) -> FilterResult;

pub struct BuiltinHandler {
    pub cmd: &'static str,
    /// If `None`, matches every subcommand of `cmd`.
    pub subcmd: Option<&'static str>,
    pub priority: i32,
    pub filter: BuiltinFilter,
}

/// Catalog of built-in handlers. Listed in rough specificity order —
/// resolution is by exact (cmd, subcmd) first, then (cmd, None) wildcard.
pub fn builtins() -> Vec<BuiltinHandler> {
    let mut v = Vec::new();
    crate::handlers::git::register(&mut v);
    crate::handlers::cargo::register(&mut v);
    crate::handlers::fs::register(&mut v);
    crate::handlers::pkg::register(&mut v);
    crate::handlers::curl::register(&mut v);
    v
}

/// The set of `cmd` values we know how to proxy. Used by `hsp hook rewrite`
/// without loading Rhai (O(1) hash match).
pub fn known_commands() -> &'static [&'static str] {
    &[
        "git", "cargo", "ls", "cat", "grep", "find", "rg", "npm", "pnpm", "yarn", "pip", "pip3",
        "curl",
    ]
}

pub fn is_known(cmd: &str) -> bool {
    known_commands().iter().any(|k| *k == cmd)
}

/// Pick the best built-in for `(cmd, subcmd)`. Exact subcmd match beats
/// wildcard; higher priority beats lower.
pub fn pick_builtin<'a>(
    builtins: &'a [BuiltinHandler],
    cmd: &str,
    subcmd: Option<&str>,
) -> Option<&'a BuiltinHandler> {
    let mut best: Option<&BuiltinHandler> = None;
    for h in builtins {
        if h.cmd != cmd {
            continue;
        }
        let matches = match (h.subcmd, subcmd) {
            (Some(a), Some(b)) => a == b,
            (None, _) => true,
            (Some(_), None) => false,
        };
        if !matches {
            continue;
        }
        let specificity = h.subcmd.map(|_| 1).unwrap_or(0);
        let best_specificity = best.and_then(|b| b.subcmd).map(|_| 1).unwrap_or(0);
        let better = match best {
            None => true,
            Some(cur) => {
                specificity > best_specificity
                    || (specificity == best_specificity && h.priority > cur.priority)
            }
        };
        if better {
            best = Some(h);
        }
    }
    best
}
