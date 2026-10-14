// SPDX-License-Identifier: GPL-3.0-or-later
#include "tray.h"

#include <QApplication>

int main(int argc, char **argv)
{
    QApplication app(argc, argv);
    QApplication::setApplicationName(QStringLiteral("plasma-slate-tray"));
    QApplication::setApplicationDisplayName(QStringLiteral("Plasma Slate"));
    QApplication::setDesktopFileName(QStringLiteral("io.github.hassanm0301.plasmaslate.tray"));
    QApplication::setQuitOnLastWindowClosed(false);
    Tray tray;
    return app.exec();
}
