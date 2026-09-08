# ARO companion widgets for Omarchy

Native Omarchy (quickshell) bar widgets that surface ARO state. They talk to
`arod` over host-standard IPC — no ARO-specific hooks in the shell — so the
shell stays stock and the widgets stay decoupled.

## `aro.notifications` — Android notification shade

An Android robot glyph in the bar that highlights (urgent color) when an ARO
app has notifications waiting, and pulls down a shade of them: app, title,
body, tap-to-open, dismiss, and Clear all. It is the persistent view Android
users expect — the freedesktop toast is still the heads-up alert, but the
shade is what you open again later.

`arod` serves the notification list over a line-delimited JSON protocol on a
Unix socket at `$XDG_RUNTIME_DIR/aro/notifications.sock` (see
`crates/arod/src/shade.rs`); this widget is a thin `Quickshell.Io.Socket`
client. Tapping a card sends `{"cmd":"invoke","id":N}` back, which rides the
same dispatch as a toast tap (re-opens the app, deep link and all).

### Install

Symlink the plugin into Omarchy's user plugin directory and add it to the bar
layout:

```bash
ln -sfn "$PWD/companion/omarchy/aro.notifications" \
  ~/.config/omarchy/plugins/aro.notifications
```

Then add `{ "id": "aro.notifications" }` to a section of
`~/.config/omarchy/shell.json` under `bar.layout` (e.g. `right`, next to
`omarchy.tray`) and `omarchy restart shell`.

### Known limits (first cut)

- One socket at a well-known path: with the current one-`arod`-per-app model,
  the most recently launched app owns the shade. A shared aro notification
  daemon would aggregate across apps.
- `ongoing` is plumbed end to end (pinned, non-dismissible in the UI) but not
  yet detected — `Notification.flags` parsing in `notification.rs` is the
  remaining piece.
