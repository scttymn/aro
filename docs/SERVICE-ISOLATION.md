# Native host service processes

Verified September 12, 2026. ARO now launches eleven native Binder workers per
Android session. They execute the same `arod` binary through its internal
`host-service` command, inherit the session's private Binder mount and user
namespace, and register real remote Binder objects with the supervisor's hub.

| Worker | Binder service | Host state/capability |
|---|---|---|
| Connectivity | `connectivity` | NetworkManager snapshot and refresh |
| Audio | `audio` | AudioManager query/control stubs |
| AudioFlinger | `media.audio_flinger` | Existing AudioFlinger compatibility surface |
| AudioPolicy | `media.audio_policy` | Existing AudioPolicy compatibility surface |
| MediaPlayer | `media.player` | Playback, recorder objects, PipeWire/mpv/FFmpeg children |
| Storage | `mount` | Android external-storage volume metadata |
| MediaProvider | `aro.media.provider` | MediaStore index, selected documents, cursors and file descriptors |
| CalendarProvider | `aro.calendar.provider` | Calendar queries and persisted data |
| SettingsProvider | `aro.settings.provider` | SettingsProvider and persisted settings |
| Clipboard | `clipboard` | Plain-text Wayland clipboard |
| Location | `location` | Android location requests and desktop Location portal |

This is process and failure isolation. Workers still have the host user's access
and share the session namespaces. M6 permissions, seccomp/Landlock restrictions
and per-app security enforcement are separate work. Activity/task lifecycle,
package registry, display/input, notifications/shade and other compatibility
services remain in the supervisor. Audio streaming APIs have not gained new
functionality merely by moving the existing handlers.

## Shared state crosses Binder explicitly

ActivityManager returns the remote media/calendar/settings provider objects in
ContentProviderHolder parcels. Provider cursor binders and file descriptors cross
the process boundary normally; they are not copied into a second in-memory store.

The media worker also owns `aro.documents`, a supervisor-only file-picker control.
It opens the portal, registers the selected file in its own document map, and
returns a content URI. ActivityManager delivers that URI to the Android activity.
Direct Android calls to this control are rejected using the Binder caller PID.

The location worker consults `aro.location.authority` in the supervisor. Only the
registered location worker can invoke it; the authority checks the original
Android Binder PID and claimed package against the attached app. The worker receives
the existing first-use setup socket by an inherited descriptor, restored to
close-on-exec before it launches anything else. Consent and optional installation
still happen in the separate helper running as the real host user. Moving location
does not install GeoClue, enable location, or bypass the user's denial.

## Startup, failure and shutdown

Workers wait on a startup pipe until the supervisor reserves their service names
for their PID. Registration by another PID is rejected. A worker must publish all
of its objects within 30 seconds before app launch continues; exit or timeout
aborts startup and reaps workers already created.

A monitor reaps exited workers and withdraws their service registrations. Retired
reservations also reject late registration from the dying worker. The hub reports
actual service owner PIDs in ServiceDebugInfo. Existing client references receive
Binder death/errors. There is no automatic worker restart: restart the app session
to restore a failed capability and its state. Android apps that do not catch a
system-service failure may themselves exit; isolation does not promise transparent
recovery for every app.

Normal app exit explicitly stops and reaps the workers. Linux parent-death signals
also terminate workers and the ART launcher if the supervisor dies unexpectedly;
the ART launcher in turn terminates its Android PID namespace. An already approved
host package transaction is still allowed to finish safely through the setup helper.

Separating services exposed a Binder transport bug in permissive stubs: they
answered `INTERFACE_TRANSACTION` as an ordinary unknown API call. Raw handlers now
leave out-of-range Binder metadata transactions to the transport, so a remote
process can obtain their interface descriptors correctly.

## Validation

Build and run the integration probe:

```sh
cargo build -p arod -p aro-exec
bash tools/build-host.sh
target/debug/arod app target/host/host.apk --url arotest://isolation
```

The probe checks eleven distinct worker PIDs over Binder, reads calendar/settings
providers, rejects a direct document-control call and sends a forged location
package name. Expected markers include:

```text
ISOLATION distinct_workers=11
CALENDAR_PROVIDER readable=true
SETTINGS_PROVIDER readable=true
DOCUMENT_CONTROL rejected=true
LOCATION_INVALID_CALLER fix=false
```

The forged location request does not open a permission prompt or access location.
The normal network/storage probe additionally passed validated Wi-Fi, HTTPS 200,
private file read/write and `/sdcard` access. Native file selection returned a
text/plain URI with readable metadata and contents through the isolated media
provider. MediaRecorder produced AAC at 44100 Hz, mono, 1.963923 seconds. A generated
quiet WAV played through Music/AudioPreview and the isolated media player.
The microphone output was deleted after checking its format.

A controlled SIGKILL of only the connectivity worker withdrew `connectivity`.
The smoke app caught DeadSystemRuntimeException and continued private/shared
storage operations; the supervisor and unrelated workers remained alive.
A separate SIGKILL of the supervisor stopped all 17 tracked processes, including
the workers and WebView process tree, with no surviving processes or zombies.

WebView still reported `renderer=true`, `JS_RESULT="ARO WebView:42"` and provider
service connection/unbinding. GeoClue remains absent and host location remains
disabled. Its live Allow/install/fix test is still pending.

`cargo test --workspace`: **56 passed**, including worker ownership/withdrawal,
retired reservations and Binder metadata regression tests. Evidence is stored
under `target/host/validation/isolation/`. Per-AIDL fuzz targets remain pending.
