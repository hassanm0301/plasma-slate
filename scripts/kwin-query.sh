#!/usr/bin/env bash
# Dev helper: print the cursor position and the geometry/output of windows whose
# caption contains $1 (default: all), via a one-shot KWin script whose print()
# output is read back from the user journal.
set -euo pipefail
caption="${1:-}"
tag="slateq$$"
script="$(mktemp --suffix=.js)"
cat >"$script" <<EOF
print("$tag cursor " + workspace.cursorPos.x + "," + workspace.cursorPos.y);
for (const w of workspace.windowList()) {
    if ("$caption" === "" || w.caption.indexOf("$caption") !== -1) {
        const g = w.frameGeometry;
        print("$tag window '" + w.caption + "' " + g.x + "," + g.y + " " + g.width + "x" + g.height + " output=" + (w.output ? w.output.name : "?"));
    }
}
for (const o of workspace.screens) {
    print("$tag output " + o.name + " " + o.geometry.x + "," + o.geometry.y + " " + o.geometry.width + "x" + o.geometry.height);
}
EOF
since="$(date '+%Y-%m-%d %H:%M:%S')"
id=$(qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.loadScript "$script" "$tag")
qdbus6 org.kde.KWin "/Scripting/Script$id" org.kde.kwin.Script.run >/dev/null 2>&1 || true
sleep 0.4
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.unloadScript "$tag" >/dev/null 2>&1 || true
rm -f "$script"
journalctl --user --since "$since" -o cat 2>/dev/null | grep "$tag" | sed "s/.*$tag //"
