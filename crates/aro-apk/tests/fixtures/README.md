`metadata.axml` is the compiled AndroidManifest.xml for `metadata.xml`. It keeps
metadata and renderer declaration tests independent of locally installed APKs.
To regenerate with Android SDK build tools:

```sh
aapt2 link -o /tmp/aro-metadata.apk -I /path/to/android.jar --manifest metadata.xml
unzip -p /tmp/aro-metadata.apk AndroidManifest.xml > metadata.axml
```
