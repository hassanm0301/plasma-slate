// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QJsonArray>
#include <QObject>
#include <QTimer>

class KStatusNotifierItem;
class QMenu;

/// System-tray front end for slated (talks to it over the session bus).
class Tray : public QObject
{
    Q_OBJECT
public:
    explicit Tray(QObject *parent = nullptr);

private Q_SLOTS:
    void refresh();
    void onSessionsChanged();

private:
    void rebuildMenu();
    QString call(const QString &method, const QVariantList &args = {}, bool *ok = nullptr);
    void addStreamMenus(QMenu *parent, const QJsonObject &session);
    void applyStream(const QString &deviceId, const QString &resolution, uint fps);

    KStatusNotifierItem *m_item;
    QMenu *m_menu;
    QTimer m_timer;
    bool m_running = false;
    QJsonArray m_sessions;
    QJsonArray m_devices;
    QString m_serverName;
    QString m_fingerprint;
};
