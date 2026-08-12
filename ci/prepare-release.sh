#!/usr/bin/env bash
# Bump the crate version to $1 and refresh the lockfile, so the release commit
# semantic-release makes carries the right version. The cross-platform binaries
# are built separately by the release workflow's matrix job from the resulting
# tag — this script does not build anything.
set -euo pipefail

version="${1:?usage: prepare-release.sh <version>}"

echo "Setting Cargo.toml version to ${version}"
# Replace only the first `version = "..."` (the [package] one).
sed -i -E "0,/^version = \"[^\"]*\"/s//version = \"${version}\"/" Cargo.toml

echo "Setting herdr-plugin.toml version to ${version}"
sed -i -E "0,/^version = \"[^\"]*\"/s//version = \"${version}\"/" herdr-plugin.toml

echo "Updating Cargo.lock"
cargo update -p co-review --precise "${version}"

echo "Done."
