# ADR 0001 — Create the tablet monitor with `zkde_screencast_unstable_v1`

Status: accepted (2026-10-14) · Evidence: `docs/spikes/m0-findings.md` S1/S2

## Decision
The daemon creates one KWin virtual output per connected tablet with
`stream_virtual_output_with_description(name = "PlasmaSlate-<device-short-id>", description = "<device name>",
width/height = tablet physical pixels in the chosen orientation, scale = 1, pointer = embedded)`, and captures the
resulting PipeWire stream. Right after creation it uses `kde_output_management_v2` to select the 60 Hz mode and
apply the tablet's saved position/scale (or computed defaults on first connect). It watches
`kde_output_device_v2` for user changes and persists them per device.

## Why
- Real output: arrangeable in Display & Monitor, mouse crosses naturally, Plasma treats it as a screen.
- Zero-copy DMA-BUF capture straight into VA-API encoding.
- Persistence must be daemon-owned because KWin's matching of virtual outputs is not per-name reliable.

## Consequences
- Requires an installed `.desktop` file with `X-KDE-Wayland-Interfaces=zkde_screencast_unstable_v1`.
- The protocol is "unstable / desktop-environment implementation detail"; all usage is isolated in `slate-kwin`.
- Fallback (not implemented): xdg-desktop-portal ScreenCast virtual source.
