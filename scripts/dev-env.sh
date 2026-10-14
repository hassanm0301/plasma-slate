#!/usr/bin/env bash
# Source this from a non-graphical shell (ssh, agents, cron) to talk to the
# running Plasma Wayland session of the current user.
#   . scripts/dev-env.sh
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
export WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-0}"
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=$XDG_RUNTIME_DIR/bus}"
export QT_QPA_PLATFORM="${QT_QPA_PLATFORM:-wayland}"
