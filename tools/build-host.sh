#!/usr/bin/env bash
# Build the M4 host-capability smoke test (unsigned; loaded directly by ARO).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$here/target/host}"
mkdir -p "$out/classes" "$out/gen"
out="$(cd "$out" && pwd)"
build_tools=("$HOME"/.cache/aro/sdk/android-*/aapt2)
platforms=("$HOME"/.cache/aro/sdk/platforms/android-*/android.jar)
bt="$(dirname "${build_tools[0]}")"
jar="${platforms[${#platforms[@]}-1]}"
"$bt/aapt2" link -o "$out/host.apk" -I "$jar" \
    --manifest "$here/tests/host/AndroidManifest.xml" --java "$out/gen"
javac --release 17 -cp "$jar" -d "$out/classes" \
    "$here/tests/host/src/org/aro/host/MainActivity.java"
"$bt/d8" --release --min-api 26 --lib "$jar" --output "$out" \
    "$out"/classes/org/aro/host/*.class
(cd "$out" && zip -q host.apk classes.dex)
echo "$out/host.apk"
