#!/usr/bin/env bash
set -euo pipefail

cargo fmt -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
