#!/bin/sh
# co-review installer: downloads the latest prebuilt binary for your platform.
#
#   curl -fsSL https://raw.githubusercontent.com/elKei24/herdr-co-review/main/install.sh | sh
#
# Override the install dir with CO_REVIEW_INSTALL_DIR=/some/bin.
set -eu

REPO="elKei24/herdr-co-review"

os="$(uname -s)"
arch="$(uname -m)"
case "${arch}" in
  x86_64 | amd64) arch=x86_64 ;;
  arm64 | aarch64) arch=aarch64 ;;
  *) echo "unsupported architecture: ${arch}" >&2; exit 1 ;;
esac
case "${os}" in
  Linux) triple="${arch}-unknown-linux-gnu" ;;
  Darwin) triple="${arch}-apple-darwin" ;;
  *) echo "unsupported OS: ${os} (download a binary from https://github.com/${REPO}/releases)" >&2; exit 1 ;;
esac

# Pick an install directory: the override, else a writable standard location,
# preferring one already on PATH. Same policy as scripts/link-on-path.sh, which
# does this for the Herdr plugin — keep the two in sync.
dir="${CO_REVIEW_INSTALL_DIR:-}"
if [ -z "${dir}" ]; then
  for candidate in /usr/local/bin "${HOME}/.local/bin"; do
    [ -w "${candidate}" ] || continue
    case ":${PATH}:" in
      *":${candidate}:"*)
        dir="${candidate}"
        break
        ;;
    esac
  done
fi
dir="${dir:-${HOME}/.local/bin}"
mkdir -p "${dir}"

url="https://github.com/${REPO}/releases/latest/download/co-review-${triple}.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT

echo "Downloading ${url}"
if ! curl -fsSL "${url}" -o "${tmp}/co-review.tar.gz"; then
  echo "error: no prebuilt binary for ${triple}." >&2
  echo "       See https://github.com/${REPO}/releases, or build with: cargo install --git https://github.com/${REPO}" >&2
  exit 1
fi
tar -C "${tmp}" -xzf "${tmp}/co-review.tar.gz"
install -m 0755 "${tmp}/co-review" "${dir}/co-review"

echo "Installed co-review to ${dir}/co-review"
"${dir}/co-review" --version || true
case ":${PATH}:" in
  *":${dir}:"*) ;;
  *) echo "Note: ${dir} is not on your PATH — add it, e.g. export PATH=\"${dir}:\$PATH\"" ;;
esac
