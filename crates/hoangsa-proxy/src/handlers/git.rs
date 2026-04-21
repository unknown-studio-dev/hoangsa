//! Built-in filters for `git`.
//!
//! Focus: git log/diff/status/blame/show — all produce verbose output
//! dominated by padding the model does not need.

use crate::filters::{grep_out, head, join, lines, sandwich};
use crate::registry::{BuiltinHandler, FilterResult, ProxyContext};
use crate::scope;

pub fn register(v: &mut Vec<BuiltinHandler>) {
    v.push(BuiltinHandler {
        cmd: "git",
        subcmd: Some("log"),
        priority: 50,
        filter: log_filter,
    });
    v.push(BuiltinHandler {
        cmd: "git",
        subcmd: Some("diff"),
        priority: 50,
        filter: diff_filter,
    });
    v.push(BuiltinHandler {
        cmd: "git",
        subcmd: Some("status"),
        priority: 50,
        filter: status_filter,
    });
    v.push(BuiltinHandler {
        cmd: "git",
        subcmd: Some("blame"),
        priority: 50,
        filter: blame_filter,
    });
    v.push(BuiltinHandler {
        cmd: "git",
        subcmd: Some("show"),
        priority: 50,
        filter: show_filter,
    });
    // Catch-all wildcard for other git subcommands — passthrough.
    v.push(BuiltinHandler {
        cmd: "git",
        subcmd: None,
        priority: 0,
        filter: passthrough,
    });
}

fn passthrough(_ctx: &ProxyContext) -> FilterResult {
    FilterResult::default()
}

/// git log: keep first 40 lines; drop blank padding lines between commits.
/// Passthrough when user explicitly scoped output (--reverse, -n, --all, …)
/// or strict mode (blank collapse is lossless but `head(40)` isn't).
fn log_filter(ctx: &ProxyContext) -> FilterResult {
    if scope::has_any_flag(&ctx.args, scope::GIT_LOG_SCOPE) {
        return FilterResult::default();
    }
    let ls = lines(&ctx.stdout);
    if ctx.strict {
        let clean = grep_out(&ls, r"^\s*$");
        return FilterResult {
            stdout: Some(join(&clean)),
            ..Default::default()
        };
    }
    let capped = head(&ls, 40);
    let clean = grep_out(&capped, r"^\s*$");
    FilterResult {
        stdout: Some(join(&clean)),
        ..Default::default()
    }
}

/// git diff: sandwich (first 120, last 40) to preserve summary + tail.
/// Strict mode: no sandwich (lossy) — just blank collapse.
fn diff_filter(ctx: &ProxyContext) -> FilterResult {
    if scope::has_any_flag(&ctx.args, scope::GIT_DIFF_SCOPE) {
        return FilterResult::default();
    }
    let ls = lines(&ctx.stdout);
    if ctx.strict {
        // Diff output has no safe lossless trim — blanks are rare and
        // semantically meaningful inside hunks. Passthrough.
        return FilterResult::default();
    }
    let trimmed = sandwich(&ls, 120, 40);
    FilterResult {
        stdout: Some(join(&trimmed)),
        ..Default::default()
    }
}

/// git status: drop "no changes added" boilerplate, keep the file list.
/// `-v` asks for inline diffs and `--porcelain` is machine-readable, so
/// passthrough in both cases. The boilerplate drop is lossless (zero-info
/// prose) so we keep it in strict mode.
fn status_filter(ctx: &ProxyContext) -> FilterResult {
    if scope::has_any_flag(&ctx.args, scope::GIT_STATUS_SCOPE) {
        return FilterResult::default();
    }
    let ls = lines(&ctx.stdout);
    let stripped = grep_out(&ls, r#"^\s*\(use "git (add|restore|rm)"#);
    FilterResult {
        stdout: Some(join(&stripped)),
        ..Default::default()
    }
}

/// git blame: first 60 lines is enough context for most queries.
/// Strict mode: passthrough (can't trim blame losslessly).
fn blame_filter(ctx: &ProxyContext) -> FilterResult {
    if scope::has_any_flag(&ctx.args, scope::GIT_BLAME_SCOPE) || ctx.strict {
        return FilterResult::default();
    }
    let ls = lines(&ctx.stdout);
    let capped = head(&ls, 60);
    FilterResult {
        stdout: Some(join(&capped)),
        ..Default::default()
    }
}

/// git show: header + sandwich'd diff body.
/// Strict mode: passthrough.
fn show_filter(ctx: &ProxyContext) -> FilterResult {
    if scope::has_any_flag(&ctx.args, scope::GIT_SHOW_SCOPE) || ctx.strict {
        return FilterResult::default();
    }
    let ls = lines(&ctx.stdout);
    // Show: header then diff. Keep first 8 (header) + sandwich the rest.
    if ls.len() <= 150 {
        return FilterResult {
            stdout: Some(join(&ls)),
            ..Default::default()
        };
    }
    let mut out = head(&ls, 8);
    let body: Vec<String> = ls.into_iter().skip(8).collect();
    let trimmed_body = sandwich(&body, 80, 40);
    out.extend(trimmed_body);
    FilterResult {
        stdout: Some(join(&out)),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(stdout: &str, subcmd: &str) -> ProxyContext {
        ProxyContext {
            cmd: "git".into(),
            subcmd: Some(subcmd.into()),
            args: vec![subcmd.into()],
            stdout: stdout.into(),
            stderr: String::new(),
            exit: 0,
            cwd: "/".into(),
            strict: false,
        }
    }

    #[test]
    fn log_filter_caps_output() {
        let big: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let out = log_filter(&ctx(&big, "log")).stdout.unwrap();
        assert!(out.lines().count() <= 40);
    }

    #[test]
    fn status_drops_hints() {
        let input = "On branch main\n  (use \"git add <file>...\" to update)\nmodified:   foo.rs\n";
        let out = status_filter(&ctx(input, "status")).stdout.unwrap();
        assert!(!out.contains("(use \"git add"));
        assert!(out.contains("modified:"));
    }
}
