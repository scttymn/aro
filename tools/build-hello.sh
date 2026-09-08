#!/usr/bin/env bash
# Build tests/hello into a signed APK using the SDK build-tools and platform
# jar that `aro` setup downloads (android.jar, aapt2, d8, apksigner).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
src="$here/tests/hello"
out="${1:-$here/target/hello}"
bt="$(ls -d "$HOME/.cache/aro/sdk"/android-*/ | grep -v platforms | head -1)"
jar="$(ls "$HOME/.cache/aro/sdk/platforms"/android-*/android.jar | tail -1)"
mkdir -p "$out/compiled" "$out/classes"
"$bt/aapt2" compile --dir "$src/res" -o "$out/compiled/res.zip" 2>/dev/null || true
"$bt/aapt2" link -o "$out/hello-unsigned.apk" -I "$jar" --manifest "$src/AndroidManifest.xml" $( [ -s "$out/compiled/res.zip" ] && echo "$out/compiled/res.zip" ) --java "$out/gen"
find "$src/src" "$out/gen" -name '*.java' > "$out/sources.txt"
javac --release 17 -cp "$jar" -d "$out/classes" @"$out/sources.txt"
"$bt/d8" --release --min-api 26 --lib "$jar" --output "$out" $(find "$out/classes" -name '*.class')
cp "$out/hello-unsigned.apk" "$out/hello-dex.apk"
(cd "$out" && zip -q hello-dex.apk classes.dex)
if [ ! -f "$out/debug.keystore" ]; then
  keytool -genkeypair -keystore "$out/debug.keystore" -storepass android -keypass android -alias aro -keyalg RSA -keysize 2048 -validity 3650 -dname "CN=ARO, O=ARO" >/dev/null 2>&1
fi
"$bt/apksigner" sign --ks "$out/debug.keystore" --ks-pass pass:android --key-pass pass:android --out "$out/hello.apk" "$out/hello-dex.apk"
echo "$out/hello.apk"
