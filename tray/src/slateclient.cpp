// SPDX-License-Identifier: GPL-3.0-or-later
#include "slateclient.h"

#include <QDBusConnection>
#include <QDBusConnectionInterface>
#include <QDBusInterface>
#include <QDBusMessage>
#include <QDesktopServices>
#include <QDir>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QProcess>
#include <QSet>
#include <QStandardPaths>
#include <QUrl>

namespace
{
const QString kService = QStringLiteral("io.github.hassanm0301.PlasmaSlate");
const QString kPath = QStringLiteral("/io/github/hassanm0301/PlasmaSlate");
const QString kIface = QStringLiteral("io.github.hassanm0301.PlasmaSlate1");

QVariantList parseArray(const QString &json)
{
    return QJsonDocument::fromJson(json.toUtf8()).array().toVariantList();
}

/// A session without its live statistics, to tell real changes from counter updates.
QVariantMap withoutStats(QVariantMap s)
{
    for (const char *k : {"frames_sent", "bytes_sent", "frames_dropped", "rtt_us", "latency_us", "bitrate_kbps", "encoder"})
        s.remove(QString::fromLatin1(k));
    return s;
}

void systemctl(const QString &verb)
{
    QProcess::startDetached(QStringLiteral("systemctl"), {QStringLiteral("--user"), verb, QStringLiteral("slated.service")});
}
} // namespace

SlateClient::SlateClient(QObject *parent)
    : QObject(parent)
{
    QDBusConnection::sessionBus().connect(kService, kPath, kIface, QStringLiteral("SessionsChanged"), this, SLOT(refresh()));
    connect(QDBusConnection::sessionBus().interface(), &QDBusConnectionInterface::serviceOwnerChanged, this,
            [this](const QString &name, const QString &, const QString &) {
                if (name == kService)
                    refresh();
            });
    connect(&m_timer, &QTimer::timeout, this, &SlateClient::refresh);
    setFastPolling(false);
    refresh();
}

void SlateClient::setFastPolling(bool fast)
{
    m_timer.start(fast ? 1500 : 3000);
    if (fast)
        refresh();
}

QList<QVariant> SlateClient::call(const QString &method, const QVariantList &args, bool *ok)
{
    QDBusMessage msg = QDBusMessage::createMethodCall(kService, kPath, kIface, method);
    msg.setArguments(args);
    const QDBusMessage reply = QDBusConnection::sessionBus().call(msg, QDBus::Block, 2000);
    const bool good = reply.type() == QDBusMessage::ReplyMessage;
    if (ok)
        *ok = good;
    return good ? reply.arguments() : QList<QVariant>{};
}

QVariant SlateClient::daemonProperty(const QString &name)
{
    QDBusInterface iface(kService, kPath, kIface, QDBusConnection::sessionBus());
    return iface.property(name.toUtf8().constData());
}

void SlateClient::refresh()
{
    bool ok = false;
    const auto reply = call(QStringLiteral("SessionsJson"), {}, &ok);
    const bool wasRunning = m_running;
    m_running = ok;
    QVariantList sessions;
    QVariantList devices;
    m_stats.clear();
    if (ok) {
        const QVariantList all = parseArray(reply.value(0).toString());
        QSet<QString> connected;
        for (const auto &v : all) {
            const auto m = v.toMap();
            m_stats.insert(m.value(QStringLiteral("session_id")).toString(), m);
            connected.insert(m.value(QStringLiteral("device_id")).toString());
            sessions.append(withoutStats(m));
        }
        if (m_serverName.isEmpty()) {
            m_serverName = daemonProperty(QStringLiteral("ServerName")).toString();
            m_fingerprint = daemonProperty(QStringLiteral("Fingerprint")).toString();
            m_port = daemonProperty(QStringLiteral("Port")).toInt();
            m_version = daemonProperty(QStringLiteral("Version")).toString();
            Q_EMIT statusChanged();
        }
        devices = parseArray(call(QStringLiteral("PairedDevicesJson")).value(0).toString());
        for (auto &d : devices) {
            auto m = d.toMap();
            const QString id = m.value(QStringLiteral("device_id")).toString();
            const auto settings = call(QStringLiteral("GetStreamSettings"), {id});
            m.insert(QStringLiteral("connected"), connected.contains(id));
            m.insert(QStringLiteral("resolution"), settings.value(0).toString());
            m.insert(QStringLiteral("fps"), settings.value(1).toUInt());
            m.remove(QStringLiteral("last_seen")); // changes on every reconnect; not shown
            d = m;
        }
    } else {
        m_serverName.clear();
    }
    if (wasRunning != m_running)
        Q_EMIT statusChanged();
    if (sessions != m_sessions) {
        m_sessions = sessions;
        Q_EMIT sessionsChanged();
    }
    if (devices != m_devices) {
        m_devices = devices;
        Q_EMIT devicesChanged();
    }
    Q_EMIT statsChanged();
}

void SlateClient::disconnectSession(const QString &sessionId)
{
    call(QStringLiteral("Disconnect"), {sessionId});
    refresh();
}

void SlateClient::forget(const QString &deviceId)
{
    call(QStringLiteral("Unpair"), {deviceId});
    refresh();
}

QString SlateClient::setStreamSettings(const QString &deviceId, const QString &resolution, uint fps)
{
    bool ok = false;
    const auto reply = call(QStringLiteral("SetStreamSettings"), {deviceId, resolution, fps}, &ok);
    refresh();
    if (!ok)
        return QStringLiteral("The Plasma Slate service is not running.");
    return reply.value(0).toString();
}

void SlateClient::startService()
{
    systemctl(QStringLiteral("start"));
}

void SlateClient::restartService()
{
    m_serverName.clear();
    systemctl(QStringLiteral("restart"));
}

void SlateClient::openDisplaySettings()
{
    QProcess::startDetached(QStringLiteral("systemsettings"), {QStringLiteral("kcm_kscreen")});
}

void SlateClient::editConfiguration()
{
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
}
