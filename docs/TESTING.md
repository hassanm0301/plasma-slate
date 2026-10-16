# Testing

## Automated

| Suite | Command | Covers |
|---|---|---|
| Rust unit + session e2e | `cd server && cargo test` | framing, touch mapping, TLS pinning, identity, store, config, codec/stream planning, adaptive bitrate, full session (pairing → video → touch → reconnect with token → PC-side disconnect) with mocked KWin/encoder |
| Rust lint | `cargo fmt --check && cargo clippy --all-targets -- -D warnings` | style, `unwrap()` outside tests |
| Android unit | `cd android && ./gradlew testDebugUnitTest lintDebug` | framing interop, touch mapper, NAL parsing, endpoint parsing |
| Desktop app | `cmake -S tray -B tray/build && cmake --build tray/build` | builds with `-Werror` |
| Desktop controls tests | `QT_QPA_PLATFORM=offscreen ctest --test-dir tray/build --output-on-failure` | default-off settings, independent toggles, saving/restoring the selected decoration |
| Window decoration | `cmake -S decoration -B decoration/build -DPLASMA_SLATE_DECORATION_TESTS=ON && cmake --build decoration/build && QT_QPA_PLATFORM=offscreen ctest --test-dir decoration/build --output-on-failure` | plugin loading, output transitions, touch layout, button actions and delayed decoration state |
| Window return script | `node --test decoration/move-window.test.cjs` | UUID targeting, previous-monitor/fallback selection and safe no-op cases |
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
| Tablet: filler skipping, software decoder, trailing AUD, vendor keys, highest refresh rate | unit-tested (AUD bytes, filler flag); measured on the MatePad below |

Tablet measurements (MatePad T 10s over USB, `scripts/dev-latency-matrix.sh`, 60 fps moving test pattern,
median of ~12 one-second samples; latency = capture on the PC → decoder output on the tablet):

| Stream | Decoder | fps | decode ms | latency ms |
|---|---|---|---|---|
| 1920×1200 | hardware (baseline, new encoder preset) | 60 | 44.5 | **54.5** |
| 1920×1200 | hardware + trailing AUD | 61 | 78.2 | 88.1 (worse) |
| 1920×1200, `filler_frames = 2` | hardware | 61 | 53.6 | 75.3 (worse) |
| 1280×800 | hardware | 60 | 52.1 | 64.1 (worse than native!) |
| 1280×800 | software | 61 | 18.2 | 43.7 |
| 1280×800, `filler_frames = 2` | software | 38 | 75.5 | 99.0 (CPU saturated) |
| 1280×800, `cpb_frames = 1` | software | 61 | 19.0 | 50.8 |
| 1280×800 @ 45 fps | software | 46 | 17.0 | 49.1 |
| 1440×900 | software | 31 | 26.9 | 33.7 (decoder caps at ~30 fps) |
| 1600×1000 | software | <30 | — | no samples |
| **1152×720 (60 %)** | **software** | **60** | **15.6** | **20.5** |
| 960×600 | software | 60 | 15.5 | 21.3 |
| 1152×720, `num_slices = 4` | software | 60 | 17.5 | 22.8 (no gain) |
| 1152×720, `num_slices = 4`, `bitrate_kbps = 4000` | software | 60 | 14.8 | 37.0 (noise; no gain) |
| 1440×900, H.264 | software (`c2.android.avc.decoder`) | 60 | 64.3 | 82.0 (holds ~3 frames) |

Making the stream "easier to decode" on the PC side was tried and does not move the software decoder: slices
(parallel decode) and a much lower bitrate (less entropy decoding) leave decode at ~15 ms at 1152×720, so
that time is per-pixel work in the decoder. H.264 for the software path is worse: Android's software AVC
decoder buffers ~3 frames even with `num_reorder_frames = 0` in the SPS. The only effective lever is pixels.

