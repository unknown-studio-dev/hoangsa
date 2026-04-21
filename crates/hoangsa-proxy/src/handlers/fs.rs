//! Built-in filters for `ls`, `cat`, `grep`, `find`, `rg`.

use crate::filters::{collapse_repeats, head, join, lines, sandwich};
use crate::registry::{BuiltinHandler, FilterResult, ProxyContext};
use crate::scope;

pub fn register(v: &mut Vec<BuiltinHandler>) {
    v.push(BuiltinHandler {
        cmd: "ls",
        subcmd: None,
        priority: 50,
        filter: ls_filter,
    });
    v.push(BuiltinHandler {
        cmd: "cat",
        subcmd: None,
        priority: 50,
        filter: cat_filter,
    });
    v.push(BuiltinHandler {
        cmd: "grep",
        subcmd: None,
        priority: 50,
        filter: grep_tool_filter,
    });
    v.push(BuiltinHandler {
        cmd: "rg",
        subcmd: None,
        priority: 50,
        filter: grep_tool_filter,
    });
    v.push(BuiltinHandler {
        cmd: "find",
        subcmd: None,
        priority: 50,
        filter: find_filter,
    });
}

/// ls: if output is very long (> 200 lines), sandwich to head/tail.
/// Strict mode: passthrough.
fn ls_filter(ctx: &ProxyContext) -> FilterResult {
    if ctx.strict {
        return FilterResult::default();
    }
    let ls = lines(&ctx.stdout);
    if ls.len() <= 200 {
        return FilterResult::default();
    }
    let trimmed = sandwich(&ls, 80, 40);
    FilterResult {
        stdout: Some(join(&trimmed)),
        ..Default::default()
    }
}

/// cat: if > 500 lines, truncate head + note elision. Assumption: the caller
/// asked for a file — seeing the beginning plus a size signal is usually
/// more useful than seeing an elided middle.
fn cat_filter(ctx: &ProxyContext) -> FilterResult {
    if scope::has_any_flag(&ctx.args, scope::CAT_SCOPE) || ctx.strict {
        return FilterResult::default();
    }
    let ls = lines(&ctx.stdout);
    if ls.len() <= 500 {
        return FilterResult::default();
    }
    let mut out = head(&ls, 500);
    let remaining = ls.len() - 500;
    out.push(format!("… ({remaining} more lines — use Read tool for full) …"));
    FilterResult {
        stdout: Some(join(&out)),
        ..Default::default()
    }
}

/// grep/rg: sandwich at 300 lines and collapse consecutive dupes.
/// Passthrough for --count / -l / --max-count — output is already bounded
/// or semantically important (files-only listings).
fn grep_tool_filter(ctx: &ProxyContext) -> FilterResult {
    if scope::has_any_flag(&ctx.args, scope::GREP_SCOPE) {
        return FilterResult::default();
    }
    let ls = lines(&ctx.stdout);
    // Strict mode: never sandwich, never annotate "(xN)". At most, drop
    // exact duplicate consecutive lines silently (they're truly redundant).
    if ctx.strict {
        // collapse_repeats adds "(xN)" suffix which alters bytes — strict
        // mode needs a plain "drop consecutive duplicates, keep one" op.
        let deduped = drop_consecutive_dupes(&ls);
        if deduped.len() != ls.len() {
            return FilterResult {
                stdout: Some(join(&deduped)),
                ..Default::default()
            };
        }
        return FilterResult::default();
    }
    let collapsed = collapse_repeats(&ls);
    if collapsed.len() <= 300 {
        if collapsed.len() != ls.len() {
            return FilterResult {
                stdout: Some(join(&collapsed)),
                ..Default::default()
            };
        }
        return FilterResult::default();
    }
    let trimmed = sandwich(&collapsed, 200, 60);
    FilterResult {
        stdout: Some(join(&trimmed)),
        ..Default::default()
    }
}

/// Lossless dedup helper: keep one instance of each run of identical
/// consecutive lines. Unlike [`collapse_repeats`], no "(xN)" annotation
/// is added — the output remains a valid subset of the input.
fn drop_consecutive_dupes(ls: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(ls.len());
    for line in ls {
        if out.last() != Some(line) {
            out.push(line.clone());
        }
    }
    out
}

/// find: dedupe and sandwich at 300 lines.
fn find_filter(ctx: &ProxyContext) -> FilterResult {
    if scope::has_any_flag(&ctx.args, scope::FIND_SCOPE) || ctx.strict {
        return FilterResult::default();
    }
    let ls = lines(&ctx.stdout);
    if ls.len() <= 300 {
        return FilterResult::default();
    }
    let trimmed = sandwich(&ls, 200, 60);
    FilterResult {
        stdout: Some(join(&trimmed)),
        ..Default::default()
    }
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
    fn ls_short_untouched() {
        let res = ls_filter(&ctx("ls", "a\nb\nc\n"));
        assert!(res.stdout.is_none());
    }

    #[test]
    fn ls_long_truncated() {
        let big: String = (0..500).map(|i| format!("f{i}\n")).collect();
        let res = ls_filter(&ctx("ls", &big)).stdout.unwrap();
        assert!(res.lines().count() < 500);
        assert!(res.contains("lines trimmed"));
    }

    #[test]
    fn cat_long_truncated_at_tail() {
        let big: String = (0..700).map(|i| format!("l{i}\n")).collect();
        let res = cat_filter(&ctx("cat", &big)).stdout.unwrap();
        assert!(res.contains("more lines"));
        assert!(res.contains("l0"));
        assert!(res.contains("l499"));
        assert!(!res.contains("l650"));
    }
}
