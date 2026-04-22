//! Fail-open: a Rhai script that errors at runtime should not prevent the
//! caller from getting back the raw child output.

use hoangsa_proxy::registry::ProxyContext;
use hoangsa_proxy::rhai_engine::RhaiRuntime;
use std::fs;
use tempfile::TempDir;

#[test]
fn rhai_runtime_error_returns_err_not_panic() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join(".hoangsa").join("proxy");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("boom.rhai"),
        r#"
        proxy::register(#{
            cmd: "git", subcmd: "log", priority: 10,
            filter: |ctx| { throw "boom" }
        });
        "#,
    )
    .unwrap();

    let mut rt = RhaiRuntime::new();
    rt.load_dirs(&project, None);
    let handler = rt.pick("git", Some("log")).unwrap();
    let ctx = ProxyContext {
        cmd: "git".into(),
        subcmd: Some("log".into()),
        args: vec!["log".into()],
        stdout: "raw output".into(),
        stderr: String::new(),
        exit: 0,
        cwd: "/".into(),
        strict: false,
    };
    let res = rt.invoke(&handler, &ctx);
    assert!(res.is_err(), "runtime errors should surface as Err");
}
