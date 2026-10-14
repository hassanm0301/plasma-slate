#!/usr/bin/env bash
# KWin only exposes restricted Wayland protocols / D-Bus interfaces to
# executables that have an installed .desktop file whose Exec= points at them.
# This installs such files for development builds (per-user, no root needed).
#
# Usage: scripts/install-dev-desktop-files.sh <absolute-binary-path> [id]
set -euo pipefail

bin="${1:?absolute path to binary}"
id="${2:-$(basename "$bin")}"
[[ "$bin" = /* ]] || { echo "path must be absolute" >&2; exit 1; }

dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
mkdir -p "$dir"
file="$dir/io.github.hassanm0301.plasmaslate.dev.$id.desktop"
cat >"$file" <<EOF
[Desktop Entry]
Type=Application
Name=Plasma Slate dev ($id)
Exec=$bin
NoDisplay=true
X-KDE-Wayland-Interfaces=zkde_screencast_unstable_v1,org_kde_kwin_fake_input
X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.EIS.RemoteDesktop,org.kde.KWin.ScreenShot2
EOF
echo "installed $file"
command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true
