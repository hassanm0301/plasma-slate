// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <KDecoration3/Decoration>
#include <KDecoration3/DecorationButton>
#include <QPointer>

namespace PlasmaSlate {

class Button;

class Decoration final : public KDecoration3::Decoration
{
    Q_OBJECT
public:
    explicit Decoration(QObject *parent, const QVariantList &args);
    bool init() override;
    void paint(QPainter *painter, const QRectF &repaintArea) override;
    bool touchMode() const;
    QColor foreground() const;

protected:
    std::shared_ptr<KDecoration3::DecorationState> createState() override;

private Q_SLOTS:
    void outputChanged();
    void reconfigure();
    void rebuildButtons();
    void updateMetrics();
    void layoutButtons();

private:
    Button *makeButton(KDecoration3::DecorationButtonType type);
    QList<qreal> buttonWidths(const QList<Button *> &buttons, qreal size) const;
    void setButtonGeometry(const QList<Button *> &buttons, const QList<QRectF> &rects);

    QPointer<QObject> m_kwinWindow;
    QString m_preferredOutput;
    QList<Button *> m_left;
    QList<Button *> m_right;
    Button *m_move = nullptr;
    QRectF m_caption;
    bool m_outputSupported = false;
    bool m_onSlate = false;
    bool m_touchControls = false;
    bool m_moveButton = false;
    bool m_layingOut = false;
};

class Button final : public KDecoration3::DecorationButton
{
    Q_OBJECT
public:
    Button(KDecoration3::DecorationButtonType type, Decoration *decoration);
    void paint(QPainter *painter, const QRectF &repaintArea) override;
private:
    QString tooltip() const;
};

} // namespace PlasmaSlate
