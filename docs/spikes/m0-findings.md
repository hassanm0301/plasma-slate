# M0 spike findings

Environment: Arch Linux, Plasma/KWin **6.7.5** (Wayland), Radeon RX 9070 XT (Mesa 26.2.4, radeonsi VA-API),
GStreamer 1.28.8, libei 1.6.0, Huawei MatePad T 10s (AGS3-L09, Android 10 / API 29, 1200×1920 portrait-native,
224 ppi, 60 Hz, HiSilicon `OMX.hisi.video.decoder.avc` / `.hevc`).

Spike code lives in `spikes/`; helper scripts in `scripts/`.

## S1 — Virtual output (`spikes/s1-virtual-output`) — PASS, with caveats

- `zkde_screencast_unstable_v1` v6 is available. KWin only advertises it to executables that have an installed
  `.desktop` file (matching `Exec=`) with `X-KDE-Wayland-Interfaces=zkde_screencast_unstable_v1`. Without it the
  global is simply absent (`scripts/install-dev-desktop-files.sh` handles this for dev builds).
- `stream_virtual_output_with_description(name, description, w, h, scale, pointer)` creates an output called
  `Virtual-<name>`. It appears in `kscreen-doctor -o` / Display & Monitor, gets a Plasma panel, and is placed to
  the right of existing outputs. A PipeWire node + object serial are delivered (`serial` then `created` events).
- **`width`/`height` are treated as the mode size in pixels** (a 60 Hz preferred mode of exactly that size is
  created). The `scale` argument is **ignored** on 6.7 — scale comes from KWin's saved config (or 1.0).
  → The daemon passes the tablet's *physical* pixel size and sets scale itself via output management.
- KWin also adds a spurious `1920×1200@29.95` custom mode to virtual outputs and on first creation it was the
  *current* mode. → The daemon must explicitly select the 60 Hz mode after creation.
- Persistence: position, scale and mode are saved in `~/.config/kwinoutputconfig.json` and restored when an output
  with the **same connector name** is recreated. When no entry exists for the name, KWin **reuses another virtual
  output's entry** (same UUID, connector name overwritten). Two simultaneous virtual outputs get distinct UUIDs.
  → Per-tablet placement must be owned by the daemon (stored per device ID, re-applied via
  `kde_output_management_v2`), not left to KWin's matching. (Risk "position not remembered" → mitigated.)
- Output disappears immediately when the stream is closed or the client disconnects (no orphans from crashes).

## S2 — Capture + encode — PASS

- `pipewiresrc target-object=<serial>` negotiates `video/x-raw(memory:DMABuf)` (AR24, modifier) directly from KWin.
  `vapostproc → vah264enc` keeps the frame on the GPU (zero copy).
- Frames are **damage-driven**: an idle output produces no frames. A moving 60 fps window yields ~59 fps captured.
  → Encoder must handle irregular frame timing; the client must not assume constant frame rate; send a keyframe
  on connect even if the screen is idle (repeat last frame).
- Encoder latency at 1920×1200 NV12 (GStreamer latency tracer, 300 frames):
  - `vah264enc` (CBR 20 Mb/s, no B-frames, target-usage=7): **avg 3.4 ms, max 6.6 ms**
  - `vah265enc` (CBR 15 Mb/s): avg 10.5 ms
  → H.264 is the default codec; HEVC optional.
- Required packages: `gst-plugin-va`, `gst-plugin-pipewire`, `gst-plugins-bad`.

## S3 — Touch via KWin EIS (`spikes/s3-touch`) — PASS

- `org.kde.KWin.EIS.RemoteDesktop.connectToEIS(capabilities)` (restricted D-Bus interface, same `.desktop`
  mechanism via `X-KDE-DBUS-Restricted-Interfaces`) returns a libei socket fd. Capability mask: `1` keyboard,
  `2` pointer, `4` touch.
- With `4`, KWin creates one "absolute device" with the touch capability and **one region per output**, each region
  carrying `mapping_id` = output name and the output's logical geometry. Coordinates are global logical pixels.
- Results on the virtual output: touch-drag of a titlebar moved the window by exactly the dragged delta; a tap on the
  close button closed the window; **the mouse cursor did not move** (1865,1027 before and after).
- Rust: `reis` 0.7 (pure Rust libei) + `zbus` 5 work; no C dependency needed.
- uinput fallback not needed.

## S4 — Android decode

Folded into M2: the first Android build *is* the decode test (MediaCodec async → SurfaceView with per-frame timing).
See `docs/spikes/s4-android-decode.md` once measured.
