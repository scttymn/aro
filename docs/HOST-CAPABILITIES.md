# M4 host capabilities

Verified on September 11, 2026 with Android CP41.260814.003.B1 and kernel
7.2.4-arch1-2. **M4a, M4b and M4c exit criteria pass. M4d is not complete.**

## Audio

The native `IMediaRecorder` bridge records the default PipeWire source using
`pw-record`, encodes AAC using FFmpeg, and writes to the file descriptor supplied
by the Android app. It supports MPEG-4/AAC and ADTS/AAC output, sample rate,
mono/stereo channel count and bitrate settings. Stop/release terminates and reaps
the capture and encoder children. Parent-death signals prevent orphaned capture.
Repeated recordings truncate and rewind regular output files, avoiding old tails.
Unsupported encoders, video recording, pause/resume and unsupported parameters
return errors. This does not yet implement the separate `AudioRecord` streaming API.

`tests/host` records for two seconds. FFprobe confirmed AAC, 44100 Hz, mono,
1.921247 seconds, 25549 bytes with the app's requested settings. Earlier default
48000 Hz capture also passed. Capture processes exited. The test recording was
removed after verification; only format metadata is retained.

Playback was rechecked with unmodified Music/AudioPreview and a generated quiet
WAV tone opened through `aro open`. MediaPlayer's host bridge started playback
and reported completion through PipeWire/mpv.

The installed `libmedia.so` differs from upstream: native Surface parcel variants
at transaction IDs 6 and 23 shift later `IMediaRecorder` calls. IDs were derived
from `BnMediaRecorder::onTransact`'s jump table, with callbacks verified in ART.

## Network

NetworkManager state is refreshed every two seconds outside Binder worker threads.
Android queries see the current snapshot, including connectivity validation,
metering and transport. IP4 NameserverData dictionaries and interface names are
read from the active connection. Bus failure withdraws the advertised network.
Network arrays now include typed-object presence markers; each Network contains
one private-DNS-bypass boolean, matching the installed framework.

The host reported NetworkManager state 70, connectivity 4, metering 4; Android
reported an active, validated, unmetered Wi-Fi network and one entry in getAllNetworks.
A real HttpsURLConnection completed with HTTP 200. Offline/online snapshot changes,
validation and metering have regression tests. Physical disconnection was not
performed. NetworkCallback delivery and complete LinkProperties remain unsupported.

## Storage and file picker

The smoke app writes and reads its private files directory and reads a known test
file from the host home through `/sdcard`. OPEN_DOCUMENT and GET_CONTENT use
`org.freedesktop.portal.FileChooser.OpenFile`, carrying the requested MIME filter.
The selected file is exposed through a session-scoped `content://media/aro-files/…`
URI, with read-only file descriptors, OpenableColumns metadata and MIME lookup.
The URI reaches the calling Activity via ActivityResultItem. Cancel returns
RESULT_CANCELED. Both selection and cancellation passed in Android; the selected
text file returned text/plain, metadata and readable contents.

Directory selection, SaveFile, multiple selection and persistable document grants
remain unsupported. Existing MediaStore and CalendarProvider functionality remains
available. Selected generic files are not treated as audio-library items.

## Desktop and intents

Run `bash tools/install-desktop.sh` to link `~/.local/bin/aro` and install an
idempotent Omarchy menu block. Existing menu customizations are preserved and a
backup is made before changes. The script has been run on this machine.

- `aro install app.apk`: retain an APK in ARO's app storage and write its launcher.
- `aro apps`: choose an Android desktop launcher; launch logs go to
  `~/.cache/aro/launcher.log`.
- `aro pick`: select an APK through the desktop portal and install it.
- `aro open URL_OR_FILE`: resolve and launch an Android app.
- `aro sync`: refresh registered launchers and icons.
- `aro menu-remove`: remove only the ARO menu block.

Browser2 was installed and launched through `aro apps` using its real icon.
Desktop Exec arguments are quoted and field-code characters escaped. Launchers
without launcher Activities are hidden. Standard host URL handlers are preserved.

`aro open https://example.org/` rendered in Android Browser2. Opening a WAV named
`ARO M4 tone %.wav` resolved to Android Music and played successfully. URI encoding
and directory-boundary mapping are covered by tests. Intent resolution now rejects
untyped filters for typed files and respects declared hosts; Calendar previously
incorrectly claimed arbitrary files.

Android-to-host URL requests now use `org.freedesktop.portal.OpenURI`. Directly
spawning Chrome inside the runtime's user namespace failed its root check. The
portal launched host Chrome normally and reported success; the page was observed.

## Location: remaining M4 exit criterion

GeoClue is an **optional host dependency installed on first use**:

