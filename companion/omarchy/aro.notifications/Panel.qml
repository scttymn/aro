import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

// The ARO notification shade: a pull-down list of the Android apps'
// notifications, fed by arod over a Unix socket (see crates/arod/src/shade.rs).
// Tapping a card re-opens the app through the same dispatch a toast tap uses;
// the × dismisses one, and Clear all empties the shade.
//
// BarWidget.qml owns the bar glyph and hands this panel the button to anchor
// against.
Panel {
  id: root
  moduleName: "aro.notifications"
  ipcTarget: "aro.notifications"
  manageIpc: false

  property var anchorItem: null
  property var hostWidget: null
  readonly property var barIdentity: hostWidget || root

  readonly property int count: notes.count
  readonly property color fg: bar ? bar.foreground : Color.foreground

  readonly property string socketPath: (Quickshell.env("XDG_RUNTIME_DIR") || "/run/user/1000") + "/aro/notifications.sock"

  function open() { root.controller.show() }
  function close() { root.controller.hide() }
  function toggle() { root.opened ? root.close() : root.open() }

  // ---- Notification model, kept in posted order (newest last from arod;
  //      we insert newest first for the shade).
  ListModel { id: notes }

  function indexOfId(id) {
    for (var i = 0; i < notes.count; i++) if (notes.get(i).nid === id) return i
    return -1
  }

  function upsert(n) {
    var row = { nid: n.id, app: n.app || "", appName: n.appName || n.app || "", title: n.title || "", body: n.body || "", ongoing: n.ongoing === true, ts: n.ts || 0 }
    var i = indexOfId(n.id)
    if (i >= 0) notes.set(i, row)
    else notes.insert(0, row) // newest on top
  }

  function removeId(id) {
    var i = indexOfId(id)
    if (i >= 0) notes.remove(i)
  }

  function onLine(line) {
    var msg
    try { msg = JSON.parse(line) } catch (e) { return }
    if (msg.type === "snapshot") {
      notes.clear()
      var list = msg.notifications || []
      for (var i = 0; i < list.length; i++) upsert(list[i])
    } else if (msg.type === "posted") {
      if (msg.notification) upsert(msg.notification)
    } else if (msg.type === "removed") {
      removeId(msg.id)
    }
  }

  function send(obj) {
    if (sock.connected) sock.write(JSON.stringify(obj) + "\n")
  }

  function invoke(id) { send({ cmd: "invoke", id: id }); root.close() }
  function dismiss(id) { send({ cmd: "dismiss", id: id }) }
  function dismissAll() { send({ cmd: "dismissAll" }) }

  Socket {
    id: sock
    path: root.socketPath
    connected: true
    parser: SplitParser { onRead: function(line) { root.onLine(line) } }
    onConnectedChanged: {
      // arod not running (yet) or the app exited: clear the list and keep
      // trying, so the shade fills in as soon as an app comes up.
      if (!connected) { notes.clear(); reconnect.restart() }
    }
    onError: reconnect.restart()
  }

  Timer {
    id: reconnect
    interval: 1500
    repeat: false
    onTriggered: if (!sock.connected) sock.connected = true
  }

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem
    owner: root.barIdentity
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(380))
    contentHeight: panel.fittedContentHeight(column.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: root.close()

      Column {
        id: column
        width: parent.width
        spacing: Style.space(10)

        // ---- Header: title (left) + Clear all (right).
        Item {
          width: parent.width
          height: title.implicitHeight

          Text {
            id: title
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
            text: String.fromCharCode(0xf17b) + "  Notifications"
            color: root.fg
            font.family: Style.font.family
            font.pixelSize: Style.font.title
            font.bold: true
          }

          Text {
            id: clearBtn
            visible: notes.count > 0
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            text: "Clear all"
            color: clearMouse.containsMouse ? Color.accent : Color.muted
            font.family: Style.font.family
            font.pixelSize: Style.font.bodySmall

            MouseArea {
              id: clearMouse
              anchors.fill: parent
              hoverEnabled: true
              cursorShape: Qt.PointingHandCursor
              onClicked: root.dismissAll()
            }
          }
        }

        // ---- Empty state.
        Text {
          visible: notes.count === 0
          width: parent.width
          text: "No notifications"
          color: Color.muted
          font.family: Style.font.family
          font.pixelSize: Style.font.body
          horizontalAlignment: Text.AlignHCenter
          topPadding: Style.space(24)
          bottomPadding: Style.space(24)
        }

        // ---- The list. Ongoing notifications float to the top by insertion
        //      order; capped height with scroll for long stacks.
        Flickable {
          width: parent.width
          height: Math.min(contentHeight, Style.space(520))
          contentWidth: width
          contentHeight: cards.implicitHeight
          clip: true
          boundsBehavior: Flickable.StopAtBounds
          interactive: contentHeight > height

          Column {
            id: cards
            width: parent.width
            spacing: Style.space(8)

            Repeater {
              model: notes

              Rectangle {
                required property int index
                required property var model
                width: cards.width
                implicitHeight: cardCol.implicitHeight + Style.space(20)
                radius: Math.max(6, Style.space(10))
                color: Color.notifications.background
                border.width: model.ongoing ? Math.max(1, Style.space(1)) : 0
                border.color: Color.notifications.border

                MouseArea {
                  anchors.fill: parent
                  cursorShape: Qt.PointingHandCursor
                  acceptedButtons: Qt.LeftButton
                  onClicked: root.invoke(model.nid)
                }

                Column {
                  id: cardCol
                  anchors.left: parent.left
                  anchors.right: parent.right
                  anchors.top: parent.top
                  anchors.leftMargin: Style.space(14)
                  anchors.rightMargin: Style.space(14)
                  anchors.topMargin: Style.space(10)
                  spacing: Style.space(2)

                  Row {
                    width: parent.width
                    spacing: Style.space(6)

                    Text {
                      text: model.appName
                      color: Color.muted
                      font.family: Style.font.family
                      font.pixelSize: Style.font.caption
                      font.capitalization: Font.AllUppercase
                      elide: Text.ElideRight
                      width: parent.width - ongoingTag.width - dismissX.width - Style.space(12)
                    }

                    Text {
                      id: ongoingTag
                      visible: model.ongoing
                      text: "ongoing"
                      color: Color.notifications.countdown
                      font.family: Style.font.family
                      font.pixelSize: Style.font.caption
                    }

                    // × dismiss (ongoing notifications can't be swiped away).
                    Text {
                      id: dismissX
                      visible: !model.ongoing
                      text: "✕"
                      color: xMouse.containsMouse ? Color.accent : Color.muted
                      font.family: Style.font.family
                      font.pixelSize: Style.font.subtitle

                      MouseArea {
                        id: xMouse
                        anchors.fill: parent
                        anchors.margins: -Style.space(6)
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.dismiss(model.nid)
                      }
                    }
                  }

                  Text {
                    width: parent.width
                    text: model.title
                    visible: text !== ""
                    color: root.fg
                    font.family: Style.font.family
                    font.pixelSize: Style.font.subtitle
                    font.bold: true
                    elide: Text.ElideRight
                  }

                  Text {
                    width: parent.width
                    text: model.body
                    visible: text !== ""
                    color: Color.notifications.text
                    font.family: Style.font.family
                    font.pixelSize: Style.font.body
                    wrapMode: Text.WordWrap
                    maximumLineCount: 4
                    elide: Text.ElideRight
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
