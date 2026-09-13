# Desktop input and clipboard

Implemented and exercised on September 11, 2026 with Android CP41.260814.003.B1.

## Keyboard and window focus

`compositor.rs` opens the Wayland seat keyboard, reads its XKB keymap without
changing the shared file offset, and forwards modifiers, key press/release,
repeat settings, and focus transitions. `keyboard.rs` resolves the host layout
through libxkbcommon, maps printable ASCII and navigation/modifier/function keys
to Android keycodes, and preserves each key's down-time. Focus loss releases held
keys with FLAG_CANCELED and stops repeat.

`input_channel.rs` emits the installed image's native InputMessage wire format:
104-byte key packets and **20-byte focus packets**. This Android 17 image appends
`android.os.FocusDirection` to the focus body. The older AOSP 16-byte focus packet
aborts InputConsumerNoResampling with “incorrect size 16 (expected 20)”. The layout
was confirmed against the image's `InputPublisher::publishFocusEvent` disassembly.

Input channel switches transfer focus from the old Android window to the new one.
Without these focus messages ViewRootImpl discarded every keyboard event even
though pointer clicks worked. InputService now supplies a populated virtual
KeyCharacterMap, matching the host-to-Android character translation.

## Pointer and wheel

Android PointerCoords uses a most-significant-bit-first axis bitmap:
`axis_bit(n) = 0x8000000000000000 >> n`. The old low-bit-first mask encoded X/Y
as unrelated axes, which forced coordinates into transform translations.
Coordinates now live in their real axes; transforms only account for window origin.

Wayland axis/discrete/frame events become Android ACTION_SCROLL with SOURCE_MOUSE,
mouse tool type, and vertical/horizontal scroll axes. Discrete wheels use detents;
continuous deltas are scaled by source. Vertical direction reverses between the
two APIs. Wheel down/up and ordinary clicks were tested through a real Wayland
virtual pointer; smooth touchpad behavior has not yet been exercised physically.

## Clipboard

The new `clipboard` Binder service translates the first text ClipData item into
UTF-8 on the host and returns host text as Android ClipData. `wl-copy` and
`wl-paste` provide the Wayland connection; calls have a two-second timeout.
Clipboard text travels over stdin/stdout, never shell interpolation or logs.

Tested both directions with native EditText and WebView using normal Ctrl+C/V,
including a Unicode arrow. The pre-test clipboard was privately backed up,
restored afterward, and the temporary backup deleted. Listeners receive changes
made through Android. Host-origin changes are read on demand; proactive host
selection-change notifications remain to be implemented.

This is a plain-text bridge. Rich styles, image/file clipboard data and multi-item
ClipData are not preserved. Non-ASCII keyboard composition, dead keys, arbitrary
Unicode key input and desktop IME protocols remain separate work; Unicode clipboard
text already works.

## Verification

`cargo test --workspace`: 33 tests pass. New regressions cover ASCII character-map
round trips, focus-loss release/repeat cancellation, key/focus packet layouts,
focus movement between channels, pointer axis values and window transforms, and
Unicode/malformed clipboard parcels.

The expanded `tests/webview` app verifies:

- Native typing: `NATIVE_TEXT=ARO keyboard 42!`.
- Backspace/Left/insertion: `NATIVE_TEXT=ARO keyboard 4X2`.
- Enter transfers focus to the HTML input; typing logs `WEB_TEXT=WebView typing 42!`.
- Ctrl+A replacement logs `WEB_TEXT=Shortcuts work`; held Backspace repeats.
- Tab/Enter activates the button and logs `CLICK_OK`.
- Wheel scroll logs SCROLL_Y increasing, then returning to zero.
- The HTTPS button loads example.com and returns `JS_RESULT="Example Domain:42"`.
- The unmodified image Browser2 APK renders example.com, then navigates to
  example.org using its native address bar (Ctrl+A, typing, Enter). Closing its
  Wayland window exits with status 0 and leaves none of its five tracked processes.

Logs and captures: `target/webview/validation/{keyboard,scroll,clipboard,network}.log`,
`desktop-input.png`, `browser.png`, `browser.log`, `browser-cleanup.txt`, and
`desktop-input-tests.log`. Build dependencies include libxkbcommon;
clipboard support requires wl-clipboard and coreutils (already installed on this
Omarchy machine).

Wire-format references: [Wayland keyboard protocol](https://wayland.freedesktop.org/docs/html/apa.html#protocol-spec-wl_keyboard),
[AOSP InputTransport.h](https://android.googlesource.com/platform/frameworks/native/+/master/include/input/InputTransport.h),
[AOSP BitSet.h](https://android.googlesource.com/platform/system/core/+/master/libutils/include/utils/BitSet.h).
The installed image remains authoritative when it differs from upstream source.
