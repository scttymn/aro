# WebView bring-up

ARO loads the image's WebView provider through Android's normal package and
Binder APIs. The app retains its Android WebView API and native Wayland window.
Chromium's renderer must run in a separate process on the same ARO Binder bus.

## Provider discovery

Application string metadata is read from the APK manifest and carried into
ApplicationInfo. This supplies `com.android.webview.WebViewLibrary` without
hardcoding the library name. Boolean, numeric and resource metadata remain
unsupported by this string-only representation.

PackageManager's WebView feature and the `webviewupdate` service use the same
provider lookup. A missing provider or missing library metadata returns a null
package and WebViewFactory error 4. The framework's `isWebViewSupported()` checks
PackageManager; bootstrap no longer changes its private field through reflection.
The synchronous `notifyRelroCreationCompleted` call writes the exception header
expected by the installed framework's Proxy.

The bundled provider is still fixed to `com.android.webview`; this is not a
general provider installation or selection implementation. Provider discovery
does not establish that the native library or renderer can start.

## Runtime verified September 11, 2026

Booted `7.2.4-arch1-2`; `python tools/check-binder.py` opens Binder successfully
and reports protocol 8. The WebView smoke app loads Chromium 149.0.7827.5,
displays local HTML in a Wayland window, and reports:

```
renderer=true
JS_RESULT="ARO WebView:42"
console: CLICK_OK
PROVIDER_SERVICE_BIND=true
PROVIDER_SERVICE_CONNECTED=ComponentInfo{com.android.webview/org.chromium.android_webview.services.VariationsSeedServer}
PROVIDER_SERVICE_UNBOUND
```

The renderer runs in a separate Android process with compatibility UID 99000.
The provider's ordinary `webview_service` runs separately with UID 10002 and
is cleaned up after the test unbinds. The final run stayed up for over 90 seconds
without a fatal exception. Closing its Wayland window exited all five captured
session processes (supervisor, launchers, app and renderer). Validation logs,
process cleanup results and captures are in `target/webview/validation/`.
See the smoke test below for rebuilding.

## Resolved kernel blocker

On kernel `7.2.3-arch1-3`, private binderfs mount and device creation succeed, but
opening the resulting Binder device returns `ESRCH` (No such process). This
happens before ARO initializes its service manager or starts Android. It also
reproduces with a small Python program that imports no ARO code:

```sh
python tools/check-binder.py
```

The check creates user, IPC and mount namespaces in a child, mounts binderfs,
creates a device with BINDER_CTL_ADD, and opens it. Mounts disappear when the
child exits.

The failure was traced to incorrect Rust bindings in the installed Arch kernel:
disassembling its `rust_binder_open` shows a load of `task_struct.mm` at offset
`0xaf8`, while `get_task_mm` and BTF place that member at `0xab8`. The code reads
the wrong field and returns ESRCH. This matches the
[reported regression](https://github.com/waydroid/waydroid/issues/2411), caused by
[bindgen's struct padding alignment bug](https://github.com/rust-lang/rust-bindgen/pull/3449).

Arch `linux 7.2.4.arch1-2` was rebuilt with bindgen 0.73.2. Both the kernel and
matching headers were downloaded from the Arch archive and their package
signatures verified. Inspection of the replacement `vmlinux` confirms both Rust
Binder and C load `mm` at `0xab8`. The replacement kernel is now booted, and Binder opens successfully.

Kernel repair artifacts and installation output are in
`~/.cache/aro/kernel-fix/`. After reboot, check `uname -r`, run the Binder check,
then run the WebView smoke test below. Do not treat a successful package install
as a successful runtime test.

Installed `linux` and `linux-headers` 7.2.4.arch1-2 on September 11. Recovery
snapshot 9 was created before installation. NVIDIA 610.57.04 DKMS rebuilt for
7.2.4-arch1-2, and the Limine hook generated `/boot/EFI/Linux/omarchy_linux.efi`.
Post-reboot verification confirmed the running kernel and Binder protocol.

## Runtime implementation

`ApplicationSharedMemory` now publishes the same feature versions as
PackageManager. The image's `SystemFeaturesCache` reads this region before
calling PackageManager; marking all entries unavailable prevented provider
initialization. `spec/system_features.rs` records the image's 191 SDK features
in Android ArraySet hash order (WebView is entry 134).

`ActivityService` routes remote service launches by Android start sequence.
`aro-exec` forwards `seq=` to ActivityThread through the bootstrap, joins the
embedding app's PID namespace, and preserves separate mount namespaces and ART
processes. The app and renderer therefore see consistent, distinct PIDs on their
shared Binder bus. External service ApplicationInfo keeps provider code while
using the embedding app's package/data context. The renderer's completion does
not launch another Activity or overwrite the app's IApplicationThread.

Service bind tokens retain their owning thread, service token and Intent.
Publication goes to the requesting IServiceConnection. Unbind sends the image's
three-argument `scheduleUnbindService`; the final unbind destroys and terminates
the remote service process, with a bounded cleanup grace period and child reaping.
`stopSelf` leaves a service alive while bindings remain. Parent-death signals and
the app PID namespace tie renderer lifetime to the session.

The camera service now has transaction codes recovered from the image's Proxy
and returns empty native vendor-tag descriptors/cache. This prevents Chromium's
camera availability observer from crashing on an unknown transaction. Native
parcel layouts were checked against
[AOSP VendorTagDescriptor.cpp](https://android.googlesource.com/platform/frameworks/av/+/master/camera/VendorTagDescriptor.cpp).

## Validation and limits

Run [the smoke test](../tests/webview/README.md). The workspace has 33 passing
unit tests, including provider discovery and shared-memory feature consistency.
The smoke app also explicitly binds/unbinds the provider maintenance service to
exercise the ordinary remote-process path without depending on a seed download.

This verifies local HTML, JavaScript, rendering and input. The expanded smoke test
also loads example.com over HTTPS, and the unmodified Browser2 APK displays it.
Keyboard editing, scrolling and clipboard exchange are described in
[Desktop input](DESKTOP-INPUT.md). Arbitrary WebView applications, video playback
and DRM are not yet verified. Provider selection
remains fixed to the bundled `com.android.webview`. UID values are Android
compatibility identities supplied by syscall supervision; all processes still map
to the host user's kernel identity. Full Android isolated-UID access control and
seccomp policy are not implemented. General service process coalescing, automatic
restart after death, and complete Android service priority/permission policy also
remain future work.

Run one instance of a given app at a time: Chromium locks its WebView data directory.
