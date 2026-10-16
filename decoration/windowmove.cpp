// SPDX-License-Identifier: GPL-3.0-or-later
#include "windowmove.h"

#include <QCoreApplication>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusPendingCallWatcher>
#include <QDBusPendingReply>
#include <QDebug>
#include <QDir>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QPointer>
#include <QTemporaryFile>
#include <QUuid>

namespace
{
const QString service = QStringLiteral("org.kde.KWin");
const QString scriptingPath = QStringLiteral("/Scripting");
const QString scriptingInterface = QStringLiteral("org.kde.kwin.Scripting");
constexpr auto pendingProperty = "plasmaSlateMovePending";

QDBusMessage scriptingCall(const QString &method, const QVariantList &arguments)
{
    auto message = QDBusMessage::createMethodCall(service, scriptingPath, scriptingInterface, method);
    message.setArguments(arguments);
    return message;
}

class MoveRequest final : public QObject
{
public:
    explicit MoveRequest(QObject *window)
        : QObject(QCoreApplication::instance())
        , m_window(window)
        , m_file(QDir::tempPath() + QStringLiteral("/plasma-slate-move-XXXXXX.js"))
        , m_name(QStringLiteral("plasma-slate-move-") + QUuid::createUuid().toString(QUuid::WithoutBraces))
    {
        window->setProperty(pendingProperty, true);
    }

    ~MoveRequest() override
    {
        if (m_window)
            m_window->setProperty(pendingProperty, false);
    }

    void start(const QString &id, const QString &preferredOutput)
    {
        QFile source(QStringLiteral(":/plasma-slate/move-window.js"));
        if (!source.open(QIODevice::ReadOnly) || !m_file.open()) {
            qWarning() << "Plasma Slate: could not prepare window move script";
            deleteLater();
            return;
        }
        // Values are JSON strings, never interpolated into JavaScript source as code.
        const auto args = QJsonDocument(QJsonArray{id, preferredOutput}).toJson(QJsonDocument::Compact);
        const QByteArray script = source.readAll() + "\nmoveSlateWindow.apply(null, " + args + ");\n";
        if (m_file.write(script) != script.size() || !m_file.flush()) {
            qWarning() << "Plasma Slate: could not write window move script";
            deleteLater();
            return;
        }
        m_file.close();
        auto *watcher = new QDBusPendingCallWatcher(QDBusConnection::sessionBus().asyncCall(
            scriptingCall(QStringLiteral("loadScript"), {m_file.fileName(), m_name}), 3000), this);
        connect(watcher, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher *call) {
            const QDBusPendingReply<int> reply = *call;
            call->deleteLater();
            if (reply.isError() || reply.value() < 0) {
                qWarning() << "Plasma Slate: KWin could not load window move script" << reply.error().message();
                // A timed-out load may still have registered its unique name.
                finish();
                return;
            }
            auto message = QDBusMessage::createMethodCall(service,
                QStringLiteral("/Scripting/Script%1").arg(reply.value()),
                QStringLiteral("org.kde.kwin.Script"), QStringLiteral("run"));
            auto *run = new QDBusPendingCallWatcher(QDBusConnection::sessionBus().asyncCall(message, 3000), this);
            connect(run, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher *result) {
                const QDBusPendingReply<> completed = *result;
                if (completed.isError())
                    qWarning() << "Plasma Slate: KWin could not move window" << completed.error().message();
                result->deleteLater();
                finish();
            });
        });
    }

private:
    void finish()
    {
        // run replies only after evaluation, so its file and script can now be removed.
        // No synchronous call is safe here: this code runs inside KWin itself.
        QDBusConnection::sessionBus().asyncCall(scriptingCall(QStringLiteral("unloadScript"), {m_name}), 3000);
        deleteLater();
    }

    QPointer<QObject> m_window;
    QTemporaryFile m_file;
    QString m_name;
};
}

void WindowMove::request(QObject *window, const QString &preferredOutput)
{
    if (!window || window->property(pendingProperty).toBool())
        return;
    const QUuid id = window->property("internalId").toUuid();
    if (id.isNull()) {
        qWarning() << "Plasma Slate: this KWin version does not expose a window identifier";
        return;
    }
    (new MoveRequest(window))->start(id.toString(), preferredOutput);
}
