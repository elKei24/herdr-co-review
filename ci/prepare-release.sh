#!/usr/bin/env bash
# Bump the crate version, build the release binary, and package it for the
# GitHub release. Invoked by semantic-release's @semantic-release/exec prepareCmd
# with the next version as $1.
set -euo pipefail

version="${1:?usage: prepare-release.sh <version>}"
target="x86_64-unknown-linux-gnu"

echo "Setting Cargo.toml version to ${version}"
# Replace only the first `version = "..."` (the [package] one).
sed -i -E "0,/^version = \"[^\"]*\"/s//version = \"${version}\"/" Cargo.toml

echo "Building release binary"
# Building refreshes Cargo.lock's co-review entry to the new version.
cargo build --release --locked || cargo build --release

echo "Packaging dist/co-review-${version}-${target}.tar.gz"
mkdir -p dist
tar -C target/release -czf "dist/co-review-${version}-${target}.tar.gz" co-review

echo "Done."
