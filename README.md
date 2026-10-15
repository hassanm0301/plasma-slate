# Plasma Slate

Use an Android tablet as a real, touch-enabled extra monitor for **KDE Plasma on Wayland**.

- The tablet is a normal screen in *System Settings → Display & Monitor*: arrange it anywhere, change its scale.
  Its position is remembered per tablet.
- Your mouse moves onto it; windows drag between screens.
- **Touch is independent of the mouse**: tapping or dragging on the tablet doesn't move your cursor.
- Works over **Wi-Fi/LAN** and over a **USB cable** (the app opens by itself when you plug the tablet in).
- Hardware video encoding on the PC (VA-API) and hardware decoding on the tablet; ~45–60 ms from PC to tablet
  on the reference setup with the defaults, less with the [latency options](#latency-tuning).
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
`systemctl --user enable --now slated`, which also starts the tray icon).
Building the desktop app needs `qt6-declarative`, `kwindowsystem`, `kirigami` and `kirigami-addons`.

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

Open **Plasma Slate** from the app launcher or click its tray icon for the window: connected tablets with live
latency and bitrate, resolution and refresh rate, disconnecting and forgetting tablets, and this PC's fingerprint.

### Resolution and refresh rate (performance)

Lowering the stream resolution or frame rate reduces work everywhere: KWin composites a smaller screen, the
GPU encodes fewer pixels and frames, the network carries less and the tablet decodes faster (which also lowers
latency). The tablet scales the picture up to its panel; the aspect ratio is always kept and the stream is never
larger than the tablet's own resolution.

- **System Settings:** *Display & Monitor* lists the tablet screen's resolutions (native, 83/75/67/50 %) and
  refresh rates (the tablet's own, 60, 45, 30 Hz) like any other monitor. Picking one there is applied to the
  stream.
- **Plasma Slate window:** *Resolution* (the same presets, or Custom…) and *Refresh rate* for each connected tablet.

Either way the tablet reconnects within a couple of seconds with the new settings, and they are remembered for
that tablet.
- **CLI:** `slated stream <device-id> --resolution 1280x800 --fps 30` (`slated devices` lists ids; without
  options it prints the current settings).
- **Defaults for all tablets:** `resolution` / `fps` in `slated.toml`.

When the resolution changes, the tablet keeps its position and gets a fresh automatic scale.

### Latency tuning

Where the time goes on the reference setup (1920×1200 HEVC, 60 fps): KWin composites the frame (0–16 ms
after the change, averaging ~8), the GPU encodes it (~8–11 ms at 1920×1200, ~6 ms at 1280×800, independent of
the encoder preset), the network carries it (2–10 ms), the tablet's hardware decoder **holds it until two more
frames have arrived** (~33 ms at 60 fps — the largest part), then decodes it (~4 ms) and the panel shows it at
its next refresh (0–16 ms). The *latency* figure in the app and the Plasma Slate window is capture → decoder
output, so it leaves out the first and last item.

Everything below is optional and off by default unless stated. The Plasma Slate window shows encode and decode
time separately so the effect of each can be seen.

**On the PC (`slated.toml`, restart the service after editing):**

| Key | Default | Effect |
|---|---|---|
| `filler_frames` | `0` | Repeats of the last picture sent between real frames (1–3). Decoders that hold frames release them when more input arrives: with `1` the hold drops from ~33 ms to ~17 ms at 60 fps, with `3` to ~8 ms. The tablet decodes but does not show them. The PC encodes `fps × (1 + filler_frames)` frames per second; `slated doctor` prints the encoder's speed (≈300 fps at 1920×1200 on an RX 9070 XT, enough for 3). |
| `rate_control` | `"auto"` | `cbr`: every frame padded to bitrate/fps — constant, predictable bandwidth (the Mesa/AMD encoder pads even a static screen to ~37 KB per frame at 19 Mbit/s). `vbr`: frames as small as the content allows (a repeat is ~50 bytes), `bitrate` is the ceiling. `auto` = `cbr`, or `vbr` when `filler_frames` > 0. |
| `cpb_frames` | `0` | Encoder buffer in frames. `1`–`2` caps every frame near bitrate/fps so a keyframe or a big change never takes several frame-times to transfer, at the price of a blurrier first frame after one (it sharpens over the next frames). |
| `keyframe_interval_secs` | `0` | `0` = keyframes only when the tablet asks (frames travel over TCP, nothing gets lost); periodic keyframes only add size spikes. |
| `target_usage` | `0` | Encoder preset 1–7; `0` picks the fastest for the GPU. The VA-API scale is 1 = quality … 7 = speed on Intel, but Mesa/AMD runs its presets the other way round (1–3 ≈ 300 fps, 6–7 ≈ 60 fps at 1920×1200). This changes throughput (the budget for fillers), not the per-frame encode time. |
| `aud` | `false` | Access-unit delimiters in the stream; a few decoders detect frame ends faster with them. |
| `resolution` / `fps` | native / panel | Still the biggest single lever on the PC side: encode time scales with pixels. Lowering `fps` makes the decoder hold *longer* (it holds frames, not milliseconds), so prefer lowering the resolution. |

**On the tablet (*Latency options* on the start screen):**

- **Decoder: Software** — the platform decoder (`c2.android.hevc.decoder`) outputs every frame as soon as it
  is decoded, so the two-frame hold disappears entirely. It needs CPU: set a lower stream resolution
  (1280×800 or 50 %) and expect more battery use. This is the shortest path to under 40 ms on a tablet whose
  hardware decoder holds frames.
- **Vendor low-latency decoder mode** — the SoC vendor's pre-Android-11 low-latency switch (Qualcomm,
  Samsung Exynos, MediaTek, Amlogic, HiSilicon). *Automatic* enables it except on HiSilicon/Kirin, where it
  was measured to make things worse.
- **Mark the end of each frame** — appends an access-unit delimiter to every frame so the decoder knows the
  frame is complete without waiting for the next one. Helps some decoders, is ignored by others.
- **Use the screen's highest refresh rate** (on) — on a 90/120 Hz tablet the stream runs at that rate and a
  decoder that holds two frames holds them for two shorter intervals.

Always on: the network and decoder threads run at display priority, received frames go straight into the
decoder from the network thread, filler frames are never rendered, and `KEY_LOW_LATENCY` is set on Android 11+.

Measured on the reference tablet (MatePad T 10s, Kirin 710A; details in [docs/TESTING.md](docs/TESTING.md)):
the hardware decoder gives ~55 ms at 1920×1200 and is *not* helped by filler frames or AUDs (its hold is
not input-driven, and it holds longer at lower resolutions). *Decoder: Software* with the stream at **60 %
(1152×720)** gives **~20 ms at 60 fps**; at 1280×800 ~45 ms, at 1440×900 ~34 ms but only 30 fps (CPU
limit). So on that class of tablet: software decoder + 60 % resolution. Filler frames are for tablets whose
hardware decoder does release frames on new input; check with `scripts/dev-latency-matrix.sh`.

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
| `filler_frames`, `rate_control`, `cpb_frames`, `keyframe_interval_secs`, `target_usage`, `aud` | see above | [Latency tuning](#latency-tuning) |

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
| `tray/` | Qt6/KF6 desktop app: tray icon and Kirigami window |
| `packaging/` | `.desktop` files, systemd user unit, PKGBUILD |
| `docs/` | plan, ADRs, spike findings, test checklist |
| `spikes/` | M0 experiments |

## Licence

GPL-3.0-or-later. See [LICENSE](LICENSE).
