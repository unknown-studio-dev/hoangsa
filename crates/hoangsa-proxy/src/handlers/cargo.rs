//! Built-in filters for `cargo`.
//!
//! The dominant source of noise is the `Compiling <crate> v…` stream and
//! `Checking …` during `cargo build|test|check|clippy`. We keep the final
//! summary line, any warnings/errors, and trim the progress chatter.

use crate::filters::{grep_out, join, lines};
use crate::registry::{BuiltinHandler, FilterResult, ProxyContext};
use crate::scope;

pub fn register(v: &mut Vec<BuiltinHandler>) {
    const SUBS: &[&str] = &["build", "check", "test", "clippy", "run"];
    for sub in SUBS {
        v.push(BuiltinHandler {
            cmd: "cargo",
            subcmd: Some(sub),
            priority: 50,
            filter: compile_filter,
        });
    }
    v.push(BuiltinHandler {
        cmd: "cargo",
        subcmd: None,
        priority: 0,
        filter: passthrough,
    });
}

fn passthrough(_ctx: &ProxyContext) -> FilterResult {
    FilterResult::default()
}

/// Preserve: warning/error lines, `Finished` summary, test output, panics.
/// Drop:     `Compiling …`, `Checking …`, `Downloaded …`, `Updating …`,
///           `Blocking waiting for …`, blank lines between progress noise.
fn compile_filter(ctx: &ProxyContext) -> FilterResult {
    // `--message-format json`, `--nocapture`, `-v`: user asked for exact
    // output, don't second-guess. Strict mode: dropping "Compiling …"
    // lines loses the fact that a build ran — err on passthrough.
    if scope::has_any_flag(&ctx.args, scope::CARGO_SCOPE) || ctx.strict {
        return FilterResult::default();
    }
    let stderr_filtered = filter_stream(&ctx.stderr);
    let stdout_filtered = filter_stream(&ctx.stdout);
    FilterResult {
        stdout: Some(stdout_filtered),
        stderr: Some(stderr_filtered),
        ..Default::default()
    }
}

fn filter_stream(raw: &str) -> String {
    let ls = lines(raw);
    // Drop the progress chatter.
    let noise = r"^\s*(Compiling|Checking|Downloaded|Downloading|Updating|Blocking|Fresh|Documenting|Compiling)\b";
    let cleaned = grep_out(&ls, noise);
    // Drop redundant blank runs (simple: remove all blank lines — cargo is
    // structured enough that we don't need them for readability).
    let cleaned = grep_out(&cleaned, r"^\s*$");
    // Force-keep important lines back by re-grep on the original input: any
    // `warning:`, `error:`, `note:`, `help:`, `Finished`, `test …`, `panicked`,
    // `thread '…' panicked` — they were already kept by the drops above, so
    // just pass through.
    join(&cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(stderr: &str) -> ProxyContext {
        ProxyContext {
            cmd: "cargo".into(),
            subcmd: Some("build".into()),
            args: vec!["build".into()],
            stdout: String::new(),
            stderr: stderr.into(),
            exit: 0,
            cwd: "/".into(),
            strict: false,
        }
    }

    #[test]
    fn drops_compiling_keeps_errors() {
        let input = "   Compiling foo v0.1\n   Compiling bar v0.1\nerror[E0425]: cannot find value `x`\nwarning: unused variable\n    Finished dev [unoptimized]\n";
        let out = compile_filter(&ctx(input)).stderr.unwrap();
        assert!(!out.contains("Compiling"));
        assert!(out.contains("error[E0425]"));
        assert!(out.contains("warning: unused"));
        assert!(out.contains("Finished"));
    }

    #[test]
    fn drops_blank_lines() {
        let input = "   Compiling foo\n\n\nerror: bad\n\n";
        let out = compile_filter(&ctx(input)).stderr.unwrap();
        assert_eq!(out.matches('\n').count(), 1);
    }

}
