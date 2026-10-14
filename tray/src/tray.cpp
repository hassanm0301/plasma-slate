// SPDX-License-Identifier: GPL-3.0-or-later
#include "tray.h"
#include "slateclient.h"

#include <KStatusNotifierItem>

#include <QMenu>

Tray::Tray(SlateClient *client, QObject *parent)
    : QObject(parent)
    , m_client(client)
    , m_item(new KStatusNotifierItem(QStringLiteral("plasma-slate"), this))
    , m_menu(new QMenu)
{
    m_item->setTitle(QStringLiteral("Plasma Slate"));
    m_item->setCategory(KStatusNotifierItem::Hardware);
    m_item->setIconByName(QStringLiteral("video-display"));
    m_item->setStandardActionsEnabled(true);
    m_item->setContextMenu(m_menu);
    // No associated window: the window is created on first use, so clicks are handled here.
    m_item->setAssociatedWindow(nullptr);
    connect(m_item, &KStatusNotifierItem::activateRequested, this, [this] {
        Q_EMIT showWindowRequested(m_item->providedToken());
    });

    // Stats arrive with every poll; the menu only depends on the status and the tablet list.
    connect(m_client, &SlateClient::statsChanged, this, &Tray::update);
    connect(m_client, &SlateClient::statusChanged, this, &Tray::rebuildMenu);
    connect(m_client, &SlateClient::sessionsChanged, this, &Tray::rebuildMenu);
    connect(m_menu, &QMenu::aboutToHide, this, &Tray::rebuildMenu, Qt::QueuedConnection);
    update();
    rebuildMenu();
}

void Tray::update()
{
    const QVariantMap stats = m_client->stats();
    QString tip;
    if (!m_client->running()) {
        m_item->setStatus(KStatusNotifierItem::Passive);
        m_item->setIconByName(QStringLiteral("video-display"));
        tip = QStringLiteral("The Plasma Slate service is not running");
    } else if (stats.isEmpty()) {
        m_item->setStatus(KStatusNotifierItem::Passive);
        m_item->setIconByName(QStringLiteral("video-display"));
        tip = QStringLiteral("Waiting for a tablet (%1, fingerprint %2)").arg(m_client->serverName(), m_client->fingerprint());
    } else {
        m_item->setStatus(KStatusNotifierItem::Active);
        m_item->setIconByName(QStringLiteral("video-display-symbolic"));
        QStringList lines;
        for (const auto &v : stats) {
            const auto s = v.toMap();
            lines << QStringLiteral("%1 — %2×%3 %4, %5 ms")
                         .arg(s.value(QStringLiteral("device_name")).toString())
                         .arg(s.value(QStringLiteral("width")).toInt())
                         .arg(s.value(QStringLiteral("height")).toInt())
                         .arg(s.value(QStringLiteral("codec")).toString())
                         .arg(s.value(QStringLiteral("latency_us")).toInt() / 1000);
        }
        tip = lines.join(QLatin1Char('\n'));
    }
    m_item->setToolTip(QStringLiteral("video-display"), QStringLiteral("Plasma Slate"), tip);
}

void Tray::rebuildMenu()
{
    // Keep the menu stable while it is open; it is rebuilt when it closes.
    if (m_menu->isVisible())
        return;
    m_menu->clear();

    m_menu->addAction(QIcon::fromTheme(QStringLiteral("video-display")), QStringLiteral("Open Plasma Slate"), this, [this] {
        Q_EMIT showWindowRequested(QString());
    });
    m_menu->addSeparator();
    if (!m_client->running()) {
        m_menu->addAction(QIcon::fromTheme(QStringLiteral("media-playback-start")), QStringLiteral("Start service"), m_client, &SlateClient::startService);
    } else {
        const QVariantList sessions = m_client->sessions();
        m_menu->addSection(sessions.isEmpty() ? QStringLiteral("No tablet connected") : QStringLiteral("Connected tablets"));
        for (const auto &v : sessions) {
            const auto s = v.toMap();
            const QString id = s.value(QStringLiteral("session_id")).toString();
            m_menu->addAction(QIcon::fromTheme(QStringLiteral("network-disconnect")),
                              QStringLiteral("Disconnect %1").arg(s.value(QStringLiteral("device_name")).toString()), this, [this, id] {
                                  m_client->disconnectSession(id);
                              });
        }
    }
    m_menu->addSeparator();
    m_menu->addAction(QIcon::fromTheme(QStringLiteral("preferences-desktop-display")), QStringLiteral("Display settings…"), m_client,
                      &SlateClient::openDisplaySettings);
}
