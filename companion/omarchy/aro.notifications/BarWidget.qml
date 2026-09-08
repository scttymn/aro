import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

// The Android affordance in the Omarchy bar: a robot glyph that carries a
// count badge and opens the ARO notification shade. The shade itself (the
// pull-down list, the socket to arod) lives in Panel.qml; this widget owns
// the bar mark and the open/close lifecycle the bar drives.
BarWidget {
  id: root
  moduleName: "aro.notifications"

  readonly property int count: panelLoader.item ? panelLoader.item.count : 0

  // ---- Bar-widget popout contract (see Bar.findPanelWidget): the bar tracks
  //      this widget, so open/close/opened and the popout hooks live here and
  //      forward to the nested panel.
  readonly property bool opened: panelLoader.item ? panelLoader.item.opened === true : false
  readonly property bool popoutSwitchClosing: panelLoader.item ? panelLoader.item.popoutSwitchClosing === true : false

  function open() { if (panelLoader.item) panelLoader.item.open() }
  function close() { if (panelLoader.item) panelLoader.item.close() }
  function togglePanel() { if (panelLoader.item) panelLoader.item.toggle() }
  function closeForPopoutSwitch() { if (panelLoader.item) panelLoader.item.closeForPopoutSwitch() }

  readonly property real openPanelIndicatorWidth: button.labelWidth
  readonly property real openPanelIndicatorHeight: Math.max(Style.space(10), Math.round(Style.bar.iconSlot * 0.55))

  function injectPanel() {
    var target = panelLoader.item
    if (!target) return
    if ("bar" in target) target.bar = root.bar
    if ("settings" in target) target.settings = root.settings
    if ("anchorItem" in target) target.anchorItem = button
    if ("hostWidget" in target) target.hostWidget = root
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  onBarChanged: injectPanel()
  onSettingsChanged: injectPanel()

  Loader {
    id: panelLoader
    active: true
    source: Qt.resolvedUrl("Panel.qml")
    visible: false
    onLoaded: {
      root.injectPanel()
      Qt.callLater(root.injectPanel)
    }
  }

  IpcHandler {
    target: "aro.notifications"
    function open(): void { root.open() }
    function close(): void { root.close() }
    function show(): void { root.open() }
    function hide(): void { root.close() }
    function toggle(): void { root.togglePanel() }
  }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    // Nerd Font android robot (fa-android, U+F17B) — the Android mark.
    text: String.fromCharCode(0xf17b)
    labelVisible: true
    hasVisualContent: true
    horizontalMargin: 8.75
    verticalPadding: 8.75
    // Highlighted (urgent color) whenever notifications are waiting — the
    // Omarchy idiom for an attention state, no count badge.
    active: root.count > 0

    onPressed: function(b) { root.togglePanel() }
  }
}
