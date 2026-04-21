//! Cold-start micro-benchmark. `#[ignore]` by default — run with
//! `cargo test -p hoangsa-proxy --test bench_cold_start --release -- --ignored --nocapture`
//! to get timing numbers.
//!
//! Measures three scenarios:
//!   1. `hsp hook rewrite` with a typical Claude Code JSON payload
//!      (the hottest call — fires per PreToolUse).
//!   2. `hsp run echo hi` — full end-to-end with exec + filter pipeline.
//!   3. `hsp --version` — minimum-work cold-start floor.
//!
//! Decision rule for future daemon work: if scenario (1) p95 > 20ms on
//! the target hardware, a daemon with Unix socket is worth building. If
//! < 20ms, the subprocess-per-call design stays.

#![cfg(unix)]

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn hsp_bin() -> String {
    env!("CARGO_BIN_EXE_hsp").to_string()
}

const N: usize = 100;

fn percentile(samples: &[Duration], pct: f64) -> Duration {
    let mut sorted: Vec<_> = samples.to_vec();
    sorted.sort();
    let idx = ((sorted.len() - 1) as f64 * pct / 100.0).round() as usize;
    sorted[idx]
}

fn summarise(label: &str, samples: &[Duration]) {
    let total: Duration = samples.iter().sum();
    let avg = total / samples.len() as u32;
    let p50 = percentile(samples, 50.0);
    let p95 = percentile(samples, 95.0);
    let p99 = percentile(samples, 99.0);
    let min = *samples.iter().min().unwrap();
    let max = *samples.iter().max().unwrap();
    println!(
        "[bench] {label:<22} n={n} avg={avg:?} p50={p50:?} p95={p95:?} p99={p99:?} min={min:?} max={max:?}",
        n = samples.len(),
    );
}

fn measure_once<F: FnMut()>(mut f: F) -> Duration {
    let t = Instant::now();
    f();
    t.elapsed()
}

#[test]
#[ignore = "expensive benchmark — run explicitly with --ignored"]
fn bench_hook_rewrite() {
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"git log -5"}}"#;
    let mut samples = Vec::with_capacity(N);
    for _ in 0..N {
        let d = measure_once(|| {
            let mut child = Command::new(hsp_bin())
                .args(["hook", "rewrite"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn hsp");
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(payload.as_bytes())
                .unwrap();
            drop(child.stdin.take());
            let _ = child.wait_with_output();
        });
        samples.push(d);
    }
    summarise("hook-rewrite", &samples);
    let p95 = percentile(&samples, 95.0);
    println!(
        "[bench] daemon_needed={} (threshold=20ms, observed_p95={p95:?})",
        p95 > Duration::from_millis(20)
    );
}

#[test]
#[ignore = "expensive benchmark — run explicitly with --ignored"]
fn bench_run_echo() {
    let mut samples = Vec::with_capacity(N);
    for _ in 0..N {
        let d = measure_once(|| {
            let _ = Command::new(hsp_bin())
                .args(["run", "echo", "hi"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .output();
        });
        samples.push(d);
    }
    summarise("run-echo-hi", &samples);
}

#[test]
#[ignore = "expensive benchmark — run explicitly with --ignored"]
fn bench_version() {
    let mut samples = Vec::with_capacity(N);
    for _ in 0..N {
        let d = measure_once(|| {
            let _ = Command::new(hsp_bin())
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .output();
        });
        samples.push(d);
    }
    summarise("version", &samples);
}
