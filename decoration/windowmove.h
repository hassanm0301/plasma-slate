// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QString>

class QObject;

namespace WindowMove
{
/// Send exactly this KWin window to its last desktop output (or another available
/// desktop output). All compositor D-Bus calls are asynchronous.
void request(QObject *window, const QString &preferredOutput);
}
