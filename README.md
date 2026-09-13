# ARO — Android Runtime for Omarchy

Android apps as native user-space processes on Omarchy, integrated with Wayland
and Hyprland through a modular service bus.

- [Architecture](docs/ARCHITECTURE.md)
- [Milestones](docs/MILESTONES.md)
- [WebView](docs/WEBVIEW.md) and [desktop input](docs/DESKTOP-INPUT.md) — current runtime validation
- [Service isolation](docs/SERVICE-ISOLATION.md) — native host workers and failure checks
- [Runtime notes](docs/RUNTIME-NOTES.md) — what Android needs from the host, as discovered

## Roadmap

Android apps run as ordinary Linux processes; each Android system service is a
Rust process on a private binder bus that translates into host capabilities.
Full detail and exit criteria are in [docs/MILESTONES.md](docs/MILESTONES.md).

| Milestone | Scope | Status |
|---|---|---|
| M0 Workspace | Rust workspace, image extraction tooling (`aro-image`) | ✅ done |
| M1 Runtime | ART (`app_process64`) runs code from an unmodified APK in a user namespace, no root | ✅ done |
| M2 Bus + Activity | Private binder bus, package/activity services; a real `ActivityThread` lifecycle reaches `onResume` | ✅ done |
| M3 Display | Window session, composer, gralloc (allocator + `mapper.aro.so`), Wayland presenter; app draws into a Hyprland window, follows the tiled size, receives clicks | ✅ done |
| M4 Host capabilities | Audio playback/recording → PipeWire ✅ · Network → NetworkManager + HTTPS ✅ · Storage + SAF picker ✅ · Desktop launchers and intent bridge ✅ · Eleven host workers verified · Location → GeoClue pending live fix; fuzz targets remain | 🚧 in progress |
| M5 Accounts + GMS | Accounts on libsecret; Play Services / Play Store as tenant apps; integrity outcome documented | ⏳ planned |
| M6 Sandbox | Per-app user namespace, seccomp, Landlock from declared permissions; services enforce grants | ⏳ planned |
| M7 ARM native libs | In-process arm64 translator so arm64-only APKs run | ⏳ planned |

**Today:** `arod app some.apk` launches an app as a native process in a Hyprland
window that follows your tiling and display scale, takes clicks, keyboard input
and wheel scrolling, and exchanges plain text with the desktop clipboard. Apps reach the network (HTTPS works, `ConnectivityManager` mirrors
the desktop's connection), read and write `/sdcard` (your home), post native
desktop notifications, navigate between activities, and open via deep links —
`arod desktop-entry app.apk` registers its URL schemes so `xdg-open <scheme>://…`
launches the app at the right screen. Hardware rendering uses the ARO Vulkan HAL
and GBM buffers; audio playback is verified through PipeWire. WebView runs with a
separate Chromium renderer and loads HTTPS pages. See the milestone notes for
remaining capabilities and current limitations.

Rust toolchain is managed by mise (`.mise.toml`); `cargo build` from the repo root.

M4 verification and remaining location work: [Host capabilities](docs/HOST-CAPABILITIES.md).
Desktop setup: `bash tools/install-desktop.sh`, then `aro install app.apk` and `aro apps`.

Location support is installed on first use: an Android app requests location,
ARO asks permission, and acceptance installs GeoClue if missing. The prompt also
discloses any need to enable desktop location. Denial returns no location and
changes no packages or settings. `aro location status` checks the host setup;
`aro location install` remains available for manual package installation.
See the host capability notes for live validation and current limitations.
