#!/usr/bin/env bash
# Cold-start benchmark runner. Builds release and reports numbers.
#
# Usage: ./bench/bench.sh [hyperfine|rust]
#   hyperfine — uses hyperfine if installed (cleaner warmup + stats)
#   rust      — uses the in-crate #[ignore] tests (no extra deps)
#   default   — auto-detects hyperfine, falls back to rust

set -euo pipefail

cd "$(dirname "$0")/.."

MODE="${1:-auto}"
HSP_BIN=""

build_release() {
  echo "▶ building release binary"
  cargo build -p hoangsa-proxy --release --quiet
  HSP_BIN="$(cargo metadata --format-version 1 | grep -o '"target_directory":"[^"]*"' | head -1 | cut -d'"' -f4)/release/hsp"
  if [[ ! -x "$HSP_BIN" ]]; then
    echo "✗ binary not found at $HSP_BIN"
    exit 1
  fi
}

bench_hyperfine() {
  local payload='{"tool_name":"Bash","tool_input":{"command":"git log -5"}}'
  echo
  echo "▶ hook rewrite (N=200, via hyperfine)"
  echo "$payload" > /tmp/hsp-bench-payload.json
  hyperfine --warmup 5 --runs 200 --shell=none \
    "$HSP_BIN hook rewrite < /tmp/hsp-bench-payload.json"

  echo
  echo "▶ run echo hi (N=200)"
  hyperfine --warmup 5 --runs 200 --shell=none \
    "$HSP_BIN run echo hi"

  echo
  echo "▶ --version (N=200)"
  hyperfine --warmup 5 --runs 200 --shell=none \
    "$HSP_BIN --version"
}

bench_rust() {
  echo
  echo "▶ running in-crate benchmarks (release profile)"
  cargo test -p hoangsa-proxy --release --test bench_cold_start -- --ignored --nocapture 2>&1 \
    | grep -E "^\[bench\]"
}

build_release

case "$MODE" in
  hyperfine) bench_hyperfine ;;
  rust) bench_rust ;;
  auto)
    if command -v hyperfine &>/dev/null; then
      bench_hyperfine
    else
      echo "ℹ hyperfine not installed — falling back to in-crate benchmarks"
      bench_rust
    fi
    ;;
  *)
    echo "usage: $0 [hyperfine|rust]"
    exit 2
    ;;
esac

echo
echo "── interpretation ──"
echo "Daemon-needed threshold: hook rewrite p95 > 20ms on target hardware."
echo "If observed p95 is below this, the subprocess-per-call design stays."
