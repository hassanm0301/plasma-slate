// SPDX-License-Identifier: GPL-3.0-or-later
#include "windowcontrols.h"

#include <KConfig>
#include <KConfigGroup>
#include <KPluginMetaData>

#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusPendingCallWatcher>
#include <QDBusPendingReply>
#include <QDir>
#include <QMap>
#include <QRegularExpression>
#include <QStandardPaths>
#include <QTimer>

namespace
{
const QString pluginId = QStringLiteral("io.github.hassanm0301.plasmaslate");
const QString pluginNamespace = QStringLiteral("org.kde.kdecoration3");
const QString decorationGroup = QStringLiteral("org.kde.kdecoration2");
const QStringList decorationKeys{QStringLiteral("library"), QStringLiteral("theme"), QStringLiteral("NoPlugin")};
const QStringList optionKeys{QStringLiteral("TouchControls"), QStringLiteral("MoveButton")};
using Entries = QMap<QString, QString>;

QString configPath(const QString &name)
{
    return QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation) + QLatin1Char('/') + name;
}

Entries readEntries(const KConfigGroup &group, const QStringList &keys)
{
    Entries result;
    for (const auto &key : keys) {
        if (group.hasKey(key))
            result.insert(key, group.readEntry(key, QString()));
    }
    return result;
}

bool entriesMutable(const KConfigGroup &group, const QStringList &keys)
{
    if (group.isImmutable())
        return false;
    for (const auto &key : keys) {
        if (group.isEntryImmutable(key))
            return false;
    }
    return true;
}

void writeEntries(KConfigGroup &group, const Entries &entries, const QStringList &keys)
{
    for (const auto &key : keys) {
        if (entries.contains(key))
            group.writeEntry(key, entries.value(key));
        else
            group.deleteEntry(key);
    }
}

void replaceGroup(KConfigGroup &group, const Entries &entries)
{
    const auto keys = group.keyList();
    for (const auto &key : keys)
        group.deleteEntry(key);
    for (auto it = entries.cbegin(); it != entries.cend(); ++it)
        group.writeEntry(it.key(), it.value());
}

Entries selectionFromBackup(const KConfigGroup &backup, const QString &prefix)
{
    Entries result;
    for (const auto &key : decorationKeys) {
        if (backup.readEntry(prefix + key + QStringLiteral("Present"), false))
            result.insert(key, backup.readEntry(prefix + key, QString()));
    }
    return result;
}

void saveSelection(KConfigGroup &backup, const QString &prefix, const Entries &selection)
{
    for (const auto &key : decorationKeys) {
        backup.writeEntry(prefix + key + QStringLiteral("Present"), selection.contains(key));
        if (selection.contains(key))
            backup.writeEntry(prefix + key, selection.value(key));
        else
            backup.deleteEntry(prefix + key);
    }
}

bool decorationAvailable()
{
    if (KPluginMetaData::findPluginById(pluginNamespace, pluginId).isValid())
        return true;
    // The tray may have been launched before a local install updated QT_PLUGIN_PATH.
    // Discovery here does not imply that the running KWin can load it: verify that below.
    const QString localDirectory = QDir::homePath() + QStringLiteral("/.local/lib/qt6/plugins/") + pluginNamespace;
    return KPluginMetaData::findPluginById(localDirectory, pluginId).isValid();
}

bool noKWin(const QString &errorName)
{
    return errorName == QStringLiteral("org.freedesktop.DBus.Error.ServiceUnknown")
        || errorName == QStringLiteral("org.freedesktop.DBus.Error.NameHasNoOwner");
}
} // namespace

WindowControls::WindowControls(QObject *parent)
    : WindowControls(configPath(QStringLiteral("plasma-slate/window-controlsrc")), configPath(QStringLiteral("kwinrc")),
                     decorationAvailable, {}, parent)
{
    m_reconfigure = [this](bool expectSlateDecoration, Completion completion) {
        reconfigureKWin(expectSlateDecoration, std::move(completion));
    };
}

WindowControls::WindowControls(const QString &settingsPath, const QString &kwinPath, PluginResolver pluginAvailable,
                               Reconfigure reconfigure, QObject *parent)
    : QObject(parent)
    , m_settingsPath(settingsPath)
    , m_kwinPath(kwinPath)
    , m_pluginAvailable(std::move(pluginAvailable))
    , m_reconfigure(std::move(reconfigure))
{
    refresh();
}

