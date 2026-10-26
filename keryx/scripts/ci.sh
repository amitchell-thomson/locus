#!/usr/bin/env bash
# Single source of truth for CI: run locally or from GitHub Actions.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> fmt";    cargo fmt --all --check
echo "==> clippy"; cargo clippy --workspace --all-targets -- -D warnings
echo "==> test";   cargo test --workspace
if [ -d crates/keryx-cli/tests ] && ls crates/keryx-cli/tests/perf*.rs >/dev/null 2>&1; then
  echo "==> perf budgets (release)"
  cargo test --release -p keryx-cli --test perf -- --nocapture
fi
echo "ok"
