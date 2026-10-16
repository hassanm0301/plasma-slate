// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QObject>
#include <functional>

/// Owns the optional decoration and restores the user's previous selection on disable.
/// Changes are committed to the UI only after KWin has accepted them.
class WindowControls : public QObject
{
    Q_OBJECT
    Q_PROPERTY(bool touchControls READ touchControls NOTIFY settingsChanged)
    Q_PROPERTY(bool moveButton READ moveButton NOTIFY settingsChanged)
    Q_PROPERTY(bool busy READ busy NOTIFY settingsChanged)
    Q_PROPERTY(QString error READ error NOTIFY settingsChanged)

public:
    using Completion = std::function<void(const QString &error)>;
    using PluginResolver = std::function<bool()>;
    using Reconfigure = std::function<void(bool expectSlateDecoration, Completion completion)>;

    explicit WindowControls(QObject *parent = nullptr);
    /// Separate paths and backend callbacks allow configuration tests without touching a session.
    WindowControls(const QString &settingsPath, const QString &kwinPath, PluginResolver pluginAvailable,
                   Reconfigure reconfigure, QObject *parent = nullptr);

    bool touchControls() const { return m_touchControls; }
    bool moveButton() const { return m_moveButton; }
    bool busy() const { return m_busy; }
    QString error() const { return m_error; }

    Q_INVOKABLE void apply(bool touchControls, bool moveButton);
    void refresh();

Q_SIGNALS:
    void settingsChanged();
    void applied(bool success, const QString &error);

private:
    void reconfigureKWin(bool expectSlateDecoration, Completion completion);
    void finish(const QString &error);

    QString m_settingsPath;
    QString m_kwinPath;
    PluginResolver m_pluginAvailable;
    Reconfigure m_reconfigure;
    bool m_touchControls = false;
    bool m_moveButton = false;
    bool m_busy = false;
    QString m_error;
};
