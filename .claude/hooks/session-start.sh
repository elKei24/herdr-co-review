#!/usr/bin/env bash
# Runs when a Claude Code session starts (including on the web) so the toolchain
# is ready to build, test, and lint co-review without a first-run stumble.
set -euo pipefail

# Ensure the components CI relies on are present (no-ops if already installed).
rustup component add rustfmt clippy >/dev/null 2>&1 || true

# Warm the dependency cache so the first `cargo build`/`cargo test` is quick.
cargo fetch >/dev/null 2>&1 || true

echo "co-review dev environment ready ($(cargo --version 2>/dev/null || echo 'cargo missing'))."
echo "Common commands: cargo test · cargo clippy --all-targets -- -D warnings · cargo fmt --all"
