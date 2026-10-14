#!/usr/bin/env bash
# Dev helper: move the first window whose caption contains $1 to the output
# named $2, using a one-shot KWin script. Requires the Plasma session env
# (source scripts/dev-env.sh first when running from a non-graphical shell).
set -euo pipefail
caption="${1:?caption substring}"
output="${2:?output name}"
script="$(mktemp --suffix=.js)"
cat >"$script" <<EOF
const outs = workspace.screens.filter(o => o.name === "$output");
if (outs.length === 0) { print("slate: output not found: $output"); }
for (const w of workspace.windowList()) {
    if (w.caption.indexOf("$caption") !== -1 && outs.length > 0) {
        workspace.sendClientToScreen(w, outs[0]);
        w.frameGeometry = { x: outs[0].geometry.x, y: outs[0].geometry.y,
                            width: Math.min(w.frameGeometry.width, outs[0].geometry.width),
                            height: Math.min(w.frameGeometry.height, outs[0].geometry.height) };
        print("slate: moved " + w.caption + " to " + "$output");
        break;
    }
}
EOF
name="slate-move-$$"
id=$(qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.loadScript "$script" "$name")
qdbus6 org.kde.KWin "/Scripting/Script$id" org.kde.kwin.Script.run >/dev/null 2>&1 || \
  qdbus6 org.kde.KWin "/$id" org.kde.kwin.Script.run >/dev/null 2>&1 || true
sleep 0.5
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.unloadScript "$name" >/dev/null 2>&1 || true
rm -f "$script"
