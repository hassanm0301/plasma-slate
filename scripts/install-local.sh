#!/usr/bin/env bash
# Build and install Plasma Slate for the current user only (no root): ~/.local/bin,
# ~/.local/share/applications, ~/.local/lib/qt6/plugins, ~/.config/systemd/user,
# and a Plasma login environment script for plugin discovery.
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
DECORATION_DIR="$HOME/.local/lib/qt6/plugins/org.kde.kdecoration3"
DECORATION_PLUGIN="$DECORATION_DIR/io.github.hassanm0301.plasmaslate.so"
WINDOW_CONTROLS_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/plasma-slate/window-controlsrc"
PLASMA_ENV_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/plasma-workspace/env"
PLASMA_ENV_SCRIPT="$PLASMA_ENV_DIR/plasma-slate.sh"
PLASMA_ENV_MARKER="# Managed by Plasma Slate local installer."

if [[ "${1:-}" == "--uninstall" ]]; then
  # Restore the saved decoration while its plugin and controller are still available.
  # Fail before removing either if KWin cannot be restored.
  # A missing plugin can still be selected in kwinrc after a partial install or
  # manual removal. Its saved settings must be restored in that case as well.
  # Older installations without this feature have neither file and can uninstall normally.
  if [[ -f "$DECORATION_PLUGIN" || -f "$WINDOW_CONTROLS_CONFIG" ]]; then
    if [[ ! -x "$BIN/plasma-slate-tray" ]] \
      || ! QT_QPA_PLATFORM=offscreen "$BIN/plasma-slate-tray" --help | grep -Fq -- '--disable-window-controls'; then
      echo "Cannot restore window controls: the installed tray app is missing or too old. Reinstall before uninstalling." >&2
      exit 1
    fi
    QT_QPA_PLATFORM=offscreen "$BIN/plasma-slate-tray" --disable-window-controls
  fi
  systemctl --user disable --now slated.service plasma-slate-tray.service 2>/dev/null || true
  pkill -f "/plasma-slate-tray$" 2>/dev/null || true
  rm -f "$BIN/slated" "$BIN/plasma-slate-tray" "$APPS/io.github.hassanm0301.plasmaslate.desktop" \
    "$APPS/io.github.hassanm0301.plasmaslate.tray.desktop" "$AUTOSTART/io.github.hassanm0301.plasmaslate.tray.desktop" \
    "$UNITS/slated.service" "$UNITS/plasma-slate-tray.service" "$DECORATION_PLUGIN"
  if [[ -f "$PLASMA_ENV_SCRIPT" ]] && grep -Fqx "$PLASMA_ENV_MARKER" "$PLASMA_ENV_SCRIPT"; then
    rm -f "$PLASMA_ENV_SCRIPT"
  fi
  systemctl --user daemon-reload
  echo "Uninstalled (pairings and settings in ~/.local/share/plasma-slate and ~/.config/plasma-slate kept)."
  exit 0
fi

if [[ -e "$PLASMA_ENV_SCRIPT" ]] && ! grep -Fqx "$PLASMA_ENV_MARKER" "$PLASMA_ENV_SCRIPT"; then
  echo "Cannot install plugin environment: $PLASMA_ENV_SCRIPT already exists and is not managed by Plasma Slate." >&2
  exit 1
fi

echo "==> building daemon"
(cd server && cargo build --release --bin slated)
echo "==> building tray app"
cmake -S tray -B tray/build -G Ninja -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build tray/build
echo "==> building optional window decoration"
cmake -S decoration -B decoration/build -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DDECORATION_INSTALL_DIR="$DECORATION_DIR" >/dev/null
cmake --build decoration/build

mkdir -p "$BIN" "$APPS" "$UNITS" "$DECORATION_DIR" "$PLASMA_ENV_DIR"
install -m755 server/target/release/slated "$BIN/slated"
install -m755 tray/build/plasma-slate-tray "$BIN/plasma-slate-tray"
# KWin may already have the plugin mapped. Replace its directory entry atomically
# so updates never overwrite the inode in use by the compositor.
DECORATION_STAGE="$(mktemp "$DECORATION_PLUGIN.XXXXXX")"
trap 'rm -f "$DECORATION_STAGE"' EXIT
install -m755 decoration/build/io.github.hassanm0301.plasmaslate.so "$DECORATION_STAGE"
mv -f "$DECORATION_STAGE" "$DECORATION_PLUGIN"
# Plasma sources this before KWin starts. A running KWin cannot gain a new plugin
# search path; the local decoration becomes available at the next login.
cat >"$PLASMA_ENV_SCRIPT" <<'EOF'
#!/bin/sh
# Managed by Plasma Slate local installer.
export QT_PLUGIN_PATH="$HOME/.local/lib/qt6/plugins${QT_PLUGIN_PATH:+:$QT_PLUGIN_PATH}"
EOF
chmod 755 "$PLASMA_ENV_SCRIPT"
sed "s|^Exec=/usr/bin/slated|Exec=$BIN/slated|" packaging/io.github.hassanm0301.plasmaslate.desktop >"$APPS/io.github.hassanm0301.plasmaslate.desktop"
sed "s|^Exec=/usr/bin/plasma-slate-tray|Exec=$BIN/plasma-slate-tray|" packaging/io.github.hassanm0301.plasmaslate.tray.desktop >"$APPS/io.github.hassanm0301.plasmaslate.tray.desktop"
sed "s|^ExecStart=/usr/bin/slated|ExecStart=$BIN/slated|" packaging/slated.service >"$UNITS/slated.service"
sed "s|^ExecStart=/usr/bin/plasma-slate-tray|ExecStart=$BIN/plasma-slate-tray|" packaging/plasma-slate-tray.service \
  >"$UNITS/plasma-slate-tray.service"
# The tray used to start from XDG autostart; slated.service now pulls it in.
rm -f "$AUTOSTART/io.github.hassanm0301.plasmaslate.tray.desktop"
# Development .desktop entries for the same binary would be redundant.
rm -f "$APPS"/io.github.hassanm0301.plasmaslate.dev.slated.desktop
command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true

systemctl --user daemon-reload
systemctl --user enable slated.service >/dev/null 2>&1
pkill -f "/plasma-slate-tray$" 2>/dev/null || true
systemctl --user restart slated.service plasma-slate-tray.service
sleep 1
systemctl --user --no-pager --lines=0 status slated.service plasma-slate-tray.service | grep -E '^●|Active:'
echo "==> installed. Check with: $BIN/slated doctor"
echo "==> sign out and back in before first enabling the optional window controls."
