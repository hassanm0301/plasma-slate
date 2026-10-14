// SPDX-License-Identifier: GPL-3.0-or-later
#include "app.h"
#include "slateclient.h"
#include "tray.h"

#include <KWindowSystem>

#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQuickWindow>

App::App(QObject *parent)
    : QObject(parent)
    , m_client(new SlateClient(this))
    , m_tray(new Tray(m_client, this))
{
    connect(m_tray, &Tray::showWindowRequested, this, &App::Show);
}

void App::Show(const QString &token)
{
    if (!m_window) {
        m_engine = new QQmlApplicationEngine(this);
        m_engine->rootContext()->setContextProperty(QStringLiteral("Slate"), m_client);
        m_engine->loadFromModule("io.github.hassanm0301.plasmaslate", "Main");
        if (m_engine->rootObjects().isEmpty())
            return;
        m_window = qobject_cast<QQuickWindow *>(m_engine->rootObjects().constFirst());
        if (!m_window)
            return;
        connect(m_window, &QWindow::visibleChanged, m_client, &SlateClient::setFastPolling);
    }
    if (!token.isEmpty())
        KWindowSystem::setCurrentXdgActivationToken(token);
    m_window->show();
    m_window->raise();
    KWindowSystem::activateWindow(m_window);
}
