// SPDX-License-Identifier: GPL-3.0-or-later
#include "tray.h"

#include <KStatusNotifierItem>

#include <QDBusConnection>
#include <QDBusConnectionInterface>
#include <QDBusInterface>
#include <QDBusReply>
#include <QDesktopServices>
#include <QDir>
#include <QJsonDocument>
#include <QJsonObject>
#include <QMenu>
#include <QProcess>
#include <QStandardPaths>
#include <QUrl>

namespace
{
const QString kService = QStringLiteral("io.github.hassanm0301.PlasmaSlate");
const QString kPath = QStringLiteral("/io/github/hassanm0301/PlasmaSlate");
const QString kIface = QStringLiteral("io.github.hassanm0301.PlasmaSlate1");

QString formatBytes(double bytes)
{
    if (bytes > 1e9)
        return QStringLiteral("%1 GB").arg(bytes / 1e9, 0, 'f', 1);
    return QStringLiteral("%1 MB").arg(bytes / 1e6, 0, 'f', 0);
}
} // namespace

Tray::Tray(QObject *parent)
    : QObject(parent)
    , m_item(new KStatusNotifierItem(QStringLiteral("plasma-slate"), this))
    , m_menu(new QMenu)
{
    m_item->setTitle(QStringLiteral("Plasma Slate"));
    m_item->setCategory(KStatusNotifierItem::Hardware);
    m_item->setIconByName(QStringLiteral("video-display"));
    m_item->setStandardActionsEnabled(true);
    m_item->setContextMenu(m_menu);
    m_item->setAssociatedWindow(nullptr);

    QDBusConnection::sessionBus().connect(kService, kPath, kIface, QStringLiteral("SessionsChanged"), this, SLOT(onSessionsChanged()));
    connect(QDBusConnection::sessionBus().interface(), &QDBusConnectionInterface::serviceOwnerChanged, this,
            [this](const QString &name, const QString &, const QString &) {
                if (name == kService)
                    refresh();
            });
    connect(&m_timer, &QTimer::timeout, this, &Tray::refresh);
    m_timer.start(3000);
    connect(m_menu, &QMenu::aboutToShow, this, &Tray::refresh);
    refresh();
}

QString Tray::call(const QString &method, const QVariantList &args, bool *ok)
{
    QDBusInterface iface(kService, kPath, kIface, QDBusConnection::sessionBus());
    QDBusMessage reply = iface.callWithArgumentList(QDBus::Block, method, args);
    const bool good = reply.type() == QDBusMessage::ReplyMessage;
    if (ok)
        *ok = good;
    if (!good || reply.arguments().isEmpty())
        return {};
    return reply.arguments().first().toString();
}

void Tray::onSessionsChanged()
{
    refresh();
}

void Tray::refresh()
{
    bool ok = false;
    const QString sessions = call(QStringLiteral("SessionsJson"), {}, &ok);
    m_running = ok;
    if (ok) {
        m_sessions = QJsonDocument::fromJson(sessions.toUtf8()).array();
        m_devices = QJsonDocument::fromJson(call(QStringLiteral("PairedDevicesJson")).toUtf8()).array();
        QDBusInterface props(kService, kPath, QStringLiteral("org.freedesktop.DBus.Properties"), QDBusConnection::sessionBus());
        QDBusReply<QVariant> name = props.call(QStringLiteral("Get"), kIface, QStringLiteral("ServerName"));
        QDBusReply<QVariant> fp = props.call(QStringLiteral("Get"), kIface, QStringLiteral("Fingerprint"));
        m_serverName = name.isValid() ? name.value().toString() : QString();
        m_fingerprint = fp.isValid() ? fp.value().toString() : QString();
    } else {
        m_sessions = {};
        m_devices = {};
    }

    QString tip;
    if (!m_running) {
        m_item->setStatus(KStatusNotifierItem::Passive);
        m_item->setIconByName(QStringLiteral("video-display"));
        tip = QStringLiteral("The slated service is not running");
    } else if (m_sessions.isEmpty()) {
        m_item->setStatus(KStatusNotifierItem::Passive);
        m_item->setIconByName(QStringLiteral("video-display"));
        tip = QStringLiteral("Waiting for a tablet (%1, fingerprint %2)").arg(m_serverName, m_fingerprint);
    } else {
        m_item->setStatus(KStatusNotifierItem::Active);
        m_item->setIconByName(QStringLiteral("video-display-symbolic"));
        QStringList lines;
        for (const auto &v : std::as_const(m_sessions)) {
            const auto s = v.toObject();
            lines << QStringLiteral("%1 — %2×%3 %4, %5 ms")
                         .arg(s[QStringLiteral("device_name")].toString())
                         .arg(s[QStringLiteral("width")].toInt())
                         .arg(s[QStringLiteral("height")].toInt())
                         .arg(s[QStringLiteral("codec")].toString())
                         .arg(s[QStringLiteral("latency_us")].toInt() / 1000);
        }
        tip = lines.join(QLatin1Char('\n'));
    }
    m_item->setToolTip(QStringLiteral("video-display"), QStringLiteral("Plasma Slate"), tip);
    rebuildMenu();
}

