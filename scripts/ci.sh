#!/usr/bin/env bash
# Single source of truth for CI: run locally or from GitHub Actions.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> fmt";    cargo fmt --all --check
echo "==> clippy"; cargo clippy --workspace --all-targets -- -D warnings
echo "==> test";   cargo test --workspace
if [ -d crates/kairos-cli/tests ] && ls crates/kairos-cli/tests/perf*.rs >/dev/null 2>&1; then
  echo "==> perf budgets (release)"
  cargo test --release -p kairos-cli --test perf -- --nocapture
fi
echo "ok"
