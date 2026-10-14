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