1. The attached Android app calls `getCurrentLocation`.
2. ARO presents an app-named Allow/Deny prompt. It explains if GeoClue must be
   installed and if desktop location services must be enabled.
3. Allow installs GeoClue when missing, using the host package manager and polkit
   authentication. Only a successful install is followed by enabling location
   when needed. Then the request proceeds to the desktop Location portal.
4. Deny, dismissal or prompt timeout returns no fix, with no package installation
   or setting change. Denial is remembered in memory for this app session, so
   repeated requests do not repeatedly prompt. Restarting the session permits retry.

A small private-socket helper is spawned before entering Android's namespaces.
Consent UI and privileged package installation therefore run as the real host user.
The helper accepts only the fixed location setup operation. The app name is checked
against the attached app and its Binder caller PID; separate service-process location
is currently unsupported because Android UIDs are still emulated. This check does
not replace the broader M6 isolation and grant-enforcement work.

Installation is serialized across sessions and rechecked after acquiring the lock.
Canceling before installation starts prevents it. An already approved package
transaction is allowed to finish safely if the Android request is canceled; the
canceled request then receives no fix and does not enable location. An existing
grant never automatically overrides a later host location disable. The setup
prompt lasts at most 60 seconds; polkit authentication/package installation may
take longer. The desktop portal may additionally present its own permission UI.

Manual package-only installation remains available as `aro location install`
(or `bash tools/setup-location.sh install` before desktop setup). That explicit
CLI command does not enable location. `aro location status` reports the package,
portal interface and preference separately; none alone proves a fix is available.
The session needs a desktop portal backend capable of presenting access prompts.

The preferred path is Android → Location portal → GeoClue. IP or manually selected
city support could be added as a separately enabled approximate mode; neither is
implemented or silently used as a fallback. Omarchy's weather widget currently
uses saved coordinates on this host, independently of the desktop Location portal.
Its selected city is not evidence of a current physical location or permission
to share it with Android apps. GeoClue sources and their service configuration
still need verification on the installed host; installation does not guarantee
GPS accuracy or a working network geolocation service.

The new `location` Binder service implements getCurrentLocation through the desktop
Location portal, its GeoClue backend, Android cancellation and a 60-second timeout.
A granted fix is marshalled as android.location.Location. Coordinates are not
logged or invented; a denied/unavailable request returns null. Host enablement is
reflected in isLocationEnabled/isProviderEnabled. The last-location API returns
null rather than exposing a fix without consent for that request. Continuous
listeners, geofences and GNSS-specific APIs remain unsupported.

**Live first-use denial verified September 12:** Android opened the actual host
prompt for `org.aro.host`; the user denied it and Android received `LOCATION fix=false`.
Afterward GeoClue remained absent and `org.gnome.system.location enabled` remained
false. The app and helper exited cleanly. Earlier, before first-use setup existed,
the real Location portal also returned `Location services disabled` as expected.

The Allow branch is covered with a fake installer/settings backend, including
install failure and cancellation, but real package installation, polkit approval,
desktop portal consent and an actual GeoClue fix remain unverified. Finishing M4d
requires a user-accepted live request through `arotest://location`; a test must
record accuracy/availability without printing coordinates. The user's live denial
has been respected; no manual installation or enablement was performed afterward.

## Validation and remaining scope

September 12: optional dependency setup/status commands added. The location
bridge now closes its session even when Start fails, validates the returned request
handle and rejects malformed timestamp fractions. Four private-D-Bus tests exercise
fast updates before the consent response, denial despite a queued fix, session
cleanup after errors, invalid timestamps, cancellation and timeout. These tests
use synthetic coordinates and do not exercise GeoClue or desktop consent UI.
The rebuilt runtime was rechecked with the Android smoke app against the disabled host:
`LOCATION fix=false`, as expected; the host preference remains false.

Six additional first-use policy tests cover denial without side effects, remembered
decisions, confirmation before installation, existing packages, install failure,
cancellation and preserving a subsequent host disable. These use fake backends.

`cargo test --workspace`: **52 passed**. Build and `git diff --check` pass.
Private-bus tests require `dbus-daemon` and never connect to the user's session bus.
The tests cover network parcels/state changes, recorder parameters, selected-file
lookup, location parcel fields, intent matching and desktop quoting/path mapping.
Artifacts are under `target/host/validation/`: Android logs, captures, launcher
checks and recording format metadata. Test windows and capture processes were
closed; generated home-directory fixtures and microphone recordings were removed.

The original milestone's separate-service-process and per-AIDL fuzz-target goals
are not delivered by this change: services still share the existing arod process,
and the new coverage is unit and live integration testing. Per-app enforcement
remains M6. This document records functional exit checks, not complete Android API
coverage or readiness to ship the development image.
