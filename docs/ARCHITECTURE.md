# ARO — Android Runtime for Omarchy

Run Android apps as native user-space processes on Omarchy, with deep integration
into the Wayland display server and Hyprland, through a modular service bus that
translates Android capabilities into native Linux calls.

## What an Android app is

An APK is dex bytecode, resources and a manifest, optionally with native
libraries. The bytecode runs on ART and calls the `android.*` framework classes.
Those classes are thin clients: nearly every one ends in a Binder call to a named
system service. The framework (`framework.jar`) is open source and reused
unmodified. What an app needs from "Android" is a set of Binder services that
honour their AIDL interfaces. That set is the service bus.

## Principles

- **Binder is the seam.** Apps speak Binder to the framework; that contract stays.
  Each capability is one AIDL system-service interface implemented by one native
  host process. Adding a capability means adding a process.
- **D-Bus and portals on the host side.** Services translate one AIDL interface to
  one host API: NetworkManager, GeoClue, PipeWire, xdg-desktop-portal, libsecret,
  xdg-open.
- **No root.** Paths, user namespaces and dmabuf fds are all unprivileged. The app
  sandbox (user namespaces, seccomp, Landlock) is its own subsystem.
- **Binder wire protocol from the `rsbinder` crate** (pure Rust, Apache-2.0,
  Android 10–17 service-manager formats). It is a protocol library, like a
  D-Bus crate; ARO's service manager and services are ARO code on top of it.
- **Rust for all ARO-authored code.** AOSP C++ libraries are consumed as prebuilt
  dependencies through one FFI crate each; `unsafe` is confined to those crates.
  Where a library would be more FFI than logic, write the Rust equivalent instead.
  One toolchain: cargo builds, tests and fuzzes (`cargo fuzz` on every AIDL handler).
- **First principles.** Decisions are argued from what the system must do, not from
  what other projects did.

## Subsystems

| # | Subsystem | AIDL / interface | Host endpoint | Notes |
|---|-----------|------------------|---------------|-------|
| 1 | Process host `aro-exec` | — | Linux process | Linked against bionic, ART, libcore; framework.jar on the boot classpath. Reads the manifest, loads the APK, instantiates Application and Activity, drives lifecycle. Replaces Zygote. |
| 2 | Service manager | Binder context manager | private binderfs instance | Kernel has binderfs and the Rust binder driver. Services register by name. |
| 3 | Package service | `IPackageManager` | `~/.local/share/aro` | Manifest and resource parsing, install database, intent resolution, resource paths. |
| 4 | Activity / Intent service | `IActivityManager` | `xdg-open`, desktop entries | Task and Activity lifecycle, intent routing, process spawning. Intents with no ARO target go to the host; host-originated intents enter here. |
| 5 | Display: gralloc | gralloc HAL | GBM / dmabuf | Buffer allocation. |
| 5 | Display: composer | `ISurfaceComposer` | Wayland `linux-dmabuf`, `xdg-shell` | Receives each Surface's BufferQueue buffers (dmabuf fds), presents one `xdg_toplevel` per task. |
| 5 | Display: window + input | `IWindowManager`, InputChannel | `wl_seat` | Activity ↔ toplevel mapping, size and density, input events written as InputMessage structs into the app's socketpair. |
| 5 | Display: GLES / Vulkan | EGL, `VK_KHR_android_surface` | Mesa (Android platform backend) | App `eglSwapBuffers` lands in our BufferQueue. |
| 6 | Audio | `IAudioFlinger`, `IAudioService` | PipeWire | AudioTrack PCM in shared memory, consumed and played; capture is the reverse. |
| 7 | Network | `IConnectivityManager`, `INetd` | NetworkManager, host resolver | Sockets are sockets; the service mirrors interface/validation state and answers DNS. |
| 8 | Storage | `IStorageManager`, `IContentProvider` | XDG dirs, file-chooser portal | App-private dirs under XDG paths, `/sdcard` → home, Storage Access Framework backed by the portal. |
| 9 | Location | `ILocationManager` | GeoClue2 | Providers backed by GeoClue. |
| 10 | Accounts | `IAccountManager` | libsecret | Account and token store with per-app grants. Google sign-in runs inside GMS, itself an app on the bus; its integrity attestation is verified by Google's servers. |
| 11 | ARM native libraries | in-process translator | — | Loaded by the linker for arm64 `.so` files. Slots into subsystem 1 later; blocks nothing above. |
| 12 | App sandbox | — | user namespaces, seccomp, Landlock | Where isolation actually comes from, independent of language. |
| 13 | System properties | bionic `/dev/__properties__` | `aro-props` | Android's key-value config bionic reads at startup; written by ARO, no init. |
| 14 | Log sink | liblog `/dev/socket/logdw` | stderr, later journald | Android logs only go here; without it nothing is visible. |

## Security model

Apps are untrusted code running as the user. Memory safety in a service matters
where that service brokers something the app is denied: accounts (tokens), storage
(files outside the sandbox), intents (launching host programs), location (a
permission), network policy. Those are privilege boundaries and the reason the
services are Rust. The composer holds nothing the app does not already own.

## Machine facts (development box)

- Kernel 7.1 with binderfs and the Rust binder driver; `/dev/binderfs` mounted.
- Intel iGPU (`renderD129`, i915) and RTX 5080 (`renderD128`, nvidia). Hyprland runs
  on the NVIDIA card. Cross-GPU buffer sharing (Intel render, NVIDIA scanout) needs
  linear modifiers and is unproven; the display milestone must test both nodes.
- `/dev/kvm` world-accessible. Docker installed; user not in the `docker` group.

## Evaluated and set aside for v1

- **Waydroid** — full Android in an LXC container with its own Wayland client; heavy, poor fit with Hyprland.
- **redroid + scrcpy** — container via Docker, Android 11 with community GApps and ARM translation, manual Play certification. Prototype CLI and APK parser from this phase live in `~/Work/omadroid` and are reusable (launcher entries, adaptive-icon-to-SVG, Omarchy menu block).
- **Google emulator** — KVM virtual machine, Play Store and ARM translation certified by Google; heaviest.
- **Droidloom** (`denialwm/droidloom`, GPL-3, Rust) — stripped Android 17 container with a native Wayland presenter, tested on Omarchy/Hyprland; no Play Store, no ARM translation, no published packages, NVIDIA unvalidated. Its `docs/contracts/` are worth reading when we design ours.
- **Android Translation Layer** — reimplements `android.*` on Linux; the pure-translation endgame, incompatible with Play Integrity.
