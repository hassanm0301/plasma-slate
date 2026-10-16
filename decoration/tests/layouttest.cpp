// SPDX-License-Identifier: GPL-3.0-or-later
#include "../layout.h"
#include <QtTest>

class LayoutTest : public QObject
{
    Q_OBJECT
private Q_SLOTS:
    void outputIdentity()
    {
        QVERIFY(PlasmaSlate::isSlateOutput(QStringLiteral("Virtual-PlasmaSlate-1")));
        QVERIFY(PlasmaSlate::isSlateOutput(QStringLiteral("PlasmaSlate-tablet")));
        QVERIFY(!PlasmaSlate::isSlateOutput(QStringLiteral("DP-1")));
        QVERIFY(!PlasmaSlate::isSlateOutput(QStringLiteral("Virtual-1")));
        QVERIFY(!PlasmaSlate::isSlateOutput(QString()));
    }

    void normalDesktop()
    {
        const auto layout = PlasmaSlate::titleLayout(800, {26}, {26, 26, 26}, 26, 3, 2);
        QCOMPARE(layout.height, 32);
        QCOMPARE(layout.left.first(), QRectF(3, 3, 26, 26));
        QCOMPARE(layout.right.last().right(), 797);
        QVERIFY(layout.caption.left() > layout.left.last().right());
        QVERIFY(layout.caption.right() < layout.right.first().left());
    }

    void touchTargetsStayLarge_data()
    {
        QTest::addColumn<double>("width");
        for (double width : {48., 52., 80., 100., 180., 260., 1200.}) {
            QTest::newRow(qPrintable(QString::number(width))) << width;
        }
    }

    void touchTargetsStayLarge()
    {
        QFETCH(double, width);
        const auto layout = PlasmaSlate::titleLayout(width, {48}, {48, 48, 48, 48}, 48, 4, 4);
        const auto rects = layout.left + layout.right;
        QCOMPARE(rects.size(), 5);
        for (qsizetype i = 0; i < rects.size(); ++i) {
            const auto target = rects[i];
            QCOMPARE(target.width(), 48);
            QCOMPARE(target.height(), 48);
            QVERIFY(target.left() >= 0);
            QVERIFY(target.right() <= width);
            QVERIFY(target.bottom() <= layout.height);
            for (qsizetype j = i + 1; j < rects.size(); ++j) QVERIFY(!target.intersects(rects[j]));
            QVERIFY(!target.intersects(layout.caption));
        }
        if (width < 260) QVERIFY(layout.caption.isEmpty());
    }

    void extremelyNarrowFramesStayInsideBorder()
    {
        for (qreal width : {0., 1., 20., 47.}) {
            const auto layout = PlasmaSlate::titleLayout(width, {48}, {48, 48, 48, 48}, 48, 4, 4);
            const auto rects = layout.left + layout.right;
            for (qsizetype i = 0; i < rects.size(); ++i) {
                QVERIFY(rects[i].left() >= 0);
                QVERIFY(rects[i].right() <= width);
                QVERIFY(rects[i].bottom() <= layout.height);
                for (qsizetype j = i + 1; j < rects.size(); ++j) QVERIFY(!rects[i].intersects(rects[j]));
            }
        }
    }

    void customArrangementAndSpacer()
    {
        const auto layout = PlasmaSlate::titleLayout(640, {48, 16, 48}, {48, 48}, 48, 4, 4);
        QCOMPARE(layout.left.size(), 3);
        QCOMPARE(layout.left[1].width(), 16);
        QVERIFY(layout.left[2].left() > layout.left[1].right());
        QVERIFY(layout.right.first().left() > layout.left.last().right());
    }
};

QTEST_GUILESS_MAIN(LayoutTest)
#include "layouttest.moc"
