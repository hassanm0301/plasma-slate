# Testing

## Automated

| Suite | Command | Covers |
|---|---|---|
| Rust unit + session e2e | `cd server && cargo test` | framing, touch mapping, TLS pinning, identity, store, config, codec/stream planning, adaptive bitrate, full session (pairing → video → touch → reconnect with token → PC-side disconnect) with mocked KWin/encoder |
| Rust lint | `cargo fmt --check && cargo clippy --all-targets -- -D warnings` | style, `unwrap()` outside tests |
| Android unit | `cd android && ./gradlew testDebugUnitTest lintDebug` | framing interop, touch mapper, NAL parsing, endpoint parsing |
| Desktop app | `cmake -S tray -B tray/build && cmake --build tray/build` | builds with `-Werror` |
| System check | `slated doctor` | Wayland, KWin restricted interfaces, EIS touch, encoders, kscreen-doctor, adb |
| Daemon ↔ CLI client | `slated run --auto-approve --port 47421` + `slated client --port 47421 --out x.h264` | real KWin output + VA-API encode + TLS, without a tablet; prints `fillers=` so `filler_frames` can be checked |
| Encoder speed | `slated doctor` | frames per second of the VA encoders at 1920×1200 (the budget for `filler_frames`) |

### Latency options (2026-10-15, PC side verified; tablet side needs the tablet)

Measured on the reference PC with the CLI client (static virtual screen, 1920×1200):

| Check | Result |
|---|---|
| Mesa/AMD `target-usage`: 1–3 ≈ 300 fps, 4–5 ≈ 150 fps, 6–7 ≈ 60 fps (HEVC); H.264 280/180/180 | measured with `imagefreeze`; `target_usage = 0` now picks 1 on AMD (was 7, i.e. the encoder was saturated at 60 fps) |
| Per-frame encode time (GStreamer latency tracer, `enc` element) | ~10 ms at 1920×1200 regardless of preset; ~6 ms at 1280×800 |
| `filler_frames = 2`: repeats detected and flagged | 880 of 900 frames flagged on a static screen; the tablet decodes but does not render them |
| CBR pads static frames to bitrate/fps (37 KB) while VBR repeats are ~50 bytes | `rate_control = "auto"` switches to VBR with fillers: 900 frames in 110 KB |
| Tablet: filler skipping, software decoder, trailing AUD, vendor keys, highest refresh rate | unit-tested (AUD bytes, filler flag); **not yet measured on the MatePad** — run `scripts/dev-latency-matrix.sh` |

## Verified on hardware (2026-10-14)

Reference setup: Arch, Plasma/KWin 6.7.5, RX 9070 XT, MatePad T 10s (AGS3-L09, Android 10).

| Check | Result |
|---|---|
| Virtual screen created per tablet, listed in Display & Monitor | ✅ `Virtual-PlasmaSlate-<id>`, 1920×1200, scale 1.5 default |
| Position/scale restored for the same tablet | ✅ stored per device, re-applied on connect |
| Picture on the tablet over Wi-Fi | ✅ 60 fps, HEVC, ~45–65 ms capture → decoder output |
| Picture over USB (`adb reverse`), app auto-opened on plug-in | ✅ ~45 ms |
| Touch drag on a titlebar moves the window; tap on close button closes it; mouse cursor unchanged | ✅ (injected `adb input` touches and EIS) |
| Pairing prompt: Deny / timeout → tablet shows "not accepted" | ✅ |
| Pairing prompt: Allow → stream; reconnect uses token, no prompt; same token over USB | ✅ |
| PC-side disconnect (tray / D-Bus) → tablet shows message and closes | ✅ (session test + D-Bus) |
| systemd user service + tray icon with live stats | ✅ |
| Tray icon / launcher opens the Kirigami window (single instance) | ✅ |
| Resolution + refresh rate picked in Display & Monitor settings → stream follows (1440×900 @ 30 Hz and back) | ✅ (session test + live) |
| 2-hour soak over USB (19:38–21:37) | ✅ 743 samples: median 60 fps, latency median 48 ms / p95 59 ms, no errors |
| Live-updating window on the tablet after the cadence fix | ✅ 55–65 ms (was ~220 ms before the fix) |
| GitHub CI (Rust, Android, tray) | ✅ green on `main` |

## Manual checklist (needs a person)

1. Mouse: move the cursor off the edge of the main monitor onto the tablet and back.
2. Finger: tap buttons, scroll a page with two fingers, pinch-zoom in a browser on the tablet screen.
3. Window drag with a finger from the tablet towards the main monitor (KWin moves it to the edge; use the mouse
   or Meta+Shift+←/→ to move it fully across).
4. Rearrange the tablet in Display & Monitor (above/left of the main monitor), disconnect, reconnect. It
   returns to the same place.
5. Wi-Fi drop: toggle Wi-Fi on the tablet. It reconnects within a few seconds without a new prompt.
6. Sleep: lock the tablet for a minute, unlock, open the app. It reconnects.
7. Long session over **Wi-Fi** while actually working (the 2-hour soak was over USB). `journalctl --user -u slated`
   shows `stream stats` every 10 s.
