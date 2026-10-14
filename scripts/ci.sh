#!/usr/bin/env bash
# Local checks that mirror .github/workflows/ci.yml
# (fmt, clippy, test, coverage, audit/deny, release binary).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

run() {
  echo ""
  echo "==> $*"
  "$@"
}

run cargo fmt --all -- --check
run cargo clippy --all-targets --all-features -- -D warnings
run cargo test --all-targets
run cargo test --all-targets --all-features
run cargo test --no-default-features
run cargo doc --no-deps

echo ""
echo "==> cargo llvm-cov (CI gate: ≥80% lines)"
if ! cargo llvm-cov --version >/dev/null 2>&1; then
  echo "error: cargo-llvm-cov is required. Install with:" >&2
  echo "  rustup component add llvm-tools-preview" >&2
  echo "  cargo install cargo-llvm-cov" >&2
  exit 1
fi
if ! cargo llvm-cov --all-features --lcov --output-path lcov.info \
  --ignore-filename-regex 'src/main\.rs' \
  --fail-under-lines 80; then
  echo "error: line coverage is below 80% — CI Coverage will fail." >&2
  exit 1
fi

run cargo deny check

if cargo audit --version >/dev/null 2>&1; then
  run cargo audit
else
  echo ""
  echo "==> cargo audit (skipped — install with: cargo install cargo-audit)"
fi

run cargo build --release
run ./target/release/gnosis about

echo ""
echo "All CI checks passed."
