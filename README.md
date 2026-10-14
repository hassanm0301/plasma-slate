# Plasma Slate

Use an Android tablet as a real, touch-enabled extra monitor for **KDE Plasma on Wayland**.

- The tablet is a normal screen in *System Settings → Display & Monitor*: arrange it anywhere, change its scale.
  Its position is remembered per tablet.
- Your mouse moves onto it; windows drag between screens.
- **Touch is independent of the mouse**: tapping or dragging on the tablet doesn't move your cursor.
- Works over **Wi-Fi/LAN** and over a **USB cable** (the app opens by itself when you plug the tablet in).
- Hardware video encoding on the PC (VA-API) and hardware decoding on the tablet; ~45–60 ms from PC to tablet
  on the reference setup.
- New tablets must be approved on the PC (Allow/Deny notification); the connection is TLS 1.3 with
  certificate pinning.

Status: working on the reference setup (Plasma 6.7, Radeon RX 9070 XT, Huawei MatePad T 10s). See
[docs/PLAN.md](docs/PLAN.md) and [docs/TESTING.md](docs/TESTING.md).

## Requirements

**PC:** KDE Plasma 6.7+ Wayland session, PipeWire, `kscreen-doctor`, GStreamer with
`gst-plugin-pipewire` and `gst-plugin-va` (for hardware encoding). Optional: `android-tools` for USB.

**Tablet:** Android 10 or newer with H.264 or HEVC hardware decoding.

## Install

### PC (Arch Linux)

```sh
sudo pacman -S --needed gst-plugin-pipewire gst-plugin-va gst-plugins-bad android-tools
# from a clone of this repository:
scripts/install-local.sh          # builds, installs to ~/.local, enables the systemd user service + tray
~/.local/bin/slated doctor        # checks that everything the daemon needs is available
```

A PKGBUILD is in [`packaging/arch`](packaging/arch/PKGBUILD) (`makepkg -si` there installs system-wide; then
`systemctl --user enable --now slated`).

### Tablet

Install the APK (built by CI, or `cd android && ./gradlew assembleDebug`):

```sh
adb install android/app/build/outputs/apk/debug/app-debug.apk
```

## Use

1. Open **Plasma Slate** on the tablet. PCs on the same network are listed; tap yours (or *USB cable*).
2. The first time, a notification on the PC asks **Allow tablet to connect?** Check the fingerprint shown on
   both sides and click **Allow**.
3. The tablet becomes a screen. Arrange it in *Display & Monitor*. Press **Back** on the tablet to disconnect.

Over USB: enable USB debugging on the tablet and plug it in. The daemon forwards the connection
(`adb reverse`) and opens the app.

The tray icon shows connected tablets, latency and bitrate, and lets you disconnect or forget a tablet.

### Resolution and refresh rate (performance)

Lowering the stream resolution or frame rate reduces work everywhere: KWin composites a smaller screen, the
GPU encodes fewer pixels and frames, the network carries less and the tablet decodes faster (which also lowers
latency). The tablet scales the picture up to its panel; the aspect ratio is always kept and the stream is never
larger than the tablet's own resolution.

- **Tray:** *tablet ▸ Resolution* (Native, 83/75/67/50 %, Custom…) and *Refresh rate* (Automatic, 60…15 Hz).
  The tablet reconnects within a second with the new settings, and they are remembered for that tablet.
- **CLI:** `slated stream <device-id> --resolution 1280x800 --fps 30` (`slated devices` lists ids; without
  options it prints the current settings).
- **Defaults for all tablets:** `resolution` / `fps` in `slated.toml`.

When the resolution changes, the tablet keeps its position and gets a fresh automatic scale.

## Configuration

`~/.config/plasma-slate/slated.toml` (all optional; *Edit configuration…* in the tray creates a template):

| Key | Default | Meaning |
|---|---|---|
| `port` | `47420` | TCP port |
| `codec` | `"auto"` | `auto` (tablet's fastest decoder first), `h264`, `hevc` |
| `bitrate_kbps` | adaptive | fixed bitrate instead of adaptive |
| `mdns` | `true` | advertise on the LAN |
| `usb_adb` / `usb_autostart` | `true` / `true` | USB forwarding / open the app when plugged in |
| `max_sessions` | `4` | simultaneous tablets |
| `resolution` | `"native"` | default stream size for tablets without their own setting: `native`, `75%`, `1280x800` |
| `fps` | `0` | default frame rate; `0` = the tablet's refresh rate |
| `pairing_timeout_secs` | `60` | how long the Allow/Deny prompt waits |

Restart after changes: `systemctl --user restart slated`.

Firewall: allow TCP 47420 and mDNS (UDP 5353) on your LAN interface.

## Troubleshooting

- `slated doctor` reports what's missing.
- *"KWin did not expose zkde_screencast_unstable_v1"*: the daemon's `.desktop` file
  (`io.github.hassanm0301.plasmaslate.desktop`) must be installed and its `Exec=` must match the binary path.
  KWin only grants its screen-sharing and input interfaces to apps listed this way.
- Logs: `journalctl --user -u slated -f` (`SLATE_LOG=debug` for more).
- Tablet shows "identity changed": the PC's key was regenerated; long-press the PC in the list → *Forget*,
  then pair again.

## Layout

| Path | What |
|---|---|
| `proto/` | wire protocol (protobuf) |
| `server/` | Rust daemon `slated` (crates: protocol, KWin, media, net, daemon) |
| `android/` | Kotlin tablet app |
| `tray/` | Qt6/KF6 tray app |
| `packaging/` | `.desktop` files, systemd user unit, PKGBUILD |
| `docs/` | plan, ADRs, spike findings, test checklist |
| `spikes/` | M0 experiments |

## Licence

GPL-3.0-or-later. See [LICENSE](LICENSE).