Conclusions for this tablet: the HiSilicon hardware decoder's frame hold is not shortened by filler frames or
AUDs (both make it slower), and it holds *longer* at lower resolutions. The software decoder has no hold and
keeps 60 fps up to ~1152×720 on the Kirin 710A, where the whole path is ~20 ms. PC-side encode is 2 ms for
real content at any of these sizes (`encode_ms` in `stream stats`). The reference tablet is now set to 60 %
with the software decoder.

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
8. PC lock and sleep with the tablet connected: lock, let the monitors turn off, suspend, wake, unlock. The
   password prompt is on the main monitor, both screens keep their resolution, and the tablet returns to its
   place. With a monitor that disconnects in standby the log shows `the last monitor went away` and, after
   waking, `monitor connected`; there is no `remembered new placement … x=0 y=0`.

### Optional window controls (hardware validation required)

Software checks on 2026-10-15: the Release plugin loaded in a separate headless KWin 6.7.5 session with
two virtual desktop monitors and a screencast-created `Virtual-PlasmaSlate-test` output. A real Qt window's
titlebar changed from 32 to 56 logical pixels on Slate and back to 32 on the desktop, both restored and
maximized. An EIS synthetic touch on the native monitor button returned the window to its previous output;
the return script also targeted a maximized window by UUID and preserved its maximization. A native KWin
screenshot confirmed the large controls and monitor button render correctly. The production settings
controller enabled both options, changed them independently, and the disable CLI restored Breeze and
cleared the saved decoration backup in this same isolated setup.
The session used temporary XDG directories and its own D-Bus and PipeWire instances; the live desktop was
not changed. Physical tablet touch and the hardware scenarios below still need manual validation.

Use a normal window with a system titlebar, such as Dolphin. Record the current window decoration in
System Settings before starting. These checks change the session's selected decoration; finish with both
options off.

1. Fresh settings: confirm **Touch-sized titlebar buttons** and **Move to desktop monitor button** are both
   off and the existing decoration stays selected. Installing or restarting Plasma Slate must not enable them.
   For a first local install, enabling before logging out/in should report that KWin cannot load the plugin;
   after logging back in it should work. System-wide installs should work without this login step.
2. Enable only touch-sized buttons. Move the window from a desktop monitor to Plasma Slate, between Slate
   screens if available, and back. Confirm large, separated controls only on Slate and compact controls on
   desktop monitors. Tap minimize, maximize, restore and close on the tablet; confirm correct actions and
   visible press feedback. Check restored and maximized windows, light/dark colors, and different screen scales.
3. Disable touch-sized buttons and enable only the move button. Confirm compact controls and a move button
   only on Slate. Move the window from desktop monitor A to Slate and tap the button: it returns to A.
   Repeat from desktop monitor B. Check normal and maximized windows and windows first opened on Slate.
4. Enable both options and repeat the movement and tapping checks. Disabling either one must preserve the
   other. Dragging the titlebar and resizing the window should still work around the controls.
5. With a window on Slate, disconnect its previous desktop monitor. The move button must use another desktop
   monitor. With no desktop monitor connected, the window must stay on the tablet. Reconnect a monitor and
   check again.
6. Change a connected tablet's scale or resolution and reconnect it. Controls must continue to follow the
   window's current screen. Close a Slate connection while a window is on it and confirm KWin handles the
   window normally on a remaining screen.
7. Restart the tray and log out/in with each option enabled. Confirm choices persist and apply when Slate
   connects. Turn both off: the original decoration must return. Enable again, choose a different decoration
   in System Settings, and turn both off: the newly selected decoration must remain selected.
8. Check an application with its own titlebar, then enable its system-titlebar option if offered. Only the
   system titlebar gets Plasma Slate controls. Fullscreen and borderless windows should remain usable.
9. Run `plasma-slate-tray --disable-window-controls` with the feature enabled and confirm both options become
   off and the saved decoration returns. For a local install, verify `scripts/install-local.sh --uninstall`
   restores the decoration before removing the plugin. Updating an active plugin must use a replacement
   inode; log out/in afterwards to test the updated plugin.
