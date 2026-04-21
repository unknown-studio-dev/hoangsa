//! P1 tests — user-scoped output must not be double-capped by our filters.
//! Fixes the `git log --reverse` class of bug where a proxy ignores the
//! scope flag and returns the 40 newest commits instead of the 40 oldest.

use hoangsa_proxy::handlers::{cargo, fs, git, pkg};
use hoangsa_proxy::registry::{BuiltinHandler, FilterResult, ProxyContext};

fn ctx(cmd: &str, subcmd: Option<&str>, args: &[&str], stdout: &str) -> ProxyContext {
    ProxyContext {
        cmd: cmd.into(),
        subcmd: subcmd.map(|s| s.to_string()),
        args: args.iter().map(|s| s.to_string()).collect(),
        stdout: stdout.into(),
        stderr: String::new(),
        exit: 0,
        cwd: "/".into(),
        strict: false,
    }
}

fn find_builtin<'a>(v: &'a [BuiltinHandler], cmd: &str, sub: Option<&str>) -> &'a BuiltinHandler {
    v.iter()
        .find(|h| {
            h.cmd == cmd
                && match (h.subcmd, sub) {
                    (Some(a), Some(b)) => a == b,
                    (None, None) => true,
                    _ => false,
                }
        })
        .expect("handler registered")
}

fn run_filter(cmd: &str, sub: Option<&str>, args: &[&str], stdout: &str) -> FilterResult {
    let mut v = Vec::new();
    match cmd {
        "git" => git::register(&mut v),
        "cargo" => cargo::register(&mut v),
        "cat" | "grep" | "rg" | "find" | "ls" => fs::register(&mut v),
        "npm" | "pnpm" | "yarn" | "pip" | "pip3" => pkg::register(&mut v),
        _ => panic!("unknown cmd in test: {cmd}"),
    }
    let h = find_builtin(&v, cmd, sub);
    (h.filter)(&ctx(cmd, sub, args, stdout))
}

fn big_lines(n: usize) -> String {
    (0..n).map(|i| format!("line {i}\n")).collect()
}

#[test]
fn git_log_reverse_passthrough() {
    // User asked for oldest-first; filter must not cap.
    let out = run_filter("git", Some("log"), &["log", "--reverse"], &big_lines(200));
    assert!(
        out.stdout.is_none(),
        "--reverse must passthrough (filter returned stdout={:?})",
        out.stdout.as_ref().map(|s| s.lines().count())
    );
}

#[test]
fn git_log_n_flag_passthrough() {
    let out = run_filter("git", Some("log"), &["log", "-n", "200"], &big_lines(200));
    assert!(out.stdout.is_none());
}

#[test]
fn git_log_max_count_passthrough() {
    let out = run_filter(
        "git",
        Some("log"),
        &["log", "--max-count", "100"],
        &big_lines(200),
    );
    assert!(out.stdout.is_none());
}

#[test]
fn git_log_default_still_trims() {
    // Without scope flags, the filter still does its job.
    let out = run_filter("git", Some("log"), &["log"], &big_lines(200));
    let trimmed = out.stdout.expect("trim expected");
    assert!(trimmed.lines().count() <= 40);
}

#[test]
fn git_status_porcelain_passthrough() {
    // Machine-readable mode must NOT get its boilerplate dropped.
    let input = "## main\n M foo.rs\n?? bar.rs\n";
    let out = run_filter("git", Some("status"), &["status", "--porcelain"], input);
    assert!(out.stdout.is_none());
}

#[test]
fn git_status_verbose_passthrough() {
    let input = "On branch main\n  (use \"git add\")\nmodified: foo\n";
    let out = run_filter("git", Some("status"), &["status", "-v"], input);
    assert!(out.stdout.is_none());
}

#[test]
fn git_diff_stat_passthrough() {
    let big_diff = big_lines(500);
    let out = run_filter("git", Some("diff"), &["diff", "--stat"], &big_diff);
    assert!(out.stdout.is_none());
}

#[test]
fn cargo_message_format_json_passthrough() {
    // cargo --message-format=json is machine-readable. Filtering breaks it.
    let out = run_filter(
        "cargo",
        Some("build"),
        &["build", "--message-format=json"],
        "{\"reason\":\"compiler-message\"}\n",
    );
    assert!(out.stdout.is_none());
    assert!(out.stderr.is_none());
}

#[test]
fn cargo_test_nocapture_passthrough() {
    let out = run_filter(
        "cargo",
        Some("test"),
        &["test", "--", "--nocapture"],
        "   Compiling foo\ntest result: ok\n",
    );
    assert!(out.stdout.is_none());
}

#[test]
fn cargo_build_default_still_trims() {
    let stderr = "   Compiling foo v0.1\n   Compiling bar v0.1\nerror[E0425]\n    Finished\n";
    let out = run_filter("cargo", Some("build"), &["build"], "");
    // run_filter uses only stdout; rerun with stderr instead
    drop(out);
    let mut v = Vec::new();
    cargo::register(&mut v);
    let h = find_builtin(&v, "cargo", Some("build"));
    let c = ProxyContext {
        stderr: stderr.into(),
        ..ctx("cargo", Some("build"), &["build"], "")
    };
    let out = (h.filter)(&c).stderr.expect("trim");
    assert!(!out.contains("Compiling"));
    assert!(out.contains("error[E0425]"));
}

#[test]
fn grep_count_passthrough() {
    // -c emits a single number per file; filter would be nonsense.
    let out = run_filter("grep", None, &["-c", "foo"], "42\n");
    assert!(out.stdout.is_none());
}

#[test]
fn grep_l_passthrough() {
    let out = run_filter("grep", None, &["-l", "foo"], &big_lines(50));
    assert!(out.stdout.is_none());
}

#[test]
fn grep_default_still_trims_big() {
    let out = run_filter("grep", None, &["foo"], &big_lines(500));
    let trimmed = out.stdout.expect("trim expected");
    assert!(trimmed.lines().count() < 500);
}

#[test]
fn cat_show_all_passthrough() {
    let out = run_filter("cat", None, &["-A"], &big_lines(1000));
    assert!(out.stdout.is_none());
}

#[test]
fn find_maxdepth_passthrough() {
    let out = run_filter("find", None, &[".", "-maxdepth", "2"], &big_lines(500));
    assert!(out.stdout.is_none());
}

#[test]
fn npm_verbose_passthrough() {
    let out = run_filter("npm", None, &["install", "--verbose"], "npm notice\n");
    assert!(out.stdout.is_none());
}

#[test]
fn pip_verbose_passthrough() {
    let out = run_filter("pip", None, &["install", "-vv", "foo"], "Collecting foo\n");
    assert!(out.stdout.is_none());
}
