// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QObject>

class KStatusNotifierItem;
class QMenu;
class SlateClient;

/// System-tray icon: status at a glance; a click opens the window.
class Tray : public QObject
{
    Q_OBJECT
public:
    Tray(SlateClient *client, QObject *parent = nullptr);

Q_SIGNALS:
    /// The user asked for the window; `token` is the Wayland activation token, if any.
    void showWindowRequested(const QString &token);

private:
    void update();
    void rebuildMenu();

    SlateClient *m_client;
    KStatusNotifierItem *m_item;
    QMenu *m_menu;
};
