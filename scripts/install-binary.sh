#!/usr/bin/env bash
# Put a `co-review` binary in ./bin, preferring a prebuilt release asset and
# falling back to a local `cargo build`. Used by the Herdr plugin's build step
# (see herdr-plugin.toml) so installing the plugin does not require a Rust
# toolchain once releases exist. Safe to run standalone too.
set -euo pipefail

# Herdr runs plugin build commands with a minimal PATH; make sure curl/tar/cargo
# from the usual locations are reachable.
PATH="$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"
export PATH

REPO="elKei24/herdr-co-review"
root="$(cd "$(dirname "$0")/.." && pwd)"
bindir="${root}/bin"
mkdir -p "${bindir}"

# Map the host to a Rust target triple used in the release asset names.
os="$(uname -s)"
arch="$(uname -m)"
case "${arch}" in
  x86_64 | amd64) arch=x86_64 ;;
  arm64 | aarch64) arch=aarch64 ;;
  *) arch="" ;;
esac
case "${os}" in
  Linux) triple="${arch}-unknown-linux-gnu" ;;
  Darwin) triple="${arch}-apple-darwin" ;;
  *) triple="" ;;
esac

download() {
  [ -n "${triple}" ] || return 1
  local url="https://github.com/${REPO}/releases/latest/download/co-review-${triple}.tar.gz"
  local tmp
  tmp="$(mktemp -d)"
  echo "Fetching prebuilt binary: ${url}"
  if curl -fsSL "${url}" -o "${tmp}/co-review.tar.gz" 2>/dev/null; then
    tar -C "${tmp}" -xzf "${tmp}/co-review.tar.gz"
    mv "${tmp}/co-review" "${bindir}/co-review"
    chmod +x "${bindir}/co-review"
    rm -rf "${tmp}"
    return 0
  fi
  rm -rf "${tmp}"
  return 1
}

build() {
  command -v cargo >/dev/null 2>&1 || return 1
  echo "No prebuilt binary available; building from source with cargo"
  (cd "${root}" && cargo build --release)
  cp "${root}/target/release/co-review" "${bindir}/co-review"
  chmod +x "${bindir}/co-review"
}

if download; then
  echo "Installed prebuilt co-review -> ${bindir}/co-review"
elif build; then
  echo "Built co-review -> ${bindir}/co-review"
else
  echo "error: could not download a prebuilt binary (no release asset for '${triple:-unknown}')" >&2
  echo "       and cargo is not available to build from source." >&2
  echo "       Install Rust (https://rustup.rs) and re-run, or download a binary from" >&2
  echo "       https://github.com/${REPO}/releases and place it at ${bindir}/co-review." >&2
  exit 1
fi

"${bindir}/co-review" --version
