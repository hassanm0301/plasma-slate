// SPDX-License-Identifier: GPL-3.0-or-later
// The private bridge below is only a test compositor. The shipped plugin uses
// the public decoration API plus the explicitly checked QObject window bridge.
#include <KConfig>
#include <KConfigGroup>
#include <KDecoration3/DecoratedWindow>
#include <KDecoration3/Decoration>
#include <KDecoration3/DecorationButton>
#include <KDecoration3/DecorationSettings>
#include <KPluginFactory>
#include <QGuiApplication>
#include <QHoverEvent>
#include <QMouseEvent>
#include <QPainter>
#include <QPluginLoader>
#include <QTemporaryDir>
#include <QtTest>
#include <kdecoration3/private/decoratedwindowprivate.h>
#include <kdecoration3/private/decorationbridge.h>
#include <kdecoration3/private/decorationsettingsprivate.h>

using namespace KDecoration3;

class FakeOutput : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QString name MEMBER name CONSTANT)
public:
    QString name;
};

class FakeWindow : public QObject
{
    Q_OBJECT
    Q_PROPERTY(FakeOutput *output MEMBER output NOTIFY outputChanged)
    Q_PROPERTY(bool moveableAcrossScreens MEMBER moveableAcrossScreens)
public:
    FakeOutput *output = nullptr;
    bool moveableAcrossScreens = true;
    void moveTo(FakeOutput *target) { output = target; Q_EMIT outputChanged(nullptr); }
Q_SIGNALS:
    void outputChanged(FakeOutput *previous);
};

class FakeClient : public DecoratedWindowPrivateV4
{
public:
    FakeClient(DecoratedWindow *client, Decoration *decoration)
        : DecoratedWindowPrivateV4(client, decoration), client(client) {}
    DecoratedWindow *client;
    bool maximized = false;
    bool minimizeable = true;
    qreal clientWidth = 800;
    int closes = 0, minimizes = 0, maximizes = 0, menus = 0;
    bool isActive() const override { return true; }
    QString caption() const override { return QStringLiteral("Plasma Slate — A window with touch-friendly controls"); }
    bool isOnAllDesktops() const override { return false; }
    bool isShaded() const override { return false; }
    QIcon icon() const override { return {}; }
    bool isMaximized() const override { return maximized; }
    bool isMaximizedHorizontally() const override { return maximized; }
    bool isMaximizedVertically() const override { return maximized; }
    bool isKeepAbove() const override { return false; }
    bool isKeepBelow() const override { return false; }
    bool isCloseable() const override { return true; }
    bool isMaximizeable() const override { return true; }
    bool isMinimizeable() const override { return minimizeable; }
    bool providesContextHelp() const override { return false; }
    bool isModal() const override { return false; }
    bool isShadeable() const override { return true; }
    bool isMoveable() const override { return true; }
    bool isResizeable() const override { return true; }
    qreal width() const override { return clientWidth; }
    qreal height() const override { return 200; }
    QSizeF size() const override { return {width(), height()}; }
    QPalette palette() const override { return QGuiApplication::palette(); }
    Qt::Edges adjacentScreenEdges() const override { return {}; }
    qreal scale() const override { return 1; }
    qreal nextScale() const override { return 1; }
    void requestShowToolTip(const QString &) override {}
    void requestHideToolTip() override {}
    void requestClose() override { ++closes; }
    void requestToggleMaximization(Qt::MouseButtons) override { ++maximizes; }
    void requestMinimize() override { ++minimizes; }
    void requestContextHelp() override {}
    void requestToggleOnAllDesktops() override {}
    void requestToggleShade() override {}
    void requestToggleKeepAbove() override {}
    void requestToggleKeepBelow() override {}
    void requestShowWindowMenu(const QRect &) override { ++menus; }
    QString windowClass() const override { return QStringLiteral("test"); }
    bool hasApplicationMenu() const override { return false; }
    bool isApplicationMenuActive() const override { return false; }
    void showApplicationMenu(int) override {}
    void requestShowApplicationMenu(const QRect &, int) override {}
    QString applicationMenuServiceName() const override { return {}; }
    QString applicationMenuObjectPath() const override { return {}; }
    void popup(const Positioner &, QMenu *) override {}
    bool isExcludedFromCapture() const override { return false; }
    void requestToggleExcludeFromCapture() override {}
};

