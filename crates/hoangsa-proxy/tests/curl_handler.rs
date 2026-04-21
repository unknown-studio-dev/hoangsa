//! End-to-end: `hsp run curl …` compacts pretty JSON bodies. We can't
//! actually call a network endpoint from a unit test, so we fake `curl`
//! by running `cat <pretty.json>` under the curl handler — it shares the
//! same stdout path.

#![cfg(unix)]

use std::fs;
use std::process::{Command, Stdio};
use tempfile::TempDir;

fn hsp_bin() -> String {
    env!("CARGO_BIN_EXE_hsp").to_string()
}

fn run(cwd: &std::path::Path, args: &[&str]) -> (String, String, Option<i32>) {
    let out = Command::new(hsp_bin())
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code(),
    )
}

/// The curl handler is keyed on `cmd == "curl"`. We create a tiny fake
/// curl binary on PATH that just prints a fixture file — then invoke
/// `hsp run curl fixture.json`. This gives us a realistic end-to-end
/// without touching the network.
fn install_fake_curl(dir: &std::path::Path, fixture: &str) -> std::path::PathBuf {
    fs::write(dir.join("body.json"), fixture).unwrap();
    let script = dir.join("curl");
    let body = format!(
        "#!/bin/sh\ncat \"{}/body.json\"\n",
        dir.display()
    );
    fs::write(&script, body).unwrap();
    // chmod +x
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).unwrap();
    script
}

#[test]
fn curl_pretty_json_compacted_e2e() {
    let tmp = TempDir::new().unwrap();
    let fake = install_fake_curl(
        tmp.path(),
        "{\n  \"name\": \"foo\",\n  \"count\": 3,\n  \"tags\": [\n    \"a\",\n    \"b\"\n  ]\n}",
    );
    // `hsp run <path-to-fake-curl> …` — the binary's **filename** is `curl`
    // so our handler-matching logic (cmd == "curl") fires.
    let (out, _err, code) = run(
        tmp.path(),
        &["run", fake.to_str().unwrap(), "https://fake/x"],
    );
    assert_eq!(code, Some(0));
    // Compact form — no newlines between keys.
    assert!(
        out.starts_with("{\"name\":\"foo\""),
        "not compacted, got: {out:?}"
    );
    // All fields present.
    assert!(out.contains("\"count\":3"));
    assert!(out.contains("\"tags\":[\"a\",\"b\"]"));
}

#[test]
fn curl_verbose_flag_passthrough_e2e() {
    let tmp = TempDir::new().unwrap();
    let fake = install_fake_curl(tmp.path(), "{\n  \"a\": 1\n}\n");
    let (out, _err, code) = run(
        tmp.path(),
        &["run", fake.to_str().unwrap(), "-v", "https://x"],
    );
    assert_eq!(code, Some(0));
    // With -v, handler passes through verbatim — pretty form preserved.
    assert!(out.contains("  \"a\": 1"), "got: {out:?}");
}

#[test]
fn curl_non_json_passthrough_e2e() {
    let tmp = TempDir::new().unwrap();
    let fake = install_fake_curl(tmp.path(), "<html><body>hi</body></html>\n");
    let (out, _err, code) = run(tmp.path(), &["run", fake.to_str().unwrap(), "https://x"]);
    assert_eq!(code, Some(0));
    assert_eq!(out, "<html><body>hi</body></html>\n");
}
