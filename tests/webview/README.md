# WebView smoke test

Build and launch with the unpacked Android image and SDK already installed:

```sh
bash tools/build-webview.sh
cargo build -p arod -p aro-exec
bash tools/build-bootstrap.sh
target/debug/arod app target/webview/webview.apk
```

The page uses local HTML, so provider loading, renderer startup, JavaScript and
input can be checked without DNS or a remote server. Expected results:

- A window displays “WebView on ARO”.
- Android logs contain `AROWebView: renderer=true` and
  `JS_RESULT="ARO WebView:42"` (the log sink adds its own tag formatting).
- Type in the native field; logs contain `NATIVE_TEXT=...`. Enter focuses the
  HTML field, where typing logs `WEB_TEXT=...`. Backspace, arrows, Ctrl+A,
  Ctrl+C/V, Tab, Enter and held-key repeat work.
- Clicking “Test interaction” changes the button and logs `console: CLICK_OK`.
- Mouse wheel scrolling changes `SCROLL_Y`; reverse scrolling returns to zero.
- “Load HTTPS page” opens example.com and logs `JS_RESULT="Example Domain:42"`.
  This optional check requires host network access.
- A separate provider service logs `PROVIDER_SERVICE_CONNECTED` and
  `PROVIDER_SERVICE_UNBOUND`; its process then exits.
- Closing the window leaves no app or renderer processes behind.

Do not run two instances of this APK concurrently: Chromium locks its data directory.

These are runtime acceptance checks; building the APK does not establish that
they pass.
