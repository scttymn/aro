# Host capability smoke app

Build and run:

```sh
bash tools/build-host.sh
target/debug/arod app target/host/host.apk
```

The app checks ConnectivityManager, performs HTTPS to example.com, and writes and
reads its private storage. For the explicit `/sdcard` check, create a file named
`~/ARO-M4-test.txt` containing `ARO file-chooser portal test` followed by a newline;
remove it after testing. The check logs `SDCARD fixture=true`.

Buttons exercise OPEN_DOCUMENT, host browser opening, a two-second microphone
recording and a consent-gated location request. Logs use tag AROHost. The file
picker should report type=text/plain, metadata=true and readable=true; dismissing
it should report canceled. Select only a test fixture.

For repeatable non-interactive invocation of individual Android calls:

```sh
target/debug/arod app target/host/host.apk --url arotest://record
target/debug/arod app target/host/host.apk --url arotest://host
target/debug/arod app target/host/host.apk --url arotest://location
```

`record` accesses the default microphone for two seconds. Its private output is
`~/.local/share/aro/data/data/org.aro.host/files/m4-recording.m4a`; inspect using
FFprobe, then remove it. `host` opens example.com in the host browser. `location`
opens ARO's first-use location prompt. Allow installs GeoClue if missing and enables
host location if disclosed by the prompt, then requests a fix through the desktop
portal. Deny changes no packages/settings and returns fix=false; subsequent requests
in the same app session stay denied. Restart the app to try the prompt again.
Administrator authentication and the portal's own permission UI may appear after
Allow. Coordinates are not printed. Only run one instance of this package at a time.

See [M4 implementation and verification](../../docs/HOST-CAPABILITIES.md).
