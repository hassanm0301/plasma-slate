// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QList>
#include <QRectF>
#include <QString>
#include <algorithm>
#include <cmath>

namespace PlasmaSlate {

inline bool isSlateOutput(const QString &name)
{
    return name.startsWith(QLatin1String("Virtual-PlasmaSlate-"))
        || name.startsWith(QLatin1String("PlasmaSlate-"));
}

struct TitleLayout {
    QList<QRectF> left;
    QList<QRectF> right;
    QRectF caption;
    qreal height = 0;
};

// The same layout is used for the requested border height and the applied hit
// targets. Narrow windows wrap controls, preserving full-size touch targets.
// A frame narrower than one target can only accommodate a clipped target.
inline TitleLayout titleLayout(qreal width, const QList<qreal> &left,
                               const QList<qreal> &right, qreal size,
                               qreal padding, qreal gap)
{
    TitleLayout result;
    width = std::max(qreal(0), width);
    padding = std::min(padding, std::max(qreal(0), (width - size) / 2));
    const qreal available = std::max(qreal(0), width - 2 * padding);
    const auto extent = [gap](const QList<qreal> &items) {
        qreal sum = 0;
        for (qreal item : items) sum += item;
        return sum + std::max(qsizetype(0), items.size() - 1) * gap;
    };
    const qreal leftWidth = extent(left);
    const qreal rightWidth = extent(right);
    if (leftWidth + rightWidth + ((!left.isEmpty() && !right.isEmpty()) ? gap : 0) <= available) {
        qreal x = padding;
        for (qreal item : left) {
            result.left.append(QRectF(x, padding, item, size));
            x += item + gap;
        }
        x = width - padding - rightWidth;
        for (qreal item : right) {
            result.right.append(QRectF(x, padding, item, size));
            x += item + gap;
        }
        const qreal start = padding + leftWidth + (left.isEmpty() ? 0 : gap);
        const qreal end = width - padding - rightWidth - (right.isEmpty() ? 0 : gap);
        result.caption = QRectF(start, padding, std::max(qreal(0), end - start), size);
        result.height = size + 2 * padding;
        return result;
    }

    qreal x = padding;
    qreal y = padding;
    const auto appendRow = [&](const QList<qreal> &items, QList<QRectF> &rects) {
        for (qreal item : items) {
            const qreal itemWidth = std::min(item, available);
            if (x > padding && x + itemWidth > width - padding + 0.001) {
                x = padding;
                y += size + gap;
            }
            rects.append(QRectF(x, y, itemWidth, size));
            x += itemWidth + gap;
        }
    };
    appendRow(left, result.left);
    appendRow(right, result.right);
    result.height = y + size + padding;
    return result;
}

} // namespace PlasmaSlate
