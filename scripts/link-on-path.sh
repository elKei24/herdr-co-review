#!/usr/bin/env bash
# Symlink a co-review binary into a directory on the user's PATH, so that
# `herdr plugin install` alone is enough to run `co-review` from a shell.
#
#   link-on-path.sh <path-to-co-review>
#
# Set CO_REVIEW_NO_PATH_LINK=1 to skip. Problems only warn: the plugin's action
# and link handler work without a PATH entry. See docs/DECISIONS.md (17).
set -uo pipefail

src="${1:-}"
if [ -z "${src}" ] || [ ! -x "${src}" ]; then
  echo "usage: link-on-path.sh <path-to-co-review>" >&2
  exit 2
fi
src="$(cd "$(dirname "${src}")" && pwd)/$(basename "${src}")"

if [ -n "${CO_REVIEW_NO_PATH_LINK:-}" ]; then
  echo "CO_REVIEW_NO_PATH_LINK is set; leaving PATH alone (binary: ${src})"
  exit 0
fi

on_path() {
  case ":${PATH}:" in
    *":$1:"*) return 0 ;;
    *) return 1 ;;
  esac
}

# A `co-review` we may replace: our own symlink into this or another herdr
# plugin, or one left dangling by `herdr plugin uninstall` (a link to a file
# that no longer exists is worth nothing to anyone).
ours() {
  local link="$1" target
  [ -L "${link}" ] || return 1
  [ -e "${link}" ] || return 0
  target="$(readlink "${link}")"
  case "${target}" in
    "${src}" | */herdr/plugins/*) return 0 ;;
    *) return 1 ;;
  esac
}

# Same directory policy as install.sh — keep the two in sync — preferring a
# candidate that is already on PATH, since a link elsewhere helps nobody.
dir="${CO_REVIEW_INSTALL_DIR:-}"
if [ -z "${dir}" ]; then
  for candidate in /usr/local/bin "${HOME}/.local/bin"; do
    [ -w "${candidate}" ] || continue
    dir="${dir:-${candidate}}"
    if on_path "${candidate}"; then
      dir="${candidate}"
      break
    fi
  done
fi
dir="${dir:-${HOME}/.local/bin}"

# Never compete with a co-review the user installed themselves: shadowing it
# from another PATH directory would be as rude as overwriting it.
IFS=: read -r -a path_dirs <<<"${PATH}"
for d in "${path_dirs[@]}"; do
  [ -n "${d}" ] && [ "${d}" != "${dir}" ] || continue
  other="${d}/co-review"
  { [ -e "${other}" ] || [ -L "${other}" ]; } || continue
  if ! ours "${other}"; then
    echo "warning: ${other} is already on your PATH and we did not install it; leaving PATH alone." >&2
    echo "         The plugin's binary is at ${src}." >&2
    exit 0
  fi
done

if ! mkdir -p "${dir}" 2>/dev/null; then
  echo "warning: cannot create ${dir}; co-review stays at ${src}" >&2
  exit 0
fi

link="${dir}/co-review"
if [ -e "${link}" ] || [ -L "${link}" ]; then
  if ours "${link}"; then
    rm -f "${link}"
  else
    echo "warning: ${link} was not installed by the plugin; leaving it alone." >&2
    echo "         The plugin's binary is at ${src}. Remove ${link} to use the plugin's copy." >&2
    exit 0
  fi
fi

if ! ln -s "${src}" "${link}" 2>/dev/null; then
  echo "warning: could not link ${link} -> ${src}" >&2
  exit 0
fi
echo "Linked ${link} -> ${src}"

if ! on_path "${dir}"; then
  echo "Note: ${dir} is not on your PATH — add it, e.g. export PATH=\"${dir}:\$PATH\"" >&2
fi
