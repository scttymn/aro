#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
bt="$(ls -d "$HOME/.cache/aro/sdk"/android-*/ | grep -v platforms | head -1)"
out="$HOME/.local/share/aro"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

mkdir -p "$tmp/classes" "$tmp/dex"
javac --release 17 -d "$tmp/classes" "$here/bootstrap/src/org/aro/Bootstrap.java"
"$bt/d8" --release --min-api 26 --output "$tmp/dex" "$tmp/classes/org/aro/Bootstrap.class"
rm -f "$out/aro-bootstrap.jar"
(cd "$tmp/dex" && zip -q "$out/aro-bootstrap.jar" classes.dex)
echo "Built $out/aro-bootstrap.jar"
