# ADR 0003 — H.264 via GStreamer VA-API as the default video path

Status: accepted (2026-10-14) · Evidence: `docs/spikes/m0-findings.md` S2

## Decision
Pipeline: `pipewiresrc (DMA-BUF) → vapostproc (NV12) → vah264enc (CBR, no B-frames, low-latency usage,
periodic/forced IDR) → h264parse (byte-stream, AU-aligned) → appsink`. HEVC (`vah265enc`) is negotiable but not
default. Software `x264enc` (tune=zerolatency) is the fallback when no VA encoder exists.

## Why
- 3.4 ms average encode at 1920×1200 on the RX 9070 XT vs 10.5 ms for HEVC.
- The tablet's HiSilicon decoder supports both; H.264 has the most predictable low-latency behaviour on Android.

## Consequences
- Requires `gst-plugin-va`, `gst-plugin-pipewire` at runtime.
- Damage-driven capture → variable frame rate; client treats PTS as informational only.
