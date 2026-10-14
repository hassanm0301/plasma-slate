// SPDX-License-Identifier: GPL-3.0-or-later
#include "app.h"

#include <QApplication>
#include <QCommandLineParser>
#include <QDBusConnection>
#include <QDBusConnectionInterface>
#include <QDBusMessage>
#include <QIcon>

namespace
{
const QString kAppService = QStringLiteral("io.github.hassanm0301.PlasmaSlate.Tray");
const QString kAppPath = QStringLiteral("/App");
const QString kAppIface = QStringLiteral("io.github.hassanm0301.PlasmaSlate.App");
} // namespace

int main(int argc, char **argv)
{
    QApplication app(argc, argv);
    QApplication::setApplicationName(QStringLiteral("plasma-slate-tray"));
    QApplication::setApplicationDisplayName(QStringLiteral("Plasma Slate"));
    QApplication::setDesktopFileName(QStringLiteral("io.github.hassanm0301.plasmaslate.tray"));
    QApplication::setWindowIcon(QIcon::fromTheme(QStringLiteral("video-display")));
    QApplication::setQuitOnLastWindowClosed(false);

    QCommandLineParser parser;
    parser.setApplicationDescription(QStringLiteral("Plasma Slate: tablets as extra screens"));
    parser.addHelpOption();
    const QCommandLineOption background(QStringLiteral("background"), QStringLiteral("Start in the system tray without opening the window."));
    parser.addOption(background);
    parser.process(app);

    // Launcher activation token (Wayland), handed to whichever instance shows the window.
    const QString token = qEnvironmentVariable("XDG_ACTIVATION_TOKEN");
    qunsetenv("XDG_ACTIVATION_TOKEN");

    // Single instance: a second start shows the running instance's window (or, with
    // --background, e.g. from the service, just exits).
    auto bus = QDBusConnection::sessionBus();
    if (bus.interface()
        && bus.interface()->registerService(kAppService, QDBusConnectionInterface::DontQueueService, QDBusConnectionInterface::DontAllowReplacement).value()
            != QDBusConnectionInterface::ServiceRegistered) {
        if (!parser.isSet(background)) {
            auto msg = QDBusMessage::createMethodCall(kAppService, kAppPath, kAppIface, QStringLiteral("Show"));
            msg.setArguments({token});
            bus.call(msg, QDBus::Block, 3000);
        }
        return 0;
    }

    App slate;
    bus.registerObject(kAppPath, &slate, QDBusConnection::ExportScriptableSlots);
    if (!parser.isSet(background))
        slate.Show(token);
    return app.exec();
}
