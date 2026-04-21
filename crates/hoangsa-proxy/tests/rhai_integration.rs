//! End-to-end test: load a Rhai script, pick it by (cmd, subcmd), invoke
//! it against a synthetic ProxyContext, and verify the filter actually ran.

use hoangsa_proxy::registry::ProxyContext;
use hoangsa_proxy::rhai_engine::RhaiRuntime;
use std::fs;
use tempfile::TempDir;

fn ctx(cmd: &str, subcmd: &str, stdout: &str) -> ProxyContext {
    ProxyContext {
        cmd: cmd.into(),
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
fn loads_script_and_applies_filter() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join(".hoangsa-proxy");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("git.rhai"),
        r#"
        proxy::register(#{
            cmd: "git",
            subcmd: "log",
            priority: 100,
            filter: |ctx| {
                let ls = proxy::lines(ctx.stdout);
                let trimmed = proxy::head(ls, 3);
                #{ stdout: proxy::join(trimmed) }
            }
        });
        "#,
    )
    .unwrap();

    let mut rt = RhaiRuntime::new();
    rt.load_dirs(&project, None);
    assert!(rt.errors.is_empty(), "script load errors: {:?}", rt.errors);

    let handler = rt.pick("git", Some("log")).expect("handler registered");
    let big = (0..10)
        .map(|i| format!("commit {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let result = rt.invoke(&handler, &ctx("git", "log", &big)).unwrap();
    let out = result.stdout.unwrap();
    let line_count = out.lines().count();
    assert_eq!(line_count, 3, "script should have kept only 3 lines");
}

#[test]
fn project_tier_overrides_global() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("proj");
    let global = tmp.path().join("glob");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&global).unwrap();

    fs::write(
        global.join("git.rhai"),
        r#"
        proxy::register(#{
            cmd: "git", subcmd: "log", priority: 100,
            filter: |ctx| #{ stdout: "GLOBAL" }
        });
        "#,
    )
    .unwrap();
    fs::write(
        project.join("git.rhai"),
        r#"
        proxy::register(#{
            cmd: "git", subcmd: "log", priority: 10,
            filter: |ctx| #{ stdout: "PROJECT" }
        });
        "#,
    )
    .unwrap();

    let mut rt = RhaiRuntime::new();
    rt.load_dirs(&project, Some(&global));

    let handler = rt.pick("git", Some("log")).unwrap();
    let res = rt.invoke(&handler, &ctx("git", "log", "x")).unwrap();
    // Project wins regardless of priority — tier is the primary dimension.
    assert_eq!(res.stdout.as_deref(), Some("PROJECT"));
}

#[test]
fn bad_script_is_logged_not_fatal() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("proj");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("bad.rhai"), "this is not valid rhai %%%").unwrap();

    let mut rt = RhaiRuntime::new();
    rt.load_dirs(&project, None);
    assert!(!rt.errors.is_empty(), "bad script should produce error");
    assert!(rt.pick("git", Some("log")).is_none());
}