void WindowControls::refresh()
{
    if (m_busy)
        return;
    const KConfig settings(m_settingsPath, KConfig::SimpleConfig);
    const KConfigGroup general(&settings, QStringLiteral("General"));
    m_touchControls = general.readEntry("TouchControls", false);
    m_moveButton = general.readEntry("MoveButton", false);
    Q_EMIT settingsChanged();
}

void WindowControls::finish(const QString &error)
{
    m_busy = false;
    m_error = error;
    refresh();
    Q_EMIT applied(error.isEmpty(), error);
}

void WindowControls::apply(bool touchControls, bool moveButton)
{
    if (m_busy)
        return;
    m_error.clear();
    const bool enabling = touchControls || moveButton;
    KConfig settings(m_settingsPath, KConfig::SimpleConfig);
    KConfig kwin(m_kwinPath, KConfig::SimpleConfig);
    KConfigGroup general(&settings, QStringLiteral("General"));
    KConfigGroup backup(&settings, QStringLiteral("PreviousDecoration"));
    KConfigGroup decoration(&kwin, decorationGroup);
    const Entries oldOptions = readEntries(general, optionKeys);
    const Entries oldBackup = backup.entryMap();
    const Entries oldDecoration = readEntries(decoration, decorationKeys);
    Entries newDecoration = oldDecoration;
    const bool saved = backup.readEntry("Saved", false);
    const bool stillOwned = saved && oldDecoration == selectionFromBackup(backup, QStringLiteral("Applied"));
    const bool turningOn = (touchControls && !general.readEntry("TouchControls", false))
        || (moveButton && !general.readEntry("MoveButton", false));
    const bool selectSlate = enabling && (turningOn || stillOwned);
    if (selectSlate && !m_pluginAvailable()) {
        finish(QStringLiteral("The Plasma Slate window decoration is not installed. Install it before enabling window controls."));
        return;
    }

    if (selectSlate) {
        // Keep the original selection through independent toggle changes. If another
        // decoration was chosen meanwhile, this explicit enable starts a new ownership period.
        if (!saved || !stillOwned) {
            saveSelection(backup, QStringLiteral("Previous"), oldDecoration);
            backup.writeEntry("Saved", true);
        }
        newDecoration = {{QStringLiteral("library"), pluginId}, {QStringLiteral("theme"), QString()},
                         {QStringLiteral("NoPlugin"), QStringLiteral("false")}};
        saveSelection(backup, QStringLiteral("Applied"), newDecoration);
    } else if (!enabling) {
        if (stillOwned)
            newDecoration = selectionFromBackup(backup, QStringLiteral("Previous"));
        // A user-selected replacement theme is left untouched, even if our backup exists.
        replaceGroup(backup, {});
    }
    const Entries newOptions{{QStringLiteral("TouchControls"), touchControls ? QStringLiteral("true") : QStringLiteral("false")},
                             {QStringLiteral("MoveButton"), moveButton ? QStringLiteral("true") : QStringLiteral("false")}};
    const bool changeDecoration = oldDecoration != newDecoration;
    const Entries newBackup = backup.entryMap();
    if (!settings.isConfigWritable(false) || !entriesMutable(general, optionKeys) || backup.isImmutable()
        || (changeDecoration && (!kwin.isConfigWritable(false) || !entriesMutable(decoration, decorationKeys)))) {
        settings.markAsClean();
        finish(QStringLiteral("The window control settings could not be saved. Check that the configuration files are writable."));
        return;
    }
    writeEntries(general, newOptions, optionKeys);
    writeEntries(decoration, newDecoration, decorationKeys);

    // Roll back only entries still matching this attempt, so a concurrent Settings change
    // is never overwritten while waiting for KWin's reply.
    const auto rollback = [this, oldOptions, oldBackup, oldDecoration, newOptions, newBackup, newDecoration]() {
        KConfig rollbackSettings(m_settingsPath, KConfig::SimpleConfig);
        KConfig rollbackKwin(m_kwinPath, KConfig::SimpleConfig);
        KConfigGroup rollbackGeneral(&rollbackSettings, QStringLiteral("General"));
        KConfigGroup rollbackBackup(&rollbackSettings, QStringLiteral("PreviousDecoration"));
        KConfigGroup rollbackDecoration(&rollbackKwin, decorationGroup);
        if (readEntries(rollbackGeneral, optionKeys) == newOptions)
            writeEntries(rollbackGeneral, oldOptions, optionKeys);
        if (rollbackBackup.entryMap() == newBackup)
            replaceGroup(rollbackBackup, oldBackup);
        if (readEntries(rollbackDecoration, decorationKeys) == newDecoration)
            writeEntries(rollbackDecoration, oldDecoration, decorationKeys);
        const bool settingsRestored = rollbackSettings.sync();
        const bool decorationRestored = rollbackKwin.sync();
        return settingsRestored && decorationRestored;
    };

    if (!settings.sync() || !kwin.sync()) {
        // Prevent a dirty local object from writing the rejected attempt at destruction.
        settings.markAsClean();
        kwin.markAsClean();
        const bool restored = rollback();
        finish(QStringLiteral("The window control settings could not be saved.")
               + (restored ? QString() : QStringLiteral(" Restoring the previous configuration also failed.")));
        return;
    }

    m_busy = true;
    Q_EMIT settingsChanged();
    m_reconfigure(selectSlate, [this, rollback](const QString &error) {
        if (error.isEmpty()) {
            finish({});
            return;
        }
        const bool restored = rollback();
        const QString reported = error + (restored ? QString() : QStringLiteral(" Restoring the previous configuration also failed."));
        // Re-apply the restored selection as well; never restart the compositor.
        m_reconfigure(false, [this, reported](const QString &rollbackError) {
            finish(reported + (rollbackError.isEmpty() ? QString() : QStringLiteral(" KWin could not reload the restored settings: ") + rollbackError));
        });
    });
}