void Tray::rebuildMenu()
{
    // Keep the menu stable while it is open; it is refreshed again on the next aboutToShow.
    if (m_menu->isVisible())
        return;
    m_menu->clear();

    if (!m_running) {
        m_menu->addSection(QStringLiteral("Service not running"));
        m_menu->addAction(QIcon::fromTheme(QStringLiteral("media-playback-start")), QStringLiteral("Start service"), this, [] {
            QProcess::startDetached(QStringLiteral("systemctl"), {QStringLiteral("--user"), QStringLiteral("start"), QStringLiteral("slated.service")});
        });
    } else {
        m_menu->addSection(m_sessions.isEmpty() ? QStringLiteral("No tablet connected") : QStringLiteral("Connected tablets"));
        for (const auto &v : std::as_const(m_sessions)) {
            const auto s = v.toObject();
            const QString id = s[QStringLiteral("session_id")].toString();
            const QString deviceId = s[QStringLiteral("device_id")].toString();
            auto *sub = m_menu->addMenu(QIcon::fromTheme(QStringLiteral("tablet")), s[QStringLiteral("device_name")].toString());
            auto info = [sub](const QString &text) { sub->addAction(text)->setEnabled(false); };
            info(QStringLiteral("Screen: %1 (%2×%3)")
                     .arg(s[QStringLiteral("output_name")].toString())
                     .arg(s[QStringLiteral("width")].toInt())
                     .arg(s[QStringLiteral("height")].toInt()));
            info(QStringLiteral("Link: %1 via %2").arg(s[QStringLiteral("link")].toString(), s[QStringLiteral("peer")].toString()));
            info(QStringLiteral("Video: %1, %2 kbit/s target (%3)")
                     .arg(s[QStringLiteral("codec")].toString())
                     .arg(s[QStringLiteral("bitrate_kbps")].toInt())
                     .arg(s[QStringLiteral("encoder")].toString()));
            info(QStringLiteral("Latency: %1 ms · RTT %2 ms · sent %3")
                     .arg(s[QStringLiteral("latency_us")].toInt() / 1000)
                     .arg(s[QStringLiteral("rtt_us")].toInt() / 1000.0, 0, 'f', 1)
                     .arg(formatBytes(s[QStringLiteral("bytes_sent")].toDouble())));
            sub->addSeparator();
            sub->addAction(QIcon::fromTheme(QStringLiteral("network-disconnect")), QStringLiteral("Disconnect"), this, [this, id] {
                call(QStringLiteral("Disconnect"), {id});
            });
            sub->addAction(QIcon::fromTheme(QStringLiteral("edit-delete")), QStringLiteral("Disconnect and forget"), this, [this, deviceId] {
                call(QStringLiteral("Unpair"), {deviceId});
            });
        }

        auto *paired = m_menu->addMenu(QIcon::fromTheme(QStringLiteral("security-high")), QStringLiteral("Paired devices"));
        if (m_devices.isEmpty())
            paired->addAction(QStringLiteral("None"))->setEnabled(false);
        for (const auto &v : std::as_const(m_devices)) {
            const auto d = v.toObject();
            const QString deviceId = d[QStringLiteral("device_id")].toString();
            paired->addAction(QStringLiteral("Forget %1").arg(d[QStringLiteral("name")].toString()), this, [this, deviceId] {
                call(QStringLiteral("Unpair"), {deviceId});
            });
        }
        m_menu->addAction(QStringLiteral("PC fingerprint: %1").arg(m_fingerprint))->setEnabled(false);
    }

    m_menu->addSeparator();
    m_menu->addAction(QIcon::fromTheme(QStringLiteral("preferences-desktop-display")), QStringLiteral("Display settings…"), this, [] {
        QProcess::startDetached(QStringLiteral("systemsettings"), {QStringLiteral("kcm_kscreen")});
    });
    m_menu->addAction(QIcon::fromTheme(QStringLiteral("document-edit")), QStringLiteral("Edit configuration…"), this, [] {
        const QString dir = QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation) + QStringLiteral("/plasma-slate");
        const QString file = dir + QStringLiteral("/slated.toml");
        QDir().mkpath(dir);
        if (!QFile::exists(file)) {
            QFile f(file);
            if (f.open(QIODevice::WriteOnly)) {
                f.write("# Plasma Slate daemon configuration. Restart the service after editing:\n"
                        "#   systemctl --user restart slated\n"
                        "# port = 47420\n# codec = \"auto\"          # auto | h264 | hevc\n# bitrate_kbps = 20000     # fixed bitrate (default: adaptive)\n"
                        "# mdns = true\n# usb_adb = true\n# usb_autostart = true\n# max_sessions = 4\n");
            }
        }
        QDesktopServices::openUrl(QUrl::fromLocalFile(file));
    });
    if (m_running) {
        m_menu->addAction(QIcon::fromTheme(QStringLiteral("view-refresh")), QStringLiteral("Restart service"), this, [] {
            QProcess::startDetached(QStringLiteral("systemctl"), {QStringLiteral("--user"), QStringLiteral("restart"), QStringLiteral("slated.service")});
        });
    }
}
