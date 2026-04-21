//! P3 tests — HSP_STRICT / --strict lossless mode.
//!
//! In strict mode, no filter may drop line content (head/tail/sandwich,
//! "(xN)" annotation, line-count caps). Lossless ops (ANSI strip,
//! consecutive dedupe, blank collapse on git-log, known-noise drop) stay.

use hoangsa_proxy::handlers::{cargo, fs, git, pkg};
use hoangsa_proxy::registry::{BuiltinHandler, FilterResult, ProxyContext};

fn find<'a>(v: &'a [BuiltinHandler], cmd: &str, sub: Option<&str>) -> &'a BuiltinHandler {
    v.iter()
        .find(|h| {
            h.cmd == cmd
                && match (h.subcmd, sub) {
                    (Some(a), Some(b)) => a == b,
                    (None, None) => true,
                    _ => false,
                }
        })
        .expect("handler")
}

fn run(cmd: &str, sub: Option<&str>, args: &[&str], stdout: &str, strict: bool) -> FilterResult {
    let mut v = Vec::new();
    match cmd {
        "git" => git::register(&mut v),
        "cargo" => cargo::register(&mut v),
        "cat" | "grep" | "rg" | "find" | "ls" => fs::register(&mut v),
        "npm" | "pnpm" | "yarn" | "pip" | "pip3" => pkg::register(&mut v),
        _ => panic!("unknown"),
    }
    let h = find(&v, cmd, sub);
    let ctx = ProxyContext {
        cmd: cmd.into(),
        subcmd: sub.map(|s| s.to_string()),
        args: args.iter().map(|s| s.to_string()).collect(),
        stdout: stdout.into(),
        stderr: String::new(),
        exit: 0,
        cwd: "/".into(),
        strict,
    };
    (h.filter)(&ctx)
}

fn big(n: usize) -> String {
    (0..n).map(|i| format!("line {i}\n")).collect()
}

#[test]
fn strict_git_log_does_not_cap() {
    let out = run("git", Some("log"), &["log"], &big(200), true);
    // Strict allows blank collapse (lossless) but MUST NOT head-cap.
    let s = out.stdout.expect("blank collapse still runs");
    assert!(
        s.lines().count() >= 200,
        "strict must keep all 200 lines, got {}",
        s.lines().count()
    );
}

#[test]
fn default_git_log_still_caps() {
    let out = run("git", Some("log"), &["log"], &big(200), false);
    let s = out.stdout.expect("cap");
    assert!(s.lines().count() <= 40);
}

#[test]
fn strict_git_diff_passthrough() {
    let out = run("git", Some("diff"), &["diff"], &big(500), true);
    assert!(out.stdout.is_none(), "strict diff must passthrough");
}

#[test]
fn strict_grep_avoids_xn_annotation() {
    // default mode annotates `line (x3)`; strict must emit raw duplicate
    // dedupe (keep one instance, no suffix).
    let input = "a\na\na\nb\nc\nc\n";
    let out = run("grep", None, &[], input, true).stdout.expect("dedupe");
    assert!(!out.contains("(x"), "strict must not annotate, got: {out:?}");
    assert!(out.lines().count() == 3, "got: {out:?}");
    assert_eq!(out, "a\nb\nc\n");
}

#[test]
fn default_grep_still_annotates() {
    let input = "a\na\na\nb\n";
    let out = run("grep", None, &[], input, false).stdout.expect("collapse");
    assert!(out.contains("(x3)"));
}

#[test]
fn strict_grep_big_output_no_sandwich() {
    // 500 lines, no dups → default sandwiches to ~260, strict keeps all 500.
    let out = run("grep", None, &[], &big(500), true);
    assert!(out.stdout.is_none(), "strict must passthrough when no dedupe");
}

#[test]
fn strict_cat_passthrough() {
    let out = run("cat", None, &[], &big(1000), true);
    assert!(out.stdout.is_none());
}

#[test]
fn strict_ls_passthrough() {
    let out = run("ls", None, &[], &big(500), true);
    assert!(out.stdout.is_none());
}

#[test]
fn strict_find_passthrough() {
    let out = run("find", None, &["."], &big(500), true);
    assert!(out.stdout.is_none());
}

#[test]
fn strict_cargo_passthrough() {
    // In non-strict, cargo drops "Compiling" noise. In strict, user wants
    // to see the full build stream including progress.
    let stderr = "   Compiling foo\n   Compiling bar\nerror: boom\n";
    let mut v = Vec::new();
    cargo::register(&mut v);
    let h = find(&v, "cargo", Some("build"));
    let ctx = ProxyContext {
        cmd: "cargo".into(),
        subcmd: Some("build".into()),
        args: vec!["build".into()],
        stdout: String::new(),
        stderr: stderr.into(),
        exit: 0,
        cwd: "/".into(),
        strict: true,
    };
    let out = (h.filter)(&ctx);
    assert!(
        out.stderr.is_none(),
        "strict cargo must passthrough, got: {:?}",
        out.stderr
    );
}

#[test]
fn strict_npm_passthrough() {
    let out = run("npm", None, &["install"], "npm notice\n", true);
    assert!(out.stdout.is_none());
}

#[test]
fn strict_pip_passthrough() {
    let out = run(
        "pip",
        None,
        &["install", "foo"],
        "Collecting foo\nRequirement already satisfied: bar\n",
        true,
    );
    assert!(out.stdout.is_none());
}

#[test]
fn strict_git_status_still_drops_zero_info_hints() {
    // "(use \"git add …\")" prose is zero-information boilerplate, not
    // data — safe to drop even in strict mode.
    let input = "On branch main\n  (use \"git add <file>...\" to update)\nmodified: foo\n";
    let out = run("git", Some("status"), &["status"], input, true)
        .stdout
        .expect("still strips");
    assert!(!out.contains("(use \"git add"));
    assert!(out.contains("modified:"));
}
