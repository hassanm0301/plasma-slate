# S4 — Android decode latency (MatePad T 10s, Kirin 710A)

Setup: slated on Plasma 6.7.5 / RX 9070 XT (VA-API), tablet on 5 GHz Wi-Fi (390 Mb/s link, PC wired),
1920×1200 @ 60 fps desktop with a 60 fps test pattern on the virtual screen.
Tool: `scripts/dev-latency-matrix.sh` (tablet logs per-second `perf` lines; latency = capture on the PC →
decoder output on the tablet, clocks aligned with Ping/Pong offset estimation).

| Variant | Decode (receive → output) | Capture → output | Notes |
|---|---|---|---|
| H.264, HiSilicon low-latency vendor keys | 155–190 ms | 160–195 ms | decoder holds ~9–10 frames internally; our input queue stays at 1–2 |
| H.264, no vendor keys | ~189 ms | ~193 ms | same |
| HEVC, vendor keys | ~80 ms | ~85 ms | keys *hurt* on this firmware |
| **HEVC, no vendor keys** | **~38 ms** | **~43 ms** | steady 60 fps |

Network RTT ≈ 5–20 ms (Wi-Fi power-save adds jitter), PC encode ≈ 3.4 ms (H.264) / 10.5 ms (HEVC).

The H.264 SPS already signals `max_num_reorder_frames = 0`, `max_dec_frame_buffering = 2`; the HiSilicon AVC
decoder ignores it. Codec config on the device also shows the HEVC decoder is ~2.5× faster at 1080p
(`measured-frame-rate-1920x1080` 163–360 vs AVC 64–140).

## Decisions

- The client lists decoders lowest-latency first (hardware HEVC first); the server's default `codec = "auto"`
  follows that order. `codec = "h264"`/`"hevc"` in `slated.toml` overrides it.
- HiSilicon vendor low-latency keys are off by default (developer extra `debug_vendor_low_latency`).
- The decoder flushes and resyncs on a persistent input backlog, so a slow decoder can't accumulate latency.
- Adaptive bitrate waits 5 s after stream start (the first keyframe and decoder start look like congestion).

## Follow-up: idle screens (2-hour soak, 2026-10-14)

Capture is damage-driven, so a mostly static screen produced few frames. The HiSilicon decoder releases a frame
only after receiving more input (~2 frames held). At 60 fps that's ~35 ms; at 10 fps it was 200+ ms, so a click
or keystroke appeared late. The adaptive bitrate then read that latency as congestion and fell to its floor.

Fixes:
- The encoder keeps a steady cadence: `pipewiresrc keepalive-time ≈ 1000/fps + 4 ms` re-sends the last frame
  when nothing changed. On a real (static) desktop image repeated HEVC P-frames are ~57 bytes.
- Bitrate adaptation uses only network signals (send-queue drops, RTT far above its floor), not display
  latency.

Result: ~55–65 ms with a live-updating window (System Monitor graphs) where it had been ~220 ms.

## Follow-up: encoder side and filler frames (2026-10-15)

Measured on the PC while looking for the rest of the ~50 ms:

- **`target-usage` is inverted on Mesa/AMD.** `vah265enc` at 1920×1200: target-usage 1–3 ≈ 300 fps,
  4–5 ≈ 150 fps, 6–7 ≈ 60 fps (H.264: 280 / 180 / 180). We ran with 7, so the HEVC encoder was saturated at
  60 fps. `slated` now defaults to 1 on AMD (`/sys/class/drm/renderD128/device/vendor` = 0x1002) and 7
  elsewhere (`target_usage` in `slated.toml` overrides).
- **Per-frame encode time does not depend on the preset**: the GStreamer latency tracer puts the `enc`
  element at ~10 ms per frame at 1920×1200 (~6 ms at 1280×800) with either preset. The preset buys
  throughput, i.e. headroom for filler frames, not latency.
- **Mesa's CBR pads every frame to bitrate/fps**: a static screen costs ~37 KB per frame at 19 Mbit/s.
  VBR encodes a repeat in ~50 bytes. The "~57-byte repeats" noted above were measured on an earlier Mesa.
- **Filler frames** (`filler_frames = N`): `pipewiresrc keepalive-time` is set to the frame interval divided
  by N + 1, so repeats of the last picture arrive between real frames as well as when the screen is idle. A
  repeat is a `gst_buffer_copy` of a copy, so it carries one more `ParentBufferMeta` than a real buffer; the
  probe flags it, the output matcher maps encoder output back to inputs by timestamp spacing, and the frame is
  sent with `VideoFrame.filler = true`. The tablet decodes it (which pushes the held real frame out) but
  releases the output buffer without rendering. Verified with the CLI client: 880/900 flagged on a static
  screen, 110 KB total with VBR.

Measured on the MatePad the same day (table in `docs/TESTING.md`): the HiSilicon decoder's hold is **not**
shortened by filler frames (75 ms vs 55 ms baseline — the extra decodes just add load) nor by trailing AUDs
(88 ms), and it holds longer at lower resolutions (64 ms at 1280×800). The software decoder
(`c2.android.hevc.decoder`) has no hold: 20 ms capture → output at 1152×720 @ 60 fps, 44 ms at 1280×800
(CPU limit starts there), 34 ms at 1440×900 but only 30 fps. The reference tablet now runs 60 % + software.
