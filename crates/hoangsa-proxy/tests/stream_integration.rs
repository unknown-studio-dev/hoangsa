//! Streaming exec tests — verifies the concurrent reader threads handle
//! loud stdout + loud stderr without deadlocking on OS pipe buffers, and
//! that the 100 MB hard cap + 10 MB warn threshold flip correctly.
//!
//! The big-output test runs in a few hundred ms; the cap test is
//! `#[ignore]` by default because it allocates ~100 MB and slows CI.

use hoangsa_proxy::exec::{self, OUTPUT_CAP_BYTES, WARN_THRESHOLD_BYTES};

#[cfg(unix)]
#[test]
fn concurrent_capture_does_not_deadlock() {
    // Child writes >64KB to BOTH stdout AND stderr, larger than typical
    // pipe buffers. The old sequential reader (output()) would block if
    // stderr filled before stdout was consumed. Our thread-per-stream
    // reader must finish promptly.
    //
    // 200 KB per stream.
    let script = r#"
        yes 'A' | head -c 200000
        yes 'B' | head -c 200000 1>&2
    "#;
    let c = exec::run("sh", &["-c".into(), script.into()], None).expect("run");
    assert_eq!(c.exit, 0);
    assert_eq!(c.stdout.len(), 200_000);
    assert_eq!(c.stderr.len(), 200_000);
    assert!(!c.stdout_truncated);
    assert!(!c.stderr_truncated);
    assert!(!c.stdout_warn);
    assert!(!c.stderr_warn);
}

#[cfg(unix)]
#[test]
fn warn_threshold_flips_at_10mb() {
    // Exactly WARN_THRESHOLD_BYTES on stdout — the warn flag must be set,
    // but truncation must not.
    let snippet = format!("yes 'A' | head -c {}", WARN_THRESHOLD_BYTES);
    let c = exec::run("sh", &["-c".into(), snippet], None).expect("run");
    assert_eq!(c.exit, 0);
    assert_eq!(c.stdout.len(), WARN_THRESHOLD_BYTES);
    assert!(c.stdout_warn, "warn must be set at threshold");
    assert!(!c.stdout_truncated, "must not truncate below cap");
}

#[cfg(unix)]
#[test]
fn total_bytes_reports_unclipped_volume() {
    // Even when we'd hit the cap, the `total` counter keeps counting so
    // the trim report can say "we saw X, kept Y".
    //
    // Small output — verify total matches output size.
    let c = exec::run("sh", &["-c".into(), "printf 'abcdef'".into()], None).expect("run");
    assert_eq!(c.stdout_total_bytes, 6);
    assert_eq!(c.stdout, "abcdef");
}

#[cfg(unix)]
#[test]
fn empty_output_has_clean_flags() {
    let c = exec::run("sh", &["-c".into(), "true".into()], None).expect("run");
    assert_eq!(c.exit, 0);
    assert_eq!(c.stdout, "");
    assert_eq!(c.stderr, "");
    assert!(!c.stdout_warn);
    assert!(!c.stderr_warn);
    assert_eq!(c.stdout_total_bytes, 0);
}

#[cfg(unix)]
#[test]
fn exit_code_survives_thread_reader() {
    // Sanity check that the thread-based reader didn't break exit-code
    // capture.
    let c = exec::run("sh", &["-c".into(), "printf hi; exit 42".into()], None).expect("run");
    assert_eq!(c.exit, 42);
    assert_eq!(c.stdout, "hi");
}

#[cfg(unix)]
#[test]
#[ignore = "allocates ~100MB; run with --ignored"]
fn hard_cap_truncates_at_100mb() {
    // Overshoot the cap to prove we drop the overflow without blocking the
    // child. Produce 101 MB on stdout.
    let snippet = format!("yes 'A' | head -c {}", OUTPUT_CAP_BYTES + 1024 * 1024);
    let c = exec::run("sh", &["-c".into(), snippet], None).expect("run");
    assert!(c.stdout_truncated, "must flag truncation at cap");
    assert!(c.stdout_warn, "warn must also be set when past cap");
    assert_eq!(c.stdout.len(), OUTPUT_CAP_BYTES);
    // total counts pre-cap bytes we saw on the pipe
    assert!(c.stdout_total_bytes >= OUTPUT_CAP_BYTES);
}
