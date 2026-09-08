# ARO — Android Runtime for Omarchy

Android apps as native user-space processes on Omarchy, integrated with Wayland
and Hyprland through a modular service bus.

- [Architecture](docs/ARCHITECTURE.md)
- [Milestones](docs/MILESTONES.md)
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
| M4 Host capabilities | Notifications → freedesktop ✅ · Network → NetworkManager + host DNS ✅ · Storage → home dir ✅ (SAF picker pending intents) · Audio → PipeWire · Location → GeoClue · Intent dispatcher ✅ + deep links ✅ (`arod desktop-entry` registers schemes; PendingIntent/SAF next) | 🚧 in progress |
| M5 Accounts + GMS | Accounts on libsecret; Play Services / Play Store as tenant apps; integrity outcome documented | ⏳ planned |
| M6 Sandbox | Per-app user namespace, seccomp, Landlock from declared permissions; services enforce grants | ⏳ planned |
| M7 ARM native libs | In-process arm64 translator so arm64-only APKs run | ⏳ planned |

**Today:** `arod app some.apk` launches an app as a native process in a Hyprland
window that follows your tiling and display scale, takes clicks, and keeps
rendering. Apps reach the network (HTTPS works, `ConnectivityManager` mirrors
the desktop's connection), read and write `/sdcard` (your home), post native
desktop notifications, navigate between activities, and open via deep links —
`arod desktop-entry app.apk` registers its URL schemes so `xdg-open <scheme>://…`
launches the app at the right screen. Rendering is software (the GSI has no GPU
driver path yet); audio and location are still to come.

Rust toolchain is managed by mise (`.mise.toml`); `cargo build` from the repo root.
