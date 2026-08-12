#!/usr/bin/env bash
# Entry point for the plugin's herdr actions. Herdr runs action commands with
# the plugin root as cwd and a minimal PATH, so prepend the usual bin dirs
# (git, gh) and invoke the installed binary by absolute path.
set -euo pipefail

PATH="$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"
export PATH

root="${HERDR_PLUGIN_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
exec "$root/bin/co-review" "$@"
