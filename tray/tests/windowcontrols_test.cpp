// SPDX-License-Identifier: GPL-3.0-or-later
#include "windowcontrols.h"

#include <KConfig>
#include <KConfigGroup>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QtTest>

namespace
{
const QString pluginId = QStringLiteral("io.github.hassanm0301.plasmaslate");
const QString groupName = QStringLiteral("org.kde.kdecoration2");

struct Fixture {
    QTemporaryDir directory;
    QString settings = directory.filePath(QStringLiteral("window-controlsrc"));
    QString kwin = directory.filePath(QStringLiteral("kwinrc"));
    bool available = true;
    QString enableError;
    QList<bool> requests;

    std::unique_ptr<WindowControls> controls()
    {
        return std::make_unique<WindowControls>(settings, kwin, [this]() { return available; },
                                               [this](bool expectSlate, WindowControls::Completion completion) {
                                                   requests.append(expectSlate);
                                                   completion(expectSlate ? enableError : QString());
                                               });
    }

    void setDecoration(const QString &library, const QString &theme)
    {
        KConfig config(kwin, KConfig::SimpleConfig);
        KConfigGroup group(&config, groupName);
        group.writeEntry("library", library);
        group.writeEntry("theme", theme);
        group.writeEntry("BorderSize", QStringLiteral("Tiny"));
        QVERIFY(config.sync());
    }

    QMap<QString, QString> decoration() const
    {
        const KConfig config(kwin, KConfig::SimpleConfig);
        return KConfigGroup(&config, groupName).entryMap();
    }
};
} // namespace

class WindowControlsTest : public QObject
{
    Q_OBJECT

private Q_SLOTS:
    void defaultsDoNotWriteConfiguration()
    {
        Fixture f;
        const auto controls = f.controls();
        QVERIFY(!controls->touchControls());
        QVERIFY(!controls->moveButton());
        QVERIFY(!QFile::exists(f.settings));
        QVERIFY(!QFile::exists(f.kwin));
        QVERIFY(f.requests.isEmpty());
    }

    void independentOptionsPersistAndRestoreOriginalSelection()
    {
        Fixture f;
        f.setDecoration(QStringLiteral("org.kde.breeze"), QStringLiteral("Original theme"));
        const auto original = f.decoration();
        auto controls = f.controls();
        controls->apply(true, false);
        QVERIFY(controls->touchControls());
        QVERIFY(!controls->moveButton());
        QCOMPARE(f.decoration().value(QStringLiteral("library")), pluginId);
        controls->apply(true, true);
        QVERIFY(controls->moveButton());
        controls.reset();

        // A new tray process must retain the original backup through partial disable.
        controls = f.controls();
        QVERIFY(controls->touchControls());
        QVERIFY(controls->moveButton());
        controls->apply(false, true);
        QVERIFY(!controls->touchControls());
        QVERIFY(controls->moveButton());
        QCOMPARE(f.decoration().value(QStringLiteral("library")), pluginId);
        controls->apply(false, false);
        QCOMPARE(f.decoration(), original);
        QVERIFY(!f.decoration().contains(QStringLiteral("NoPlugin")));
        QVERIFY(!controls->touchControls());
        QVERIFY(!controls->moveButton());
        const KConfig config(f.settings, KConfig::SimpleConfig);
        QVERIFY(KConfigGroup(&config, QStringLiteral("PreviousDecoration")).entryMap().isEmpty());
    }

    void moveButtonCanBeEnabledAloneAndMissingKeysStayMissing()
    {
        Fixture f;
        const auto controls = f.controls();
        controls->apply(false, true);
        QVERIFY(!controls->touchControls());
        QVERIFY(controls->moveButton());
        controls->apply(false, false);
        QVERIFY(f.decoration().isEmpty());
    }

    void originallyDisabledDecorationIsRestored()
    {
        Fixture f;
        {
            KConfig config(f.kwin, KConfig::SimpleConfig);
            KConfigGroup group(&config, groupName);
            group.writeEntry("NoPlugin", true);
            QVERIFY(config.sync());
        }
        const auto original = f.decoration();
        const auto controls = f.controls();
        controls->apply(true, false);
        QCOMPARE(f.decoration().value(QStringLiteral("NoPlugin")), QStringLiteral("false"));
        controls->apply(false, false);
        QCOMPARE(f.decoration(), original);
        QVERIFY(!f.decoration().contains(QStringLiteral("library")));
        QVERIFY(!f.decoration().contains(QStringLiteral("theme")));
    }