class FakeSettings : public DecorationSettingsPrivateV2
{
public:
    explicit FakeSettings(DecorationSettings *settings) : DecorationSettingsPrivateV2(settings) {}
    bool isOnAllDesktopsAvailable() const override { return true; }
    bool isAlphaChannelSupported() const override { return true; }
    bool isCloseOnDoubleClickOnMenu() const override { return false; }
    bool isAlwaysShowExcludeFromCapture() const override { return false; }
    QList<DecorationButtonType> decorationButtonsLeft() const override { return {DecorationButtonType::Menu}; }
    QList<DecorationButtonType> decorationButtonsRight() const override
    { return {DecorationButtonType::Minimize, DecorationButtonType::Maximize, DecorationButtonType::Close}; }
    BorderSize borderSize() const override { return BorderSize::Tiny; }
    QFont font() const override { return QFont(QStringLiteral("sans-serif"), 10); }
};

class FakeBridge : public DecorationBridge
{
public:
    FakeClient *client = nullptr;
    std::unique_ptr<DecoratedWindowPrivate> createClient(DecoratedWindow *window, Decoration *deco) override
    {
        auto result = std::make_unique<FakeClient>(window, deco);
        client = result.get();
        return result;
    }
    std::unique_ptr<DecorationSettingsPrivate> settings(DecorationSettings *settings) override
    { return std::make_unique<FakeSettings>(settings); }
};

class RuntimeTest : public QObject
{
    Q_OBJECT
    QPluginLoader loader{QStringLiteral(DECORATION_PLUGIN_PATH)};
    KPluginFactory *factory = nullptr;
    FakeOutput desktop, slate;
    FakeWindow parent;
    FakeBridge bridge;
    std::shared_ptr<DecorationSettings> settings;
    std::unique_ptr<Decoration> deco;
    QMetaObject::Connection applyState;

