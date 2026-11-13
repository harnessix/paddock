#!/usr/bin/env bash
# Build paddock-tui as one program for this machine and install it.
#
#   tui/scripts/install.sh [DIR]
#
# The program goes to DIR/paddock-tui. Without DIR, it goes next to the
# `paddock` on PATH, or to ~/.local/bin when PATH has no paddock.
# `paddock tui` starts the paddock-tui next to its own binary.
#
# You do not need this script for a paddock release: `paddock update` and
# the first `paddock tui` install the program of the release. It is for a
# build from the source tree: bin/install, the Arch package and a
# development build use it.
#
# The build needs Node.js 22 or newer with npm, and bun. With mise on PATH,
# the script uses the versions that tui/mise.toml names. mise downloads
# them and checks each download against the checksum in tui/mise.lock.
set -euo pipefail

tui="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
case "${1:-}" in
  -h | --help) sed -n '2,16p' "$0"; exit 0 ;;
esac
if [ "$#" -gt 1 ]; then
  echo "tui/scripts/install.sh: too many arguments. Usage: tui/scripts/install.sh [DIR]" >&2
  exit 2
fi

dest="${1:-}"
if [ -z "$dest" ]; then
  if found="$(command -v paddock)"; then
    dest="$(cd "$(dirname "$found")" && pwd -P)"
  else
    dest="$HOME/.local/bin"
  fi
fi

if command -v mise >/dev/null; then
  # Fail when a tool is not in the lockfile, in place of a download that
  # no checksum covers.
  export MISE_LOCKED=1
  tool() { (cd "$tui" && mise exec -- "$@"); }
else
  for need in node npm bun; do
    command -v "$need" >/dev/null || {
      echo "tui/scripts/install.sh: $need is not on PATH. Install mise, which gets the right node and bun, or install Node.js 22 or newer and bun." >&2
      exit 1
    }
  done
  major="$(node --version | sed 's/^v\([0-9]*\).*/\1/')"
  if [ "${major:-0}" -lt 22 ]; then
    echo "tui/scripts/install.sh: Node.js 22 or newer is needed. $(command -v node) is $(node --version)." >&2
    exit 1
  fi
  tool() { (cd "$tui" && "$@"); }
fi

echo ">>> Building the paddock TUI"
# The lockfile pins every package with its checksum. No package script runs.
tool npm ci --ignore-scripts
tool npm run build
mkdir -p "$dest"
new="$dest/.paddock-tui-new-$$"
trap 'rm -f "$new"' EXIT
tool bun scripts/bun-build.mjs native "$new"
want="paddock-tui $(sed -n 's/^  "version": "\(.*\)",$/\1/p' "$tui/package.json" | head -n 1)"
got="$("$new" --version)"
if [ "$got" != "$want" ]; then
  echo "tui/scripts/install.sh: the new program prints '$got' for --version, not '$want'. Nothing was installed." >&2
  exit 1
fi
chmod 755 "$new"
mv -f "$new" "$dest/paddock-tui"
echo ">>> Installed $dest/paddock-tui"
