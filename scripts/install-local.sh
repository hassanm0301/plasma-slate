#!/usr/bin/env bash
# Build and install Plasma Slate for the current user only (no root): ~/.local/bin,
# ~/.local/share/applications, ~/.config/autostart, ~/.config/systemd/user.
#
#   scripts/install-local.sh            # build, install, enable and (re)start the service
#   scripts/install-local.sh --uninstall
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
BIN="$HOME/.local/bin"
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
AUTOSTART="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
UNITS="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

if [[ "${1:-}" == "--uninstall" ]]; then
  systemctl --user disable --now slated.service 2>/dev/null || true
  pkill -f "/plasma-slate-tray$" 2>/dev/null || true
  rm -f "$BIN/slated" "$BIN/plasma-slate-tray" "$APPS/io.github.hassanm0301.plasmaslate.desktop" \
    "$APPS/io.github.hassanm0301.plasmaslate.tray.desktop" "$AUTOSTART/io.github.hassanm0301.plasmaslate.tray.desktop" \
    "$UNITS/slated.service"
  systemctl --user daemon-reload
  echo "Uninstalled (pairings and settings in ~/.local/share/plasma-slate and ~/.config/plasma-slate kept)."
  exit 0
fi

echo "==> building daemon"
(cd server && cargo build --release --bin slated)
echo "==> building tray app"
cmake -S tray -B tray/build -G Ninja -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build tray/build

mkdir -p "$BIN" "$APPS" "$AUTOSTART" "$UNITS"
install -m755 server/target/release/slated "$BIN/slated"
install -m755 tray/build/plasma-slate-tray "$BIN/plasma-slate-tray"
sed "s|^Exec=/usr/bin/slated|Exec=$BIN/slated|" packaging/io.github.hassanm0301.plasmaslate.desktop >"$APPS/io.github.hassanm0301.plasmaslate.desktop"
sed "s|^Exec=/usr/bin/plasma-slate-tray|Exec=$BIN/plasma-slate-tray|" packaging/io.github.hassanm0301.plasmaslate.tray.desktop >"$APPS/io.github.hassanm0301.plasmaslate.tray.desktop"
cp "$APPS/io.github.hassanm0301.plasmaslate.tray.desktop" "$AUTOSTART/"
sed "s|^ExecStart=/usr/bin/slated|ExecStart=$BIN/slated|" packaging/slated.service >"$UNITS/slated.service"
# Development .desktop entries for the same binary would be redundant.
rm -f "$APPS"/io.github.hassanm0301.plasmaslate.dev.slated.desktop
command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true

systemctl --user daemon-reload
systemctl --user enable slated.service >/dev/null 2>&1
systemctl --user restart slated.service
pkill -f "/plasma-slate-tray$" 2>/dev/null || true
(setsid "$BIN/plasma-slate-tray" >/dev/null 2>&1 &)
sleep 1
systemctl --user --no-pager --lines=0 status slated.service | head -3
echo "==> installed. Check with: $BIN/slated doctor"
