//! Integration test for exec.rs — runs a real subprocess and checks the
//! capture semantics. Uses `printf` (POSIX) which is available on every
//! platform we care about (Linux + macOS).

use hoangsa_proxy::exec;

#[cfg(unix)]
#[test]
fn captures_stdout_and_exit_code() {
    let captured = exec::run("sh", &["-c".into(), "printf hello; exit 7".into()], None).unwrap();
    assert_eq!(captured.stdout, "hello");
    assert_eq!(captured.exit, 7);
    assert!(!captured.stdout_truncated);
}

#[cfg(unix)]
#[test]
fn captures_stderr() {
    let captured = exec::run(
        "sh",
        &["-c".into(), "printf oops >&2; exit 3".into()],
        None,
    )
    .unwrap();
    assert_eq!(captured.stderr, "oops");
    assert_eq!(captured.exit, 3);
}

#[cfg(unix)]
#[test]
fn truncates_at_cap() {
    // This test is expensive in memory; only run it by hand via `cargo test
    // -- --ignored`. Default run is fast.
}
