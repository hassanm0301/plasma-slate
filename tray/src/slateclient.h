// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QObject>
#include <QTimer>
#include <QVariantList>

/// Session-bus client for slated, shared by the tray icon and the window (exposed to QML as
/// `Slate`).
class SlateClient : public QObject
{
    Q_OBJECT
    Q_PROPERTY(bool running READ running NOTIFY statusChanged)
    Q_PROPERTY(QString serverName READ serverName NOTIFY statusChanged)
    Q_PROPERTY(QString fingerprint READ fingerprint NOTIFY statusChanged)
    Q_PROPERTY(int port READ port NOTIFY statusChanged)
    Q_PROPERTY(QString version READ version NOTIFY statusChanged)
    /// Connected tablets: objects as returned by `SessionsJson`. Only changes when a tablet
    /// connects, disconnects or gets new settings, so views built from it stay put.
    Q_PROPERTY(QVariantList sessions READ sessions NOTIFY sessionsChanged)
    /// Latest `SessionsJson` object per session id, including live statistics.
    Q_PROPERTY(QVariantMap stats READ stats NOTIFY statsChanged)
    /// Paired tablets: {device_id, name, connected, resolution, fps}.
    Q_PROPERTY(QVariantList devices READ devices NOTIFY devicesChanged)

public:
    explicit SlateClient(QObject *parent = nullptr);

    bool running() const { return m_running; }
    QString serverName() const { return m_serverName; }
    QString fingerprint() const { return m_fingerprint; }
    int port() const { return m_port; }
    QString version() const { return m_version; }
    QVariantList sessions() const { return m_sessions; }
    QVariantMap stats() const { return m_stats; }
    QVariantList devices() const { return m_devices; }

    /// Polls faster while someone is looking at the numbers.
    void setFastPolling(bool fast);

    Q_INVOKABLE void disconnectSession(const QString &sessionId);
    Q_INVOKABLE void forget(const QString &deviceId);
    /// Returns an error message, or an empty string on success.
    Q_INVOKABLE QString setStreamSettings(const QString &deviceId, const QString &resolution, uint fps);
    Q_INVOKABLE void startService();
    Q_INVOKABLE void restartService();
    Q_INVOKABLE void openDisplaySettings();
    Q_INVOKABLE void editConfiguration();

public Q_SLOTS:
    void refresh();

Q_SIGNALS:
    void statusChanged();
    void sessionsChanged();
    void statsChanged();
    void devicesChanged();

private:
    QList<QVariant> call(const QString &method, const QVariantList &args = {}, bool *ok = nullptr);
    QVariant daemonProperty(const QString &name);

    QTimer m_timer;
    bool m_running = false;
    QString m_serverName;
    QString m_fingerprint;
    int m_port = 0;
    QString m_version;
    QVariantList m_sessions;
    QVariantMap m_stats;
    QVariantList m_devices;
};
