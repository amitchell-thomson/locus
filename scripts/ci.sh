#!/usr/bin/env bash
# The CI gate for the whole Rust workspace (locus and kairos alike, L8): run locally or from
# GitHub Actions.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> fmt";    cargo fmt --all --check
echo "==> clippy"; cargo clippy --workspace --all-targets -- -D warnings
echo "==> test";   cargo test --workspace
echo "==> perf budgets (release)"
cargo test --release -p kairos-cli --test perf -- --nocapture
cargo test --release -p keryx-cli --test perf -- --nocapture
echo "ok"