    DecorationButton *button(DecorationButtonType type)
    {
        for (auto *item : deco->findChildren<DecorationButton *>()) if (item->type() == type) return item;
        return nullptr;
    }
    void flags(bool touch, bool move)
    {
        KConfig config(QStringLiteral("plasma-slate/window-controlsrc"), KConfig::SimpleConfig);
        KConfigGroup group(&config, QStringLiteral("General"));
        group.writeEntry("TouchControls", touch);
        group.writeEntry("MoveButton", move);
        group.sync();
        if (settings) Q_EMIT settings->reconfigured();
    }
    void click(DecorationButtonType type)
    {
        auto *target = button(type);
        QVERIFY(target);
        const QPointF position = target->geometry().center();
        QHoverEvent hover(QEvent::HoverMove, position, position, QPointF(-1, -1));
        QCoreApplication::sendEvent(deco.get(), &hover);
        QMouseEvent press(QEvent::MouseButtonPress, position, position, Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
        QMouseEvent release(QEvent::MouseButtonRelease, position, position, Qt::LeftButton, Qt::NoButton, Qt::NoModifier);
        QCoreApplication::sendEvent(deco.get(), &press);
        QCoreApplication::sendEvent(deco.get(), &release);
    }
private Q_SLOTS:
    void initTestCase()
    {
        factory = qobject_cast<KPluginFactory *>(loader.instance());
        QVERIFY2(factory, qPrintable(loader.errorString()));
        desktop.name = QStringLiteral("DP-1");
        slate.name = QStringLiteral("Virtual-PlasmaSlate-test");
    }
    void init()
    {
        flags(false, false);
        parent.output = &desktop;
        parent.moveableAcrossScreens = true;
        const QVariantMap args{{QStringLiteral("bridge"), QVariant::fromValue<DecorationBridge *>(&bridge)}};
        deco.reset(factory->create<Decoration>(&parent, QVariantList{args}));
        QVERIFY(deco);
        settings = std::make_shared<DecorationSettings>(&bridge);
        deco->setSettings(settings);
        deco->create();
        applyState = connect(deco.get(), &Decoration::nextStateChanged, deco.get(), [this](const auto &state) { deco->apply(state->clone()); });
        QVERIFY(deco->init());
    }
    void cleanup()
    {
        deco.reset();
        settings.reset();
    }
    void offByDefault()
    {
        parent.moveTo(&slate);
        QVERIFY(deco->borderTop() < 48);
        QVERIFY(!button(DecorationButtonType::Custom)->isVisible());
    }
    void independentTogglesAndOutputTransitions()
    {
        flags(true, false);
        const qreal compact = deco->borderTop();
        parent.moveTo(&slate);
        QVERIFY(deco->borderTop() >= 48);
        QCOMPARE(button(DecorationButtonType::Close)->geometry().width(), 48);
        QVERIFY(!button(DecorationButtonType::Custom)->isVisible());
        flags(false, true);
        QCOMPARE(deco->borderTop(), compact);
        QVERIFY(button(DecorationButtonType::Custom)->isVisible());
        flags(true, true);
        QVERIFY(deco->borderTop() >= 48);
        parent.moveTo(&desktop);
        QCOMPARE(deco->borderTop(), compact);
        QVERIFY(!button(DecorationButtonType::Custom)->isVisible());
    }
    void standardActionsUseFramework()
    {
        flags(true, true);
        parent.moveTo(&slate);
        click(DecorationButtonType::Minimize);
        click(DecorationButtonType::Maximize);
        click(DecorationButtonType::Close);
        click(DecorationButtonType::Menu);
        QTRY_COMPARE(bridge.client->minimizes, 1);
        QTRY_COMPARE(bridge.client->maximizes, 1);
        QTRY_COMPARE(bridge.client->closes, 1);
        QTRY_COMPARE(bridge.client->menus, 1);
        bridge.client->minimizeable = false;
        Q_EMIT bridge.client->client->minimizeableChanged(false);
        QVERIFY(!button(DecorationButtonType::Minimize)->isEnabled());
        click(DecorationButtonType::Minimize);
        QCoreApplication::processEvents();
        QCOMPARE(bridge.client->minimizes, 1);
    }
    void maximizedBordersAndNarrowWindow()
    {
        flags(true, true);
        parent.moveTo(&slate);
        bridge.client->maximized = true;
        Q_EMIT bridge.client->client->maximizedChanged(true);
        QCOMPARE(deco->borderLeft(), 0);
        QCOMPARE(deco->borderRight(), 0);
        QCOMPARE(deco->borderBottom(), 0);
        QCOMPARE(deco->resizeOnlyBorders(), QMarginsF());
        bridge.client->clientWidth = 100;
        Q_EMIT bridge.client->client->widthChanged(100);
        QVERIFY(deco->borderTop() > 100);
        for (auto *item : deco->findChildren<DecorationButton *>()) {
            QVERIFY(item->geometry().bottom() <= deco->borderTop());
            QVERIFY(item->geometry().right() <= deco->size().width());
        }
    }
    void unsupportedParentFallsBack()
    {
        // Preview parents lack KWin's output meta-property and must remain safe.
        QObject preview;
        const QVariantMap args{{QStringLiteral("bridge"), QVariant::fromValue<DecorationBridge *>(&bridge)}};
        std::unique_ptr<Decoration> other(factory->create<Decoration>(&preview, QVariantList{args}));
        other->setSettings(settings);
        other->create();
        connect(other.get(), &Decoration::nextStateChanged, other.get(), [ptr = other.get()](const auto &state) { ptr->apply(state->clone()); });
        flags(true, true);
        QVERIFY(other->init());
        QVERIFY(other->borderTop() < 48);
    }
    void delayedStateNeverCoversClient()
    {
        disconnect(applyState);
        const qreal compact = deco->borderTop();
        flags(true, true);
        parent.moveTo(&slate);
        bridge.client->clientWidth = 100;
        Q_EMIT bridge.client->client->widthChanged(100);
        QCOMPARE(deco->borderTop(), compact);
        QVERIFY(deco->nextState()->borders().top() > compact);
        for (auto *item : deco->findChildren<DecorationButton *>()) {
            if (!item->geometry().isEmpty()) QVERIFY(item->geometry().bottom() <= compact);
        }
        deco->apply(deco->nextState()->clone());
        QCOMPARE(button(DecorationButtonType::Close)->geometry().width(), 48);
        QVERIFY(!button(DecorationButtonType::Close)->geometry().isEmpty());
    }
    void renderExamples()
    {
        const QString output = qEnvironmentVariable("PLASMA_SLATE_PREVIEW_OUTPUT");
        if (output.isEmpty()) return;
        QImage image(850, 190, QImage::Format_ARGB32_Premultiplied);
        image.fill(QColor(225, 229, 235));
        QPainter painter(&image);
        painter.setPen(QColor(40, 45, 50));
        painter.drawText(24, 22, QStringLiteral("Desktop monitor"));
        painter.save();
        painter.translate(24, 30);
        deco->paint(&painter, QRectF(0, 0, 802, deco->borderTop()));
        painter.restore();
        painter.drawText(24, 99, QStringLiteral("Plasma Slate · touch controls and move button enabled"));
        flags(true, true);
        parent.moveTo(&slate);
        painter.translate(24, 108);
        deco->paint(&painter, QRectF(0, 0, 802, deco->borderTop()));
        painter.end();
        QVERIFY(image.save(output));
    }
};

int main(int argc, char **argv)
{
    QTemporaryDir config;
    if (!config.isValid()) return 1;
    qputenv("XDG_CONFIG_HOME", config.path().toUtf8());
    QGuiApplication app(argc, argv);
    RuntimeTest test;
    return QTest::qExec(&test, argc, argv);
}

#include "runtimetest.moc"
