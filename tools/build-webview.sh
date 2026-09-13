#!/usr/bin/env bash
# Build the local-content WebView smoke test (unsigned; loaded directly by ARO).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$here/target/webview}"
mkdir -p "$out/classes" "$out/gen"
out="$(cd "$out" && pwd)"
build_tools=("$HOME"/.cache/aro/sdk/android-*/aapt2)
platforms=("$HOME"/.cache/aro/sdk/platforms/android-*/android.jar)
bt="$(dirname "${build_tools[0]}")"
jar="${platforms[${#platforms[@]}-1]}"
"$bt/aapt2" link -o "$out/webview.apk" -I "$jar" \
    --manifest "$here/tests/webview/AndroidManifest.xml" --java "$out/gen"
javac --release 17 -cp "$jar" -d "$out/classes" \
    "$here/tests/webview/src/org/aro/webview/MainActivity.java"
"$bt/d8" --release --min-api 26 --lib "$jar" --output "$out" \
    "$out"/classes/org/aro/webview/*.class
(cd "$out" && zip -q webview.apk classes.dex)
echo "$out/webview.apk"