    void immutableDecorationReportsErrorWithoutChangingConfiguration()
    {
        Fixture f;
        QFile config(f.kwin);
        QVERIFY(config.open(QIODevice::WriteOnly));
        config.write("[org.kde.kdecoration2][$i]\nlibrary=org.kde.breeze\n");
        config.close();
        const auto original = f.decoration();
        const auto controls = f.controls();
        controls->apply(true, false);
        QVERIFY(!controls->error().isEmpty());
        QVERIFY(!controls->touchControls());
        QCOMPARE(f.decoration(), original);
        QVERIFY(f.requests.isEmpty());
        QVERIFY(!QFile::exists(f.settings));
    }

    void externalThemeIsPreservedWhenDisabling()
    {
        Fixture f;
        f.setDecoration(QStringLiteral("org.kde.breeze"), QStringLiteral("Original"));
        const auto controls = f.controls();
        controls->apply(true, true);
        f.setDecoration(QStringLiteral("another.decoration"), QStringLiteral("New choice"));
        const auto external = f.decoration();
        f.available = false;
        controls->apply(false, true);
        QVERIFY(controls->error().isEmpty());
        QCOMPARE(f.decoration(), external);
        controls->apply(false, false);
        QCOMPARE(f.decoration(), external);
    }

    void unavailablePluginDoesNotChangeSettingsAndDoesNotPreventDisable()
    {
        Fixture f;
        const auto controls = f.controls();
        f.available = false;
        controls->apply(true, false);
        QVERIFY(!controls->error().isEmpty());
        QVERIFY(!controls->touchControls());
        QVERIFY(!QFile::exists(f.settings));
        QVERIFY(!QFile::exists(f.kwin));
        QVERIFY(f.requests.isEmpty());

        f.available = true;
        controls->apply(false, true);
        QVERIFY(controls->moveButton());
        f.available = false;
        controls->apply(false, false);
        QVERIFY(controls->error().isEmpty());
        QVERIFY(!controls->moveButton());
        QVERIFY(f.decoration().isEmpty());
    }

    void failedLoadRollsBackConfigurationAndReportsFailure()
    {
        Fixture f;
        f.setDecoration(QStringLiteral("org.kde.breeze"), QStringLiteral("Original"));
        const auto original = f.decoration();
        f.enableError = QStringLiteral("Plugin failed to load");
        const auto controls = f.controls();
        QSignalSpy applied(controls.get(), &WindowControls::applied);
        controls->apply(true, true);
        QCOMPARE(applied.count(), 1);
        QVERIFY(!applied.at(0).at(0).toBool());
        QCOMPARE(controls->error(), f.enableError);
        QVERIFY(!controls->touchControls());
        QVERIFY(!controls->moveButton());
        QCOMPARE(f.decoration(), original);
        QCOMPARE(f.requests, QList<bool>({true, false}));
        const KConfig settings(f.settings, KConfig::SimpleConfig);
        QVERIFY(KConfigGroup(&settings, QStringLiteral("PreviousDecoration")).entryMap().isEmpty());
    }

    void unverifiedChangesStayPendingAndConcurrentThemeChangeSurvivesRollback()
    {
        Fixture f;
        f.setDecoration(QStringLiteral("org.kde.breeze"), QStringLiteral("Original"));
        WindowControls::Completion pending;
        WindowControls controls(f.settings, f.kwin, []() { return true; },
                                [&pending](bool expectSlate, WindowControls::Completion completion) {
                                    if (expectSlate)
                                        pending = std::move(completion);
                                    else
                                        completion({});
                                });
        controls.apply(true, false);
        QVERIFY(controls.busy());
        QVERIFY(!controls.touchControls());
        f.setDecoration(QStringLiteral("external.decoration"), QStringLiteral("Changed while waiting"));
        const auto external = f.decoration();
        pending(QStringLiteral("Could not verify plugin"));
        QVERIFY(!controls.busy());
        QVERIFY(!controls.touchControls());
        QCOMPARE(f.decoration(), external);
    }
};

QTEST_GUILESS_MAIN(WindowControlsTest)
#include "windowcontrols_test.moc"
