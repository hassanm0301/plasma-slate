// SPDX-License-Identifier: GPL-3.0-or-later
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(__dirname + '/move-window.js', 'utf8');

const desktop = {name: 'DP-1'};
const secondDesktop = {name: 'HDMI-A-1'};
const slate = {name: 'Virtual-PlasmaSlate-tablet'};
const otherSlate = {name: 'Virtual-PlasmaSlate-other'};
const makeWindow = (extra = {}) => ({internalId: '{target}', output: slate, moveableAcrossScreens: true, ...extra});

function run(windows, screens, preferred = '', id = '{target}') {
    const moves = [];
    const workspace = {screens, windowList: () => windows, sendClientToScreen: (w, o) => moves.push([w, o])};
    const context = vm.createContext({workspace});
    vm.runInContext(source, context);
    context.moveSlateWindow(id, preferred);
    return moves;
}

test('returns to remembered monitor even if it is not first', () => {
    const window = makeWindow();
    assert.deepEqual(run([window], [desktop, slate, secondDesktop], secondDesktop.name), [[window, secondDesktop]]);
});
test('falls back to an available non-Slate monitor after unplugging original', () => {
    const window = makeWindow();
    assert.deepEqual(run([window], [slate, otherSlate, desktop], 'disconnected'), [[window, desktop]]);
});
test('does nothing when no desktop monitor remains', () => {
    assert.deepEqual(run([makeWindow()], [slate, otherSlate]), []);
});
test('closed window and changed focus cannot move another window', () => {
    assert.deepEqual(run([makeWindow({internalId: '{other}', caption: 'same title'})], [slate, desktop]), []);
});
test('window that already left the tablet stays where it is', () => {
    assert.deepEqual(run([makeWindow({output: secondDesktop})], [slate, desktop, secondDesktop]), []);
});
test('non-moveable windows are ignored', () => {
    assert.deepEqual(run([makeWindow({moveableAcrossScreens: false})], [slate, desktop]), []);
});
test('never selects another Slate even if it was remembered', () => {
    const window = makeWindow();
    assert.deepEqual(run([window], [otherSlate, desktop], otherSlate.name), [[window, desktop]]);
});
test('supports the unprefixed Slate output name too', () => {
    const window = makeWindow({output: {name: 'PlasmaSlate-legacy'}});
    assert.deepEqual(run([window], [slate, desktop]), [[window, desktop]]);
});
test('keeps maximized and tiled state and geometry under KWin control', () => {
    const window = makeWindow({maximizeMode: 3, tile: {id: 1}, frameGeometry: {x: 1200, y: 0, width: 800, height: 600}});
    const before = structuredClone(window);
    assert.deepEqual(run([window], [slate, desktop]), [[window, desktop]]);
    assert.deepEqual(window, before);
});
test('output names are matched literally, including quotes and Unicode', () => {
    const output = {name: 'Desk "é" \\ monitor'};
    const window = makeWindow();
    assert.deepEqual(run([window], [desktop, output], output.name), [[window, output]]);
});
