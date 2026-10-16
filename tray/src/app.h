// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QObject>
#include <QPointer>

class QQmlApplicationEngine;
class QQuickWindow;
class SlateClient;
class Tray;
class WindowControls;

/// The running app: tray icon plus the window, created on first use. Registered on the
/// session bus so a second launch (e.g. from the app launcher) shows this instance's window.
class App : public QObject
{
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "io.github.hassanm0301.PlasmaSlate.App")
public:
    explicit App(QObject *parent = nullptr);

public Q_SLOTS:
    /// Shows and raises the window. `token` is a Wayland activation token (may be empty).
    Q_SCRIPTABLE void Show(const QString &token);

private:
    SlateClient *m_client;
    Tray *m_tray;
    WindowControls *m_windowControls;
    QQmlApplicationEngine *m_engine = nullptr;
    QPointer<QQuickWindow> m_window;
};
