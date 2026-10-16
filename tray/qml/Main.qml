// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.kirigamiaddons.formcard as FormCard

Kirigami.ApplicationWindow {
    id: root

    title: "Plasma Slate"
    width: Kirigami.Units.gridUnit * 32
    height: Kirigami.Units.gridUnit * 40
    minimumWidth: Kirigami.Units.gridUnit * 20
    minimumHeight: Kirigami.Units.gridUnit * 20
    pageStack.initialPage: mainPage
    pageStack.globalToolBar.style: Kirigami.ApplicationHeaderStyle.None

    function formatBytes(b) {
        return b > 1e9 ? (b / 1e9).toFixed(1) + " GB" : (b / 1e6).toFixed(0) + " MB";
    }

    function linkLabel(link) {
        return link === "usb" ? "USB cable" : link === "network" ? "Network" : "Unknown link";
    }

    function resolutionLabel(value) {
        if (!value || value === "native")
            return "native resolution";
        return value;
    }

    function rateLabel(fps) {
        return fps === 0 ? "automatic refresh rate" : fps + " Hz";
    }

    function apply(deviceId, resolution, fps) {
        const err = Slate.setStreamSettings(deviceId, resolution, fps);
        if (err.length > 0)
            root.showPassiveNotification(err, "long");
    }

    Kirigami.PromptDialog {
        id: customDialog

        property string deviceId
        property int fps

        title: "Custom resolution"
        subtitle: "Enter WIDTH×HEIGHT (for example 1280x800) or a percentage (for example 60%). "
            + "The tablet's aspect ratio is kept, and it is never higher than the tablet's own."
        standardButtons: Kirigami.Dialog.Ok | Kirigami.Dialog.Cancel
        onAccepted: {
            if (customField.text.trim().length > 0)
                root.apply(deviceId, customField.text.trim(), fps);
        }

        QQC2.TextField {
            id: customField
            placeholderText: "1280x800"
            onAccepted: customDialog.accept()
        }
    }

    Kirigami.PromptDialog {
        id: forgetDialog

        property string deviceId
        property string deviceName

        title: "Forget " + deviceName + "?"
        subtitle: "The tablet is disconnected and has to be paired again (with confirmation on this PC) before it can connect."
        standardButtons: Kirigami.Dialog.Cancel
        customFooterActions: [
            Kirigami.Action {
                text: "Forget"
                icon.name: "edit-delete"
                onTriggered: {
                    Slate.forget(forgetDialog.deviceId);
                    forgetDialog.close();
                }
            }
        ]
    }

    FormCard.FormCardPage {
        id: mainPage

        title: "Plasma Slate"

        Kirigami.InlineMessage {
            Layout.fillWidth: true
            Layout.maximumWidth: Kirigami.Units.gridUnit * 30
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: Kirigami.Units.largeSpacing
            visible: !Slate.running
            type: Kirigami.MessageType.Warning
            text: "The Plasma Slate service is not running, so tablets cannot connect."
            actions: Kirigami.Action {
                text: "Start service"
                icon.name: "media-playback-start"
                onTriggered: Slate.startService()
            }
        }

        // --- connected tablets

        FormCard.FormHeader {
            title: "Connected tablets"
            visible: Slate.running
        }

        FormCard.FormCard {
            visible: Slate.running && Slate.sessions.length === 0

            FormCard.FormTextDelegate {
                icon.name: "tablet"
                text: "No tablet connected"
                description: "Open Plasma Slate on the tablet and choose “" + Slate.serverName + "”. "
                    + "Over USB, plug the tablet in with USB debugging enabled; the app opens by itself."
                textItem.wrapMode: Text.Wrap
            }
        }

        Repeater {
            model: Slate.sessions

            delegate: ColumnLayout {
                id: tablet

                required property var modelData
                readonly property var live: Slate.stats[modelData.session_id] || modelData
                readonly property string deviceId: modelData.device_id

                Layout.fillWidth: true
                spacing: 0

                FormCard.FormCard {
                    Layout.topMargin: Kirigami.Units.largeSpacing

                    FormCard.FormTextDelegate {
                        icon.name: "tablet"
                        text: tablet.modelData.device_name
                        description: tablet.modelData.output_name
                    }

                    FormCard.FormDelegateSeparator {}

                    FormCard.FormTextDelegate {
                        text: "Screen"
                        description: tablet.modelData.width + "×" + tablet.modelData.height + " at " + tablet.modelData.fps + " Hz"
                            + " (tablet: " + tablet.modelData.native_width + "×" + tablet.modelData.native_height
                            + " at " + tablet.modelData.panel_fps + " Hz)"
                    }

                    FormCard.FormTextDelegate {
                        text: "Connection"
                        description: root.linkLabel(tablet.modelData.link) + " · " + tablet.modelData.peer
                    }

                    FormCard.FormTextDelegate {
                        text: "Video"
                        description: tablet.live.codec + " · " + tablet.live.bitrate_kbps + " kbit/s target"
                            + (tablet.live.encoder ? " · " + tablet.live.encoder : "")
                    }

                    FormCard.FormTextDelegate {
                        text: "Latency"
                        description: (tablet.live.latency_us / 1000).toFixed(0) + " ms to the tablet's screen ("
                            + (tablet.live.encode_us / 1000).toFixed(1) + " ms encode, "
                            + (tablet.live.client_decode_us / 1000).toFixed(1) + " ms decode) · "
                            + (tablet.live.rtt_us / 1000).toFixed(1) + " ms round trip · "
                            + root.formatBytes(tablet.live.bytes_sent) + " sent"
                            + (tablet.live.fillers_sent ? " · " + tablet.live.fillers_sent + " filler frames" : "")
                    }
                }

                FormCard.FormCard {
                    Layout.topMargin: Kirigami.Units.largeSpacing

                    FormCard.FormComboBoxDelegate {
                        id: resolutionBox

                        readonly property var choices: {
                            const out = (tablet.modelData.resolution_presets || []).map(p => ({
                                text: p.width + "×" + p.height + (p.value === "native" ? " (native)" : " (" + p.value + ")"),
                                value: p.value
                            }));
                            const cur = tablet.modelData.resolution_setting || "native";
                            if (!out.some(c => c.value === cur))
                                out.push({ text: "Custom: " + cur, value: cur });
                            out.push({ text: "Custom…", value: "" });
                            return out;
                        }

                        text: "Resolution"
                        description: "Lower is lighter on this PC, the tablet and the network."
                        model: choices
                        textRole: "text"
                        valueRole: "value"
                        currentIndex: Math.max(0, choices.findIndex(c => c.value === (tablet.modelData.resolution_setting || "native")))
                        onActivated: index => {
                            const v = choices[index].value;
                            if (v === "") {
                                customDialog.deviceId = tablet.deviceId;
                                customDialog.fps = tablet.modelData.fps_setting;
                                customField.text = tablet.modelData.resolution_setting;
                                customDialog.open();
                                customField.forceActiveFocus();
                            } else if (v !== (tablet.modelData.resolution_setting || "native")) {
                                root.apply(tablet.deviceId, v, tablet.modelData.fps_setting);
                            }
                        }
                    }

                    FormCard.FormDelegateSeparator {}

                    FormCard.FormComboBoxDelegate {
                        readonly property var choices: {
                            const out = [{ text: "Automatic (" + tablet.modelData.panel_fps + " Hz)", value: 0 }];
                            for (const r of (tablet.modelData.refresh_presets || []))
                                if (r !== tablet.modelData.panel_fps)
                                    out.push({ text: r + " Hz", value: r });
                            const cur = tablet.modelData.fps_setting;
                            if (!out.some(c => c.value === cur))
                                out.push({ text: cur + " Hz", value: cur });
                            return out;
                        }

                        text: "Refresh rate"
                        model: choices
                        textRole: "text"
                        valueRole: "value"
                        currentIndex: Math.max(0, choices.findIndex(c => c.value === tablet.modelData.fps_setting))
                        onActivated: index => {
                            const v = choices[index].value;
                            if (v !== tablet.modelData.fps_setting)
                                root.apply(tablet.deviceId, tablet.modelData.resolution_setting || "native", v);
                        }
                    }

                    FormCard.FormDelegateSeparator {}

                    FormCard.FormButtonDelegate {
                        icon.name: "preferences-desktop-display"
                        text: "Display & Monitor settings…"
                        description: "Arrange the tablet screen and set its scale. Resolution and refresh rate can be changed there too. "
                            + "Each change reconnects the tablet for a moment."
                        onClicked: Slate.openDisplaySettings()
                    }
                }

                FormCard.FormCard {
                    Layout.topMargin: Kirigami.Units.largeSpacing

                    FormCard.FormButtonDelegate {
                        icon.name: "network-disconnect"
                        text: "Disconnect"
                        onClicked: Slate.disconnectSession(tablet.modelData.session_id)
                    }

                    FormCard.FormDelegateSeparator {}

                    FormCard.FormButtonDelegate {
                        icon.name: "edit-delete"
                        text: "Disconnect and forget…"
                        onClicked: {
                            forgetDialog.deviceId = tablet.deviceId;
                            forgetDialog.deviceName = tablet.modelData.device_name;
                            forgetDialog.open();
                        }
                    }
                }
            }
        }

        // --- paired tablets

        FormCard.FormHeader {
            title: "Paired tablets"
            visible: Slate.running
        }

        FormCard.FormCard {
            visible: Slate.running

            FormCard.FormTextDelegate {
                visible: Slate.devices.length === 0
                text: "No paired tablets yet"
                description: "The first time a tablet connects, this PC asks you to confirm it."
                textItem.wrapMode: Text.Wrap
            }

            Repeater {
                model: Slate.devices

                delegate: FormCard.FormTextDelegate {
                    id: device

                    required property var modelData

                    icon.name: modelData.connected ? "tablet" : "tablet-symbolic"
                    text: modelData.name
                    description: (modelData.connected ? "Connected" : "Not connected") + " · "
                        + root.resolutionLabel(modelData.resolution) + ", " + root.rateLabel(modelData.fps)
                    trailing: QQC2.ToolButton {
                        icon.name: "edit-delete"
                        text: "Forget"
                        display: QQC2.AbstractButton.IconOnly
                        QQC2.ToolTip.text: text
                        QQC2.ToolTip.visible: hovered
                        onClicked: {
                            forgetDialog.deviceId = device.modelData.device_id;
                            forgetDialog.deviceName = device.modelData.name;
                            forgetDialog.open();
                        }
                    }
                }
            }
        }

        FormCard.FormHeader {
            title: "Tablet window controls"
        }

        FormCard.FormCard {
            FormCard.FormTextDelegate {
                text: "Window decoration"
                description: "Uses the Plasma Slate window decoration. Applies to system titlebars on tablet displays."
                textItem.wrapMode: Text.Wrap
            }

            FormCard.FormDelegateSeparator {}

            FormCard.FormSwitchDelegate {
                text: "Touch-sized titlebar buttons"
                description: "Enlarge minimize, maximize and close buttons on tablet displays."
                checked: WindowControls.touchControls
                enabled: !WindowControls.busy
                onToggled: {
                    WindowControls.apply(checked, WindowControls.moveButton);
                    checked = Qt.binding(() => WindowControls.touchControls);
                }
            }

            FormCard.FormDelegateSeparator {}

            FormCard.FormSwitchDelegate {
                text: "Move to desktop monitor button"
                description: "Add a titlebar button that sends the window back to a desktop monitor."
                checked: WindowControls.moveButton
                enabled: !WindowControls.busy
                onToggled: {
                    WindowControls.apply(WindowControls.touchControls, checked);
                    checked = Qt.binding(() => WindowControls.moveButton);
                }
            }

            FormCard.FormTextDelegate {
                visible: WindowControls.busy
                text: "Applying window controls…"
            }
        }

        Kirigami.InlineMessage {
            Layout.fillWidth: true
            Layout.maximumWidth: Kirigami.Units.gridUnit * 30
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: Kirigami.Units.largeSpacing
            visible: WindowControls.error.length > 0
            type: Kirigami.MessageType.Error
            text: WindowControls.error
        }

        // --- this PC

        FormCard.FormHeader {
            title: "This PC"
            visible: Slate.running
        }

        FormCard.FormCard {
            visible: Slate.running

            FormCard.FormTextDelegate {
                text: "Name"
                description: Slate.serverName
            }

            FormCard.FormTextDelegate {
                text: "Fingerprint"
                description: Slate.fingerprint + " — check that the tablet shows the same when pairing"
            }

            FormCard.FormTextDelegate {
                text: "Port"
                description: Slate.port + " (TCP)"
            }

            FormCard.FormTextDelegate {
                text: "Version"
                description: Slate.version
            }
        }

        FormCard.FormCard {
            Layout.topMargin: Kirigami.Units.largeSpacing
            Layout.bottomMargin: Kirigami.Units.largeSpacing

            FormCard.FormButtonDelegate {
                icon.name: "document-edit"
                text: "Edit configuration file…"
                description: "Port, codec, bitrate, USB and network discovery. Restart the service afterwards."
                onClicked: Slate.editConfiguration()
            }

            FormCard.FormDelegateSeparator {}

            FormCard.FormButtonDelegate {
                visible: Slate.running
                icon.name: "view-refresh"
                text: "Restart service"
                description: "Connected tablets are disconnected."
                onClicked: Slate.restartService()
            }
        }
    }
}
