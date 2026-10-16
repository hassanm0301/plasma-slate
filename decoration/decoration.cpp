// SPDX-License-Identifier: GPL-3.0-or-later
#include "decoration.h"
#include "layout.h"
#include "windowmove.h"

#include <KConfig>
#include <KConfigGroup>
#include <KDecoration3/DecoratedWindow>
#include <KDecoration3/DecorationSettings>
#include <KPluginFactory>
#include <QFontMetricsF>
#include <QMetaProperty>
#include <QPainter>
#include <QScopedValueRollback>

K_PLUGIN_FACTORY_WITH_JSON(PlasmaSlateFactory, "plasmaslatedecoration.json",
                          registerPlugin<PlasmaSlate::Decoration>();)

namespace PlasmaSlate {
using Type = KDecoration3::DecorationButtonType;

namespace {
struct State final : KDecoration3::DecorationState {
    bool touch = false;
    qreal buttonSize = 28;
    qreal padding = 3;
    qreal gap = 2;
    std::shared_ptr<KDecoration3::DecorationState> clone() const override
    {
        return std::make_shared<State>(*this);
    }
};

QColor translucent(QColor color, qreal alpha)
{
    color.setAlphaF(alpha);
    return color;
}
} // namespace

Decoration::Decoration(QObject *parent, const QVariantList &args)
    : KDecoration3::Decoration(parent, args), m_kwinWindow(parent)
{
}

std::shared_ptr<KDecoration3::DecorationState> Decoration::createState()
{
    return std::make_shared<State>();
}

bool Decoration::init()
{
    // KWin currently parents a decoration to its Window. This is deliberately
    // isolated and checked: the public KDecoration API does not expose outputs.
    // Previews and an incompatible future KWin retain ordinary compact controls.
    if (m_kwinWindow) {
        const auto *meta = m_kwinWindow->metaObject();
        const int index = meta->indexOfProperty("output");
        if (index >= 0) {
            const auto property = meta->property(index);
            if (property.isReadable() && property.hasNotifySignal()) {
                const auto slot = metaObject()->method(metaObject()->indexOfSlot("outputChanged()"));
                m_outputSupported = bool(QObject::connect(m_kwinWindow, property.notifySignal(), this, slot));
            }
        }
    }
    const auto config = settings();
    connect(config.get(), &KDecoration3::DecorationSettings::reconfigured, this, &Decoration::reconfigure);
    connect(config.get(), &KDecoration3::DecorationSettings::decorationButtonsLeftChanged, this, &Decoration::rebuildButtons);
    connect(config.get(), &KDecoration3::DecorationSettings::decorationButtonsRightChanged, this, &Decoration::rebuildButtons);
    connect(config.get(), &KDecoration3::DecorationSettings::fontChanged, this, &Decoration::updateMetrics);
    connect(config.get(), &KDecoration3::DecorationSettings::borderSizeChanged, this, &Decoration::updateMetrics);
    const auto client = window();
    connect(client, &KDecoration3::DecoratedWindow::widthChanged, this, &Decoration::updateMetrics);
    connect(client, &KDecoration3::DecoratedWindow::heightChanged, this, qOverload<>(&Decoration::update));
    connect(client, &KDecoration3::DecoratedWindow::maximizedChanged, this, &Decoration::updateMetrics);
    connect(client, &KDecoration3::DecoratedWindow::maximizedHorizontallyChanged, this, &Decoration::updateMetrics);
    connect(client, &KDecoration3::DecoratedWindow::maximizedVerticallyChanged, this, &Decoration::updateMetrics);
    connect(client, &KDecoration3::DecoratedWindow::adjacentScreenEdgesChanged, this, &Decoration::updateMetrics);
    connect(client, &KDecoration3::DecoratedWindow::resizeableChanged, this, &Decoration::updateMetrics);
    connect(client, &KDecoration3::DecoratedWindow::moveableChanged, this, &Decoration::updateMetrics);
    connect(client, &KDecoration3::DecoratedWindow::captionChanged, this, qOverload<>(&Decoration::update));
    connect(client, &KDecoration3::DecoratedWindow::activeChanged, this, qOverload<>(&Decoration::update));
    connect(client, &KDecoration3::DecoratedWindow::paletteChanged, this, qOverload<>(&Decoration::update));
    connect(client, &KDecoration3::DecoratedWindow::iconChanged, this, qOverload<>(&Decoration::update));
    connect(this, &KDecoration3::Decoration::currentStateChanged, this, &Decoration::layoutButtons);
    setOpaque(true);
    reconfigure();
    outputChanged();
    return true;
}

void Decoration::reconfigure()
{
    const KConfig config(QStringLiteral("plasma-slate/window-controlsrc"), KConfig::SimpleConfig);
    const KConfigGroup group(&config, QStringLiteral("General"));
    m_touchControls = group.readEntry("TouchControls", false);
    m_moveButton = group.readEntry("MoveButton", false);
    rebuildButtons();
}

void Decoration::outputChanged()
{
    QObject *output = m_outputSupported && m_kwinWindow
        ? m_kwinWindow->property("output").value<QObject *>() : nullptr;
    const QString name = output ? output->property("name").toString() : QString();
    m_onSlate = isSlateOutput(name);
    if (!name.isEmpty() && !m_onSlate) m_preferredOutput = name;
    updateMetrics();
}

Button *Decoration::makeButton(Type type)
{
    auto *button = new Button(type, this);
    connect(button, &Button::visibilityChanged, this, &Decoration::updateMetrics);
    return button;
}

void Decoration::rebuildButtons()
{
    // Buttons use the framework's actions, including middle/right maximize,
    // window-menu double click, application menus, and capability changes.
    qDeleteAll(m_left);
    qDeleteAll(m_right);
    m_left.clear();
    m_right.clear();
    m_move = nullptr;
    auto leftTypes = settings()->decorationButtonsLeft();
    const auto rightTypes = settings()->decorationButtonsRight();
    if (!leftTypes.contains(Type::Menu) && !rightTypes.contains(Type::Menu)) {
        leftTypes.prepend(Type::Menu);
    }
    for (Type type : leftTypes) {
        if (type != Type::Custom) m_left.append(makeButton(type));
    }
    m_move = makeButton(Type::Custom);
    m_right.append(m_move);
    connect(m_move, &Button::clicked, this, [this] {
        if (m_onSlate && m_moveButton && m_kwinWindow) {
            WindowMove::request(m_kwinWindow, m_preferredOutput);
        }
    }, Qt::QueuedConnection);
    for (Type type : rightTypes) {
        if (type != Type::Custom) m_right.append(makeButton(type));
    }
    updateMetrics();
}

QList<qreal> Decoration::buttonWidths(const QList<Button *> &buttons, qreal size) const
{
    QList<qreal> widths;
    for (const auto *button : buttons) {
        if (button->isVisible()) widths.append(button->type() == Type::Spacer ? size / 3 : size);
    }
    return widths;
}

void Decoration::updateMetrics()
{
    if (m_layingOut || !m_move) return;
    QScopedValueRollback guard(m_layingOut, true);
    m_move->setVisible(m_onSlate && m_moveButton);
    const QVariant acrossScreens = m_kwinWindow ? m_kwinWindow->property("moveableAcrossScreens") : QVariant();
    m_move->setEnabled(acrossScreens.isValid() ? acrossScreens.toBool() : window()->isMoveable());
    const bool touch = m_onSlate && m_touchControls;
    const qreal buttonSize = std::max(touch ? qreal(48) : qreal(26), std::ceil(settings()->fontMetrics().height()) + 8);
    const qreal padding = touch ? 4 : 3;
    const qreal gap = touch ? 4 : 2;
    qreal border = 1;
    switch (settings()->borderSize()) {
    case KDecoration3::BorderSize::None: border = 0; break;
    case KDecoration3::BorderSize::NoSides: border = 0; break;
    case KDecoration3::BorderSize::Tiny: border = 1; break;
    case KDecoration3::BorderSize::Normal: border = 3; break;
    case KDecoration3::BorderSize::Large: border = 4; break;
    case KDecoration3::BorderSize::VeryLarge: border = 6; break;
    case KDecoration3::BorderSize::Huge: border = 8; break;
    case KDecoration3::BorderSize::VeryHuge: border = 10; break;
    case KDecoration3::BorderSize::Oversized: border = 16; break;
    }
    const auto edges = window()->adjacentScreenEdges();
    const qreal left = window()->isMaximizedHorizontally() || edges.testFlag(Qt::LeftEdge) ? 0 : border;
    const qreal right = window()->isMaximizedHorizontally() || edges.testFlag(Qt::RightEdge) ? 0 : border;
    const qreal bottom = window()->isMaximizedVertically() || edges.testFlag(Qt::BottomEdge) ? 0
        : settings()->borderSize() == KDecoration3::BorderSize::NoSides ? 3 : border;
    const auto layout = titleLayout(window()->width() + left + right,
                                    buttonWidths(m_left, buttonSize), buttonWidths(m_right, buttonSize),
                                    buttonSize, padding, gap);
    const QMarginsF margins(left, layout.height, right, bottom);
    const auto requested = std::static_pointer_cast<State>(nextState());
    if (requested->borders() != margins || requested->touch != touch
        || requested->buttonSize != buttonSize || requested->padding != padding || requested->gap != gap) {
        setState([=](KDecoration3::DecorationState *state) {
            auto *next = static_cast<State *>(state);
            next->touch = touch;
            next->buttonSize = buttonSize;
            next->padding = padding;
            next->gap = gap;
            next->setBorders(margins);
        });
    }
    const qreal resizeMargin = window()->isResizeable() ? (touch ? 10 : 5) : 0;
    setResizeOnlyBorders(QMarginsF(
        window()->isMaximizedHorizontally() || edges.testFlag(Qt::LeftEdge) ? 0 : resizeMargin,
        window()->isMaximizedVertically() || edges.testFlag(Qt::TopEdge) ? 0 : resizeMargin,
        window()->isMaximizedHorizontally() || edges.testFlag(Qt::RightEdge) ? 0 : resizeMargin,
        window()->isMaximizedVertically() || edges.testFlag(Qt::BottomEdge) ? 0 : resizeMargin));
    layoutButtons();
}

void Decoration::setButtonGeometry(const QList<Button *> &buttons, const QList<QRectF> &rects)
{
    qsizetype index = 0;
    for (auto *button : buttons) {
        QRectF geometry;
        if (button->isVisible() && index < rects.size()) geometry = rects[index++];
        // A resize/configure is asynchronous. Do not let a newly wrapped row
        // intercept clicks in the client before KWin applies its taller border.
        if (geometry.bottom() > borderTop() + 0.001) geometry = QRectF();
        button->setGeometry(geometry);
    }
}

void Decoration::layoutButtons()
{
    const auto state = std::static_pointer_cast<State>(currentState());
    const auto layout = titleLayout(size().width(), buttonWidths(m_left, state->buttonSize),
                                    buttonWidths(m_right, state->buttonSize), state->buttonSize,
                                    state->padding, state->gap);
    setTitleBar(QRectF(0, 0, size().width(), borderTop()));
    setButtonGeometry(m_left, layout.left);
    setButtonGeometry(m_right, layout.right);
    m_caption = layout.caption.intersected(titleBar());
    update();
}

bool Decoration::touchMode() const
{
    return std::static_pointer_cast<State>(currentState())->touch;
}

QColor Decoration::foreground() const
{
    const auto group = window()->isActive() ? KDecoration3::ColorGroup::Active : KDecoration3::ColorGroup::Inactive;
    const QColor color = window()->color(group, KDecoration3::ColorRole::Foreground);
    return color.isValid() ? color : window()->palette().color(QPalette::WindowText);
}

void Decoration::paint(QPainter *painter, const QRectF &repaintArea)
{
    painter->save();
    painter->setClipRect(repaintArea);
    const auto group = window()->isActive() ? KDecoration3::ColorGroup::Active : KDecoration3::ColorGroup::Inactive;
    QColor background = window()->color(group, KDecoration3::ColorRole::TitleBar);
    if (!background.isValid()) background = window()->palette().color(QPalette::Window);
    painter->fillRect(rect(), background);
    painter->setPen(QPen(translucent(foreground(), window()->isActive() ? 0.20 : 0.12), 1));
    painter->drawRect(rect().adjusted(0.5, 0.5, -0.5, -0.5));
    painter->drawLine(QPointF(0, borderTop() - 0.5), QPointF(size().width(), borderTop() - 0.5));
    if (m_caption.width() > 8) {
        painter->save();
        painter->setClipRect(m_caption, Qt::IntersectClip);
        painter->setFont(settings()->font());
        painter->setPen(foreground());
        const auto text = painter->fontMetrics().elidedText(window()->caption(), Qt::ElideRight, int(m_caption.width() - 8));
        painter->drawText(m_caption.adjusted(4, 0, -4, 0), Qt::AlignCenter | Qt::TextSingleLine, text);
        painter->restore();
    }
    for (auto *button : m_left) button->paint(painter, repaintArea);
    for (auto *button : m_right) button->paint(painter, repaintArea);
    painter->restore();
}

Button::Button(Type type, Decoration *decoration)
    : KDecoration3::DecorationButton(type, decoration, decoration)
{
    connect(this, &Button::hoveredChanged, this, [this](bool hovered) {
        if (hovered && this->type() != Type::Spacer) this->decoration()->requestShowToolTip(tooltip());
        else this->decoration()->requestHideToolTip();
        update();
    });
    connect(this, &Button::pressedChanged, this, qOverload<>(&Button::update));
    connect(this, &Button::checkedChanged, this, qOverload<>(&Button::update));
    connect(this, &Button::enabledChanged, this, qOverload<>(&Button::update));
}

QString Button::tooltip() const
{
    switch (type()) {
    case Type::Menu: return tr("Window menu");
    case Type::ApplicationMenu: return tr("Application menu");
    case Type::Minimize: return tr("Minimize");
    case Type::Maximize: return isChecked() ? tr("Restore") : tr("Maximize");
    case Type::Close: return tr("Close");
    case Type::Custom: return tr("Move to other monitor");
    case Type::OnAllDesktops: return tr("On all desktops");
    case Type::ContextHelp: return tr("Help");
    case Type::Shade: return tr("Shade");
    case Type::KeepAbove: return tr("Keep above other windows");
    case Type::KeepBelow: return tr("Keep below other windows");
    case Type::ExcludeFromCapture: return tr("Hide from screen capture");
    case Type::Spacer: return {};
    }
    return {};
}

void Button::paint(QPainter *painter, const QRectF &repaintArea)
{
    if (!isVisible() || geometry().isEmpty() || type() == Type::Spacer || !geometry().intersects(repaintArea)) return;
    const auto *deco = static_cast<Decoration *>(decoration());
    const bool touch = deco->touchMode();
    QColor ink = deco->foreground();
    QColor accent = deco->window()->palette().color(QPalette::Highlight);
    if (type() == Type::Close) {
        accent = deco->window()->color(KDecoration3::ColorGroup::Warning, KDecoration3::ColorRole::Foreground);
        if (!accent.isValid()) accent = QColor(218, 68, 83);
    }
    painter->save();
    painter->setClipRect(geometry(), Qt::IntersectClip);
    painter->setRenderHint(QPainter::Antialiasing);
    const QRectF face = geometry().adjusted(2, 2, -2, -2);
    painter->setPen(Qt::NoPen);
    if (isEnabled() && (isPressed() || isHovered())) {
        painter->setBrush(isPressed() ? accent.darker(115) : accent);
        ink = deco->window()->palette().color(QPalette::HighlightedText);
        painter->drawRoundedRect(face, touch ? 9 : 4, touch ? 9 : 4);
    } else if (touch || isChecked()) {
        painter->setBrush(translucent(isChecked() ? accent : ink, isChecked() ? 0.18 : 0.07));
        painter->drawRoundedRect(face, touch ? 9 : 4, touch ? 9 : 4);
    }
    if (!isEnabled()) ink.setAlphaF(0.30);
    painter->setPen(QPen(ink, touch ? 1.9 : 1.4, Qt::SolidLine, Qt::RoundCap, Qt::RoundJoin));
    painter->setBrush(Qt::NoBrush);
    painter->translate(geometry().center());
    const qreal scale = touch ? 1.3 : 1.0;
    painter->scale(scale, scale);
    const auto line = [painter](qreal x1, qreal y1, qreal x2, qreal y2) {
        painter->drawLine(QPointF(x1, y1), QPointF(x2, y2));
    };
    switch (type()) {
    case Type::Close:
        line(-5, -5, 5, 5); line(-5, 5, 5, -5); break;
    case Type::Minimize:
        line(-5, 3, 5, 3); break;
    case Type::Maximize:
        if (isChecked()) {
            painter->drawRect(QRectF(-5, -2, 7, 7));
            line(-2, -5, 5, -5); line(5, -5, 5, 2); line(2, 2, 5, 2);
        } else painter->drawRect(QRectF(-5, -5, 10, 10));
        break;
    case Type::Custom:
        painter->drawRoundedRect(QRectF(-7, -6, 11, 9), 1, 1);
        line(-4, 6, 1, 6); line(-1.5, 3, -1.5, 6);
        line(0, -1, 8, -1); line(5, -4, 8, -1); line(8, -1, 5, 2); break;
    case Type::Menu:
        if (!touch && !deco->window()->icon().isNull()) {
            deco->window()->icon().paint(painter, QRect(-8, -8, 16, 16));
            break;
        }
        [[fallthrough]];
    case Type::ApplicationMenu:
        line(-5, -4, 5, -4); line(-5, 0, 5, 0); line(-5, 4, 5, 4); break;
    case Type::KeepAbove:
    case Type::KeepBelow:
    case Type::Shade: {
        const bool down = type() == Type::KeepBelow || (type() == Type::Shade && isChecked());
        const qreal sign = down ? -1 : 1;
        line(-5, 2 * sign, 0, -3 * sign); line(0, -3 * sign, 5, 2 * sign);
        if (type() == Type::Shade) line(-5, -6 * sign, 5, -6 * sign);
        else { line(-5, 6 * sign, 0, sign); line(0, sign, 5, 6 * sign); }
        break;
    }
    case Type::OnAllDesktops:
        painter->drawEllipse(QRectF(-3, -6, 6, 6));
        line(0, 0, 0, 6); line(-4, 2, 4, 2); break;
    case Type::ContextHelp: {
        QFont font = painter->font();
        font.setPixelSize(16);
        font.setBold(true);
        painter->setFont(font);
        painter->drawText(QRectF(-8, -10, 16, 20), Qt::AlignCenter, QStringLiteral("?"));
        break;
    }
    case Type::ExcludeFromCapture:
        painter->drawRoundedRect(QRectF(-6, -4, 12, 8), 3, 3);
        painter->drawEllipse(QRectF(-2, -2, 4, 4));
        line(-7, -7, 7, 7); break;
    case Type::Spacer: break;
    }
    painter->restore();
}

} // namespace PlasmaSlate

#include "decoration.moc"
