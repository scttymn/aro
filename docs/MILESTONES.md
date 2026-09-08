# ARO milestones

Each milestone has a scope, the subsystems it touches (numbered as in
ARCHITECTURE.md), and one measurable exit criterion on the development machine.
Later milestones do not start until the exit criterion of the one before is met,
except where marked parallel.

Priority (decided 2026-09-07): reach M3, an app visible on the desktop, as fast as
possible. Sandboxing and security (M6) deliberately come after the host
capabilities; services grow enforcement once the display path is proven.

## Status

| Milestone | State | Date |
|---|---|---|
| M0 | done | 2026-09-07 |
| M1 | done: `app_process64` ran `net.sourceforge.opencamera.MainActivity.useScopedStorage()` from the unmodified Open Camera APK, result `true`, 1.1 s wall | 2026-09-07 |
| M2 | done: `arod app target/hello/hello.apk` runs the test app's real `ActivityThread` lifecycle through ARO's Rust services; `HelloARO: onCreate / onStart / onResume` logged. Open Camera reaches `MainActivity.onCreate` too. | 2026-09-08 |

## M0 — Workspace

Scope: `aro` Cargo workspace; Rust via mise (project-local `.mise.toml`); an
`aro-image` tool that unpacks a published AOSP x86_64 system image (Android
sparse image format, then ext4 or erofs) into a `/system` tree under
`~/.local/share/aro/system`; CI running `cargo test` and `cargo fuzz` smoke.

Runtime source (decided 2026-09-07): extract prebuilt binaries from the latest
published AOSP x86_64 image rather than building AOSP. Current image: Android 17
QPR2 beta GSI `aosp_x86_64-exp-CP41.260814.003.B1` (2026-08-28). Its download
terms allow compatibility testing only, not redistribution, so this is a
development-only source; before ARO ships, the runtime must come from an AOSP
source build (Apache 2) at the same tag. Targeted source builds are also how the
first patches will be made.

Exit: `cargo run -p aro-image -- unpack <gsi.zip>` produces a `/system` tree
containing `bin/app_process64`, `bin/linker64`, `lib64/libart.so`,
`framework/framework.jar` and the boot classpath jars.

## M1 — Runtime (subsystem 1)

Scope: `aro-exec` boots ART with `framework.jar` on the boot classpath, loads an
APK, instantiates its `Application` class and calls `onCreate()`. No services, no
graphics. Manifest parsing only as far as finding the Application class.

Exit: `aro-exec` starts ART through `app_process64` inside a user namespace with
the unpacked `/system`, and runs a static method from an unmodified third-party
APK's dex. (`Application.onCreate()` needs a Context, hence Package and Activity
services, so it is M2's exit criterion.)

What M1 needed, all now in `aro-exec` (see docs/RUNTIME-NOTES.md for details):
unprivileged user+mount+pid+ipc namespace with the image as root; a private
binderfs instance with `binder`, `hwbinder`, `vndbinder`; system properties
written by `aro-props`; `linkerconfig` and `derive_classpath` run inside the
namespace on first start; apex-info-list.xml and aconfig flag storage generated
from the image; a stand-in `logd` socket so Android logs reach the terminal.

| M3 | next: the app dies right after `onResume` when `Activity.makeVisible` adds its window (`IInputMethodManager`, `IWindowSession`, SurfaceFlinger). | |

## M2 — Bus, Package and Activity services (subsystems 2, 3, 4)

Scope: private binderfs instance and service manager; Rust AIDL backend wired
into the build; Package service with manifest and resource parsing and an install
database under `~/.local/share/aro`; Activity service with task and Activity
lifecycle and process spawning through `aro-exec`; intent routing between ARO
apps. Fuzz targets for every AIDL handler.

Exit: an unmodified APK's `Application.onCreate()` runs, and an Activity reaches
`onCreate` and `onResume` with a real `Context`; `aro install x.apk && aro run
<package>` starts it headless.

What M2 needed beyond the bus (all in `crates/arod/src/services/`): Activity
(attach, bindApplication with 34 parameters, finishAttachApplication, the launch
`ClientTransaction`), Package (ApplicationInfo/PackageInfo/ActivityInfo from the
manifest parsed by `crates/aro-apk`), Display, PlatformCompat (evaluates the
image's compatconfig XML), User, Window and Accessibility stubs, Sensor and
Camera stubs, the IActivityClientController the app reports lifecycle to, the
application shared-memory region, the window-extensions shared libraries, and
seccomp user-notification supervision in `aro-exec` so `setpriority` with a
negative nice succeeds inside the unprivileged namespace. Formats came from
`tools/dexspec.py` over the image's `framework.jar`; the test app is
`tests/hello` built by `tools/build-hello.sh`.

## M3 — Display (subsystem 5)

Scope: gralloc over GBM; composer service receiving BufferQueue dmabufs and
presenting one `xdg_toplevel` per task via `linux-dmabuf`; window service mapping
Activities to toplevels with size and density; input from `wl_seat` written into
the app's InputChannel; Mesa's Android platform backend against our gralloc for
EGL/GLES and Vulkan. Test on both render nodes (Intel and NVIDIA) and record the
cross-GPU modifier result.

Exit: one Activity draws one frame into one Hyprland window, resizes when tiled,
and receives a click. This is the vertical slice.

## M4 — Host capabilities (subsystems 6, 7, 8, 9) — parallel

Each is one service process, one AIDL interface, one host API, with its own fuzz
target and unit tests that run without Android.

- **M4a Audio** — exit: an app plays sound through PipeWire and records from the default source.
- **M4b Network** — exit: an app performs an HTTPS request and `ConnectivityManager` reports a validated network mirrored from NetworkManager.
- **M4c Storage** — exit: an app saves to its private dir, reads `/sdcard` mapped to the home directory, and a Storage Access Framework picker opens the file-chooser portal.
- **M4d Location** — exit: an app receives a fix from GeoClue and the permission prompt gates it.

Also in M4: host-side intent bridge (an app opens a URL → Omarchy browser; `aro open <url|file>` → Android), desktop entries with real icons, Omarchy menu block (port from `~/Work/omadroid`).

## M5 — Accounts and GMS (subsystem 10)

Scope: accounts service on libsecret with per-app grants; install Google Play
Services and the Play Store as tenant apps; decide what the integrity outcome is
on this bus and document it.

Exit: an app that uses `AccountManager` obtains a token; the Play Store outcome
(sign-in, integrity) is recorded either way.

## M6 — Sandbox (subsystem 12)

Scope: per-app user namespace, seccomp profile, Landlock rules derived from
declared permissions; services enforce grants by peer credentials.

Exit: an app cannot read outside its granted paths, and a deliberately hostile
test app is contained.

## M7 — ARM native libraries (subsystem 11)

Scope: in-process translator loaded by the linker for arm64 `.so` files.

Exit: an arm64-only APK runs to its first frame.

## Later

Notifications, clipboard, drag and drop, IME integration, camera, Bluetooth,
multi-user, packaging for Omarchy (`omarchy install aro`).
