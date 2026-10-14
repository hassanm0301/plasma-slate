// SPDX-License-Identifier: GPL-3.0-or-later
#include "tray.h"

#include <QApplication>
#include <QDBusConnection>
#include <QDBusConnectionInterface>

int main(int argc, char **argv)
{
    QApplication app(argc, argv);
    QApplication::setApplicationName(QStringLiteral("plasma-slate-tray"));
    QApplication::setApplicationDisplayName(QStringLiteral("Plasma Slate"));
    QApplication::setDesktopFileName(QStringLiteral("io.github.hassanm0301.plasmaslate.tray"));
    QApplication::setQuitOnLastWindowClosed(false);
    // Single instance: a second start (autostart + manual, reinstall) exits quietly.
    auto *bus = QDBusConnection::sessionBus().interface();
    if (bus && bus->registerService(QStringLiteral("io.github.hassanm0301.PlasmaSlate.Tray"),
                                    QDBusConnectionInterface::DontQueueService,
                                    QDBusConnectionInterface::DontAllowReplacement)
                       .value()
            != QDBusConnectionInterface::ServiceRegistered) {
        return 0;
    }
    Tray tray;
    return app.exec();
}
