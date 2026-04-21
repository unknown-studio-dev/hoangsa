//! Built-in filters for `npm`, `pnpm`, `yarn`, `pip`, `pip3`.
//!
//! Package managers love to print huge blocks of deprecation warnings,
//! funding notices, audit summaries, and progress bars. We strip all of
//! those and keep errors.

use crate::filters::{grep_out, join, lines};
use crate::registry::{BuiltinHandler, FilterResult, ProxyContext};
use crate::scope;

pub fn register(v: &mut Vec<BuiltinHandler>) {
    for cmd in ["npm", "pnpm", "yarn"] {
        v.push(BuiltinHandler {
            cmd: match cmd {
                "npm" => "npm",
                "pnpm" => "pnpm",
                "yarn" => "yarn",
                _ => unreachable!(),
            },
            subcmd: None,
            priority: 50,
            filter: node_filter,
        });
    }
    for cmd in ["pip", "pip3"] {
        v.push(BuiltinHandler {
            cmd: match cmd {
                "pip" => "pip",
                "pip3" => "pip3",
                _ => unreachable!(),
            },
            subcmd: None,
            priority: 50,
            filter: pip_filter,
        });
    }
}

fn node_filter(ctx: &ProxyContext) -> FilterResult {
    if scope::has_any_flag(&ctx.args, scope::NODE_SCOPE) || ctx.strict {
        return FilterResult::default();
    }
    let drop_patterns = &[
        r"^npm (notice|WARN deprecated|fund|audit)",
        r"^(⠋|⠙|⠹|⠸|⠼|⠴|⠦|⠧|⠇|⠏)",
        r"^\s*$",
        // Progress bar line like "[####    ] 45%"
        r"^\[#+\s*\]",
        // pnpm resolution "progress: resolved X, reused Y"
        r"^Progress: resolved",
    ];
    let out = apply_drops(&ctx.stdout, drop_patterns);
    let err = apply_drops(&ctx.stderr, drop_patterns);
    FilterResult {
        stdout: Some(out),
        stderr: Some(err),
        ..Default::default()
    }
}

fn pip_filter(ctx: &ProxyContext) -> FilterResult {
    if scope::has_any_flag(&ctx.args, scope::PIP_SCOPE) || ctx.strict {
        return FilterResult::default();
    }
    let drop_patterns = &[
        r"^\s*Requirement already satisfied:",
        r"^Collecting ",
        r"^Downloading ",
        r"^  Downloading ",
        r"^  Using cached ",
        r"^\s*$",
    ];
    let out = apply_drops(&ctx.stdout, drop_patterns);
    let err = apply_drops(&ctx.stderr, drop_patterns);
    FilterResult {
        stdout: Some(out),
        stderr: Some(err),
        ..Default::default()
    }
}

fn apply_drops(raw: &str, patterns: &[&str]) -> String {
    let mut ls = lines(raw);
    for p in patterns {
        ls = grep_out(&ls, p);
    }
    join(&ls)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(cmd: &str, stdout: &str) -> ProxyContext {
        ProxyContext {
            cmd: cmd.into(),
            subcmd: None,
            args: vec![],
            stdout: stdout.into(),
            stderr: String::new(),
            exit: 0,
            cwd: "/".into(),
            strict: false,
        }
    }

    #[test]
    fn npm_drops_notices_and_funding() {
        let input = "npm notice New version available\nnpm WARN deprecated foo@1.2\nnpm fund 17 packages\nreal error here\n";
        let out = node_filter(&ctx("npm", input)).stdout.unwrap();
        assert!(!out.contains("npm notice"));
        assert!(!out.contains("npm WARN"));
        assert!(!out.contains("npm fund"));
        assert!(out.contains("real error"));
    }

    #[test]
    fn pip_drops_requirement_satisfied() {
        let input = "Requirement already satisfied: foo\nCollecting bar\nSuccessfully installed bar-1.0\n";
        let out = pip_filter(&ctx("pip", input)).stdout.unwrap();
        assert!(!out.contains("Requirement already"));
        assert!(!out.contains("Collecting"));
        assert!(out.contains("Successfully installed"));
    }
}
