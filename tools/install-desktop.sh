#!/usr/bin/env bash
# Link the checkout's desktop CLI and add its Omarchy menu block.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
bindir="${XDG_BIN_HOME:-$HOME/.local/bin}"
mkdir -p "$bindir"
if [[ -e "$bindir/aro" && ! -L "$bindir/aro" ]]; then
  printf 'Refusing to replace existing file: %s\n' "$bindir/aro" >&2
  exit 1
fi
ln -sfn "$root/bin/aro" "$bindir/aro"
"$root/bin/aro" menu-install
