// SPDX-License-Identifier: GPL-3.0-or-later
// Evaluated inside KWin. Target by immutable UUID, never by caption or focus.
function moveSlateWindow(windowId, preferredOutput) {
    function isSlate(output) {
        return output && /^(Virtual-)?PlasmaSlate-/.test(output.name);
    }

    const window = workspace.windowList().find(w => String(w.internalId) === windowId);
    // Recheck at execution time: the window may have closed or already left the tablet.
    if (!window || !isSlate(window.output) || !window.moveableAcrossScreens)
        return;

    const outputs = workspace.screens.filter(output => !isSlate(output));
    const target = outputs.find(output => output.name === preferredOutput) || outputs[0];
    if (!target)
        return;

    // KWin handles output scaling, available work area and maximized/tiled geometry.
    workspace.sendClientToScreen(window, target);
}