void WindowControls::reconfigureKWin(bool expectSlateDecoration, Completion completion)
{
    const auto message = QDBusMessage::createMethodCall(QStringLiteral("org.kde.KWin"), QStringLiteral("/KWin"),
                                                       QStringLiteral("org.kde.KWin"), QStringLiteral("reconfigure"));
    auto *watcher = new QDBusPendingCallWatcher(QDBusConnection::sessionBus().asyncCall(message, 2000), this);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [this, expectSlateDecoration, completion](QDBusPendingCallWatcher *call) {
        const QDBusPendingReply<> reply = *call;
        call->deleteLater();
        if (reply.isError()) {
            // Offline uninstall can restore the configuration for the next session.
            if (!expectSlateDecoration && noKWin(reply.error().name()))
                completion({});
            else
                completion(QStringLiteral("KWin could not apply the window controls: ") + reply.error().message());
            return;
        }
        // reconfigure() schedules KWin's configuration reload. Wait before checking that
        // the plugin actually loaded; also let disabling settle before an uninstaller exits.
        QTimer::singleShot(250, this, [this, expectSlateDecoration, completion]() {
            if (!expectSlateDecoration) {
                completion({});
                return;
            }
            const auto info = QDBusMessage::createMethodCall(QStringLiteral("org.kde.KWin"), QStringLiteral("/KWin"),
                                                            QStringLiteral("org.kde.KWin"), QStringLiteral("supportInformation"));
            auto *infoWatcher = new QDBusPendingCallWatcher(QDBusConnection::sessionBus().asyncCall(info, 2000), this);
            connect(infoWatcher, &QDBusPendingCallWatcher::finished, this, [completion](QDBusPendingCallWatcher *call) {
                const QDBusPendingReply<QString> reply = *call;
                call->deleteLater();
                if (reply.isError()) {
                    completion(QStringLiteral("Could not verify the window decoration: ") + reply.error().message());
                    return;
                }
                const QRegularExpression loaded(QStringLiteral("^Plugin:\\s*") + QRegularExpression::escape(pluginId) + QStringLiteral("\\s*$"),
                                                QRegularExpression::MultilineOption);
                if (!loaded.match(reply.value()).hasMatch()) {
                    completion(QStringLiteral("KWin could not load the Plasma Slate window decoration. Sign out and back in after installing Plasma Slate window controls, then try again."));
                    return;
                }
                completion({});
            });
        });
    });
}
