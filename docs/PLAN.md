# Plasma Slate — Project Plan

> Turn an Android tablet into a real, touch-enabled extra monitor for KDE Plasma on Wayland.

Author: Hassan Maudarbocus · Started fresh: 2026-10-14 · Status: planning · Licence: GPL-3.0-or-later

---

## 1. Goals

| # | Goal | What "done" looks like |
|---|------|------------------------|
| G1 | **Real extra monitor** | When the tablet connects, a new output appears in *System Settings → Display & Monitor*, can be dragged to any position, and KWin remembers that position next time the same tablet connects. |
| G2 | **Shared mouse** | The mouse moves seamlessly from the desktop monitor onto the tablet and back, like any physical monitor. |
| G3 | **Independent touch** | Touching the tablet interacts with whatever is under the finger *on the tablet* without moving the mouse cursor, which stays where it was (e.g. on the main monitor). Multi-touch works. |
| G4 | **Windows move freely** | Windows can be dragged between the main monitor and the tablet with the mouse; windows on the tablet can be moved/resized by touch; KWin shortcuts (Meta+Shift+←/→) work. |
| G5 | **Client describes itself** | The Android client tells the server its resolution, physical size/DPI, refresh rate, orientation, touch capabilities and supported codecs; the server builds the output from that. |
| G6 | **Network first, USB later** | v1 works over Wi-Fi/LAN. The transport is abstracted so USB (ADB reverse / AOA) is a later add-on, not a rewrite. |

### Non-goals for v1
- GNOME, Sway, Hyprland, X11 (architecture keeps the door open; see §10).
- Stylus pressure/tilt, audio forwarding, clipboard sync, HDR.
- Internet/remote use outside the LAN.
- iOS client.

### Target hardware (reference setup)
- **Server:** Arch Linux, KDE Plasma 6.8 (Wayland), Ryzen 7 9800X3D, Radeon RX 9070 XT / RX 7900 XT → hardware H.264/HEVC encode via VA-API.
- **Client:** Huawei MatePad T 10s (**AGS3-L09**), 1920×1200 10.1", Kirin 710A, Android 10 / EMUI, no Google Play Services (APK is sideloaded; nothing may depend on GMS).

---

## 2. How it works (architecture)

```
 ┌──────────────────────────── Linux (server) ─────────────────────────────┐
 │                                                                          │
 │  KWin (Wayland compositor)                                               │
 │   ├─ zkde_screencast_unstable_v1.stream_virtual_output ──► virtual output│
 │   │     "PlasmaSlate-<device-id>"  (shows in Display settings)           │
 │   │                         │ PipeWire video node (DMA-BUF)              │
 │   └─ EIS (libei) ◄──────────┼────────── touch injection (absolute, region│
 │                             │           = the tablet output)             │
 │                             ▼                                            │
 │  slated  (Rust daemon, systemd --user)                                   │
 │   ├─ session manager (one session per paired client)                     │
 │   ├─ capture: PipeWire → GStreamer → VA-API H.264/HEVC (low latency)     │
 │   ├─ input: protocol touch events → EIS touchscreen                      │
 │   ├─ discovery: mDNS  _plasmaslate._tcp                                  │
 │   ├─ transport: TLS control channel + video channel                      │
 │   └─ D-Bus API org.plasmaslate.Daemon  ◄── tray / settings UI (later)    │
 └──────────────────────────────────────────────────────────────────────────┘
                         ▲  control + touch          │ video
                         │  (TLS/TCP)                ▼ (TCP in v1 → UDP later)
 ┌──────────────────────────── Android (client) ───────────────────────────┐
 │  Plasma Slate app (Kotlin)                                               │
 │   ├─ discovery (NsdManager) + manual IP                                  │
 │   ├─ ClientHello: display + touch + codec capabilities                   │
 │   ├─ MediaCodec decoder → fullscreen SurfaceView (immersive)             │
 │   └─ MotionEvent → normalized multi-touch events → server                │
 └──────────────────────────────────────────────────────────────────────────┘
```

### Why this design satisfies each goal

- **G1 (real monitor + arrangement):** KWin's restricted Wayland protocol `zkde_screencast_unstable_v1` has `stream_virtual_output(name, size, scale, pointer_mode)`. It creates a compositor-level output (the same mechanism `krfb-virtualmonitor` and Plasma's "Share virtual screen" use). It is a first-class output: it is listed in Display & Monitor and `kscreen-doctor -o`, and can be positioned, scaled and given extra modes. Plasma 6.8 adds **resizing** of virtual screens, which we use for rotation and resolution changes. Giving each tablet a **stable output name** derived from its device ID is how we expect KWin to remember its position (verified in Spike S1).
- **G2 (shared mouse):** Because it is a real output, KWin's normal pointer handling crosses onto it. The stream is requested with pointer mode **embedded** (cursor drawn into frames) in v1; **metadata** mode (client draws the cursor) is an optional later latency win.
- **G3 (independent touch):** On Wayland, touch is a separate input path from the pointer — a touch sequence does not warp the cursor. We inject touch through **KWin's EIS (libei) endpoint** as an absolute touchscreen device whose region is the tablet output's area in the global layout. The mouse cursor stays put. Fallback if EIS touch is unsuitable: a per-client **uinput** virtual touchscreen mapped to the output via KWin's input-device output mapping (Spike S3 decides).
- **G4 (windows):** Mouse drags across outputs are native KWin behaviour. Touch-dragging a titlebar is native KWin touch handling. Note: a finger cannot physically leave the tablet, so moving a window *off* the tablet by touch alone is limited to dragging it to the edge — mouse or shortcuts cover that.
- **G5 (client describes itself):** see the handshake in §4.
- **G6 (transport):** the daemon talks to a `Transport` trait; v1 implements TCP/TLS over LAN, a later milestone implements USB via `adb reverse` (same byte stream, no network) and possibly AOA.

### Restricted KWin interfaces
`zkde_screencast_unstable_v1` and `org.kde.KWin.EIS.RemoteDesktop` are restricted. KWin only grants them to executables declared in an installed `.desktop` file:

```ini
X-KDE-Wayland-Interfaces=zkde_screencast_unstable_v1
X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.EIS.RemoteDesktop
```

The packaging ships this file next to the daemon. Fallback if this path is ever blocked: the public **xdg-desktop-portal** RemoteDesktop + ScreenCast (virtual source) route, which costs one approval prompt (persisted with a restore token) and gives less control over output size/name.

---

## 3. Technology choices

| Area | Choice | Reason |
|---|---|---|
| Server language | **Rust** | Memory-safe network-facing daemon; good crates for every part listed below; strong tooling for machine-enforced rules (clippy, rustfmt, cargo-deny). |
| Wayland client | `wayland-client` + `wayland-scanner` on the XML from `plasma-wayland-protocols` | Generates typed bindings for `zkde_screencast_unstable_v1`. |
| Capture/encode | **GStreamer** (`gstreamer-rs`): `pipewiresrc` → `vapostproc` → `vah264lpenc`/`vah264enc` (and HEVC) → app sink | VA-API hardware encoding on AMD with DMA-BUF zero copy; easy to swap encoders. |
| Input injection | **EIS** via `reis` (Rust libei protocol implementation); fallback `uinput` (`evdev` crate) | KWin supports EIS touch; uinput is a proven fallback. |
| D-Bus | `zbus` | KWin EIS handshake + our own daemon API. |
| Async runtime | `tokio` | Networking, timers, task supervision. |
| Discovery | mDNS via `mdns-sd`, service `_plasmaslate._tcp` | Zero-config on LAN; Android `NsdManager` works without GMS. |
| Security | TLS 1.3 via `rustls`, self-signed certs, pairing code + certificate pinning | LAN-only but still untrusted network; input injection is sensitive. |
| Wire protocol | **Protocol Buffers** (`prost` / `protobuf-javalite`), length-prefixed frames | Versioned schema shared by both sides; `buf breaking` catches incompatible changes in CI. |
| Android client | **Kotlin, native Android** (decided; no Flutter) | The app is essentially a fullscreen `SurfaceView` + `MediaCodec` + `MotionEvent`; native is the shortest, lowest-latency path. Flutter would add a texture/platform-channel layer on the video and touch hot paths for no gain. |
| Android min SDK | **29** (Android 10) | Matches the AGS3-L09. |
| Settings UI (later) | Qt6/QML (Kirigami) app or system-tray applet talking to the daemon over D-Bus | Native KDE look; kept out of the daemon. |
| Packaging | PKGBUILD (Arch), systemd **user** unit, signed APK | Daemon needs the user's Wayland session (`$WAYLAND_DISPLAY`, `$XDG_RUNTIME_DIR`), so it must be a user service. |

---

## 4. Wire protocol (v1)

Two logical channels per session:

1. **Control** (TLS over TCP, `TCP_NODELAY`): handshake, configuration, touch events, keepalive, keyframe requests, stats.
2. **Video**: encoded access units. In v1 it is a second TLS/TCP stream (simplest, good enough on a clean 5 GHz LAN). Milestone M5 moves it to UDP with our own packetization, NACK/keyframe-on-loss and optional FEC, because TCP head-of-line blocking hurts on Wi-Fi.

### Handshake

```
Client                                         Server
  │── ClientHello ────────────────────────────►│
  │     protocol_version, device_id (UUID),     │
  │     device_name, app_version,               │
  │     display { width_px, height_px,          │
  │       physical_width_mm, physical_height_mm,│
  │       density_dpi, refresh_hz, orientation }│
  │     touch { supported, max_points }         │
  │     decoders [ {codec, profile, level,      │
  │       max_width, max_height, max_fps} ]     │
  │                                             │  create/reuse virtual output
  │◄──────────────────────────── ServerHello ───│
  │     session_id, output_name,                │
  │     stream { codec, width, height, fps,     │
  │       bitrate, scale }                      │
  │── Ready ───────────────────────────────────►│
  │◄════════════ video frames ══════════════════│
  │═══ TouchEvent / Keepalive / Stats ═════════►│
```

- **Scale** default = derived from physical size (≈224 ppi on the T 10s → 1.5 or 2.0), user-overridable in Display settings like any monitor.
- **Codec negotiation:** server picks the best common codec (HEVC → H.264), never above the client's decoder limits.
- **Orientation/size change:** `DisplayChanged` message → server resizes the virtual output (Plasma 6.8) and restarts the encoder at the new size.

### Touch events
`TouchEvent { seq, timestamp_us, points[ {id, phase: DOWN|MOVE|UP|CANCEL, x, y} ] }` with `x, y` normalized to `[0,1]` over the video surface. The server maps them into the output's current rectangle in the global layout, so moving the tablet in Display settings never breaks touch.

### Versioning rules
- `protocol_version` is a single integer; server refuses incompatible clients with a clear `Error { code, message }`.
- Fields are only ever added; `buf breaking` runs in CI against `main`.

---

## 5. Repository layout

```
plasma-slate/
├── README.md
├── docs/
│   ├── PLAN.md                ← this file
│   ├── adr/                   ← architecture decision records (one per decision)
│   └── spikes/                ← findings from M0 spikes
├── proto/                     ← .proto files (single source of truth)
├── server/                    ← Rust cargo workspace
│   ├── crates/slate-protocol  ← generated + hand-written framing
│   ├── crates/slate-kwin      ← virtual output + EIS bindings
│   ├── crates/slate-media     ← GStreamer capture/encode
│   ├── crates/slate-net       ← transports, TLS, discovery
│   └── crates/slated          ← daemon binary, session manager, D-Bus API
├── android/                   ← Gradle project, Kotlin app
├── packaging/                 ← .desktop (restricted interfaces), systemd unit, PKGBUILD
└── .github/workflows/         ← CI
```

---

## 6. Milestones

Each milestone ends with something runnable and a short demo note in `docs/`.

### M0 — De-risking spikes (throwaway code under `spikes/`)
The four things that could sink the design are proven first.

| Spike | Question | Pass criteria |
|---|---|---|
| **S1 Virtual output** | Can our own client create a named virtual output via `zkde_screencast_unstable_v1`? Does it appear in Display & Monitor? Is its position remembered across disconnect/reconnect and across logout when the name is the same? Can it be resized (6.8)? | Output listed in Display & Monitor, draggable, position restored on recreate with the same name. Findings on where KWin stores it (`~/.config/kwinoutputconfig.json`). |
| **S2 Capture + encode** | Can we read that output's PipeWire node and encode it on the AMD GPU at 1920×1200@60 with low latency? | `.h264` file plays back; encode time per frame measured (target < 5 ms); no CPU copy if DMA-BUF works. |
| **S3 Touch** | Can we get an EIS connection from KWin, create a touchscreen device and tap/drag on the virtual output **without the cursor moving**? Does touch-dragging a titlebar move a window? Compare with uinput + output mapping. | Tap clicks a button on the virtual output while the cursor stays on the main monitor; decision recorded as an ADR. |
| **S4 Android decode** | Latency of `MediaCodec` H.264/HEVC decode on the AGS3-L09 (vendor low-latency keys, async mode, `SurfaceView`). | Decode latency measured; a test stream from S2 plays smoothly at 60 fps. |

**Exit:** ADRs written for virtual-output method, input method, codec. If S1 or S3 fails, fall back to the portal route (§2) before writing product code.

### M1 — Foundations
- Monorepo skeleton (layout above), licences, README, contributing rules.
- `proto/` v1 schema; generated Rust + Kotlin code.
- CI: rustfmt, clippy `-D warnings`, `cargo test`, `cargo deny`; ktlint/detekt, Android unit tests, `assembleDebug`; `buf lint` + `buf breaking`.
- Daemon skeleton: config file, logging (`tracing`), systemd user unit, mDNS advertising.
- Android skeleton: discovery list + manual IP, connect, handshake, show server reply.

**Exit:** tablet finds the PC, completes the handshake, both sides log the negotiated parameters.

### M2 — Picture on the tablet
- Session manager creates the virtual output from `ClientHello` and tears it down on disconnect.
- PipeWire → VA-API encode → video channel; Android decodes to fullscreen immersive `SurfaceView`, screen kept on.
- Embedded cursor.

**Exit:** a new monitor appears when the tablet connects, can be arranged in Display settings, the mouse moves onto it, windows can be dragged onto it with the mouse, and it disappears cleanly on disconnect. (G1, G2, G4-mouse)

### M3 — Touch
- Android captures multi-touch `MotionEvent`s, maps view → video coordinates (letterboxing-aware), sends `TouchEvent`s.
- Server injects via the method chosen in S3, mapped to the output's live geometry (re-read on layout change).

**Exit:** tap, long-press, scroll, pinch-zoom and titlebar-drag work on the tablet while the cursor stays on the main monitor. (G3, G4-touch)

### M4 — Robustness & lifecycle
- Reconnect after Wi-Fi drop / tablet sleep without losing the output position.
- Rotation and resolution changes via output resize.
- Keyframe request on decoder error; encoder restart on stream renegotiation.
- Multiple tablets at once (one session + output each).
- Clean shutdown of outputs on daemon crash/restart (no orphaned outputs).
- Huawei/EMUI specifics: foreground service, battery-optimisation exemption prompt, `WIFI_MODE_FULL_LOW_LATENCY` lock.

**Exit:** 2-hour soak test with sleep/wake and Wi-Fi toggles, no daemon restart needed.

### M5 — Performance
- Latency instrumentation end-to-end (capture timestamp → present time) shown in a debug overlay on the tablet.
- Video over UDP with packetization, loss detection, NACK / keyframe request; intra-refresh instead of periodic IDR.
- Adaptive bitrate from client stats; frame pacing; skip encoding when the output is idle (damage-driven).
- Optional: pointer metadata mode (client-drawn cursor).

**Targets (1920×1200@60, 5 GHz Wi-Fi):** median glass-to-glass < 50 ms, p99 < 100 ms, 60 fps steady while scrolling, server CPU < 5 %.

### M6 — Security & pairing
- First connection: server shows a 6-digit code (notification), user types it on the tablet; both sides pin each other's certificate.
- Unpaired clients cannot create outputs or inject input.
- Manage/revoke paired devices from the daemon's D-Bus API.

**Exit:** a second unpaired device on the LAN is rejected; captured traffic is encrypted.

### M7 — UX & packaging
- KDE system-tray app (Qt6/QML): status, connected tablets, paired devices, per-device defaults (scale, codec, bitrate).
- Android: settings screen, connection status, "disconnect" gesture/button, remembered servers.
- PKGBUILD for Arch, systemd user unit enabled on install, signed release APK on GitHub Releases.
- User docs: install, firewall ports, troubleshooting.

**Exit:** someone else can install from the README on Arch + a sideloaded APK and get it working.

### M8 — USB transport
- `adb reverse tcp:<port>` path: same protocol over USB, auto-detected when the tablet is plugged in with USB debugging on.
- Evaluate Android Open Accessory (AOA) to avoid needing USB debugging.

**Exit:** works with Wi-Fi off; lower and steadier latency than Wi-Fi.

### Later (backlog)
Stylus pressure/tilt via EIS/uinput tablet tool · clipboard sync · audio · GNOME/Mutter backend via portals · wlroots backend · per-app "send window to tablet" action · HDR/10-bit.

---

## 7. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Virtual output position not remembered across sessions | Breaks G1 | S1 tests it first; if KWin keys virtual outputs differently, the daemon saves and re-applies the position itself via the output management protocol. |
| Restricted KWin interfaces change or get locked down | Daemon cannot create outputs/inject input | Track `plasma-wayland-protocols`/KWin releases; keep the portal fallback path designed; isolate all KWin specifics in `slate-kwin`. |
| EIS touch quirks (gestures, edge triggers) | Degraded G3 | uinput fallback evaluated in S3; both behind an `InputBackend` trait. |
| Wi-Fi latency spikes / power saving on EMUI | Laggy, stuttery picture | UDP video, low-latency Wi-Fi lock, adaptive bitrate, 5 GHz recommendation; USB in M8. |
| Kirin 710A decoder limits / vendor quirks | Can't do 60 fps HEVC | Measure in S4; H.264 fallback; cap resolution/fps per client capabilities. |
| Huawei background-kill behaviour | Session drops | Foreground service + documented battery settings. |
| Touch-dragging windows off the tablet is physically limited | Partial G4 | Mouse + KWin shortcuts; optional "send to main screen" on-screen action later. |

---

## 8. Engineering rules (machine-enforced)

- Rust: `rustfmt --check`, `clippy -D warnings` (pedantic subset), `cargo deny` (licences/advisories), no `unwrap()` outside tests (clippy lint).
- Kotlin: ktlint + detekt with zero-issue baseline; no business logic in `Activity`.
- Protocol: `.proto` is the single source of truth; `buf lint` + `buf breaking` in CI.
- Architecture boundaries: KWin-specific code only in `slate-kwin`; GStreamer only in `slate-media`; daemon depends on traits (`OutputBackend`, `InputBackend`, `Encoder`, `Transport`) so other compositors and USB slot in.
- Every non-trivial decision gets an ADR in `docs/adr/`.
- Conventional Commits; `main` protected, changes via PR with green CI.

---

## 9. Decisions

Settled 2026-10-14:

1. **Android client stack:** native Kotlin (not Flutter).
2. **Daemon language:** Rust.
3. **Licence:** GPL-3.0-or-later (`LICENSE`).
4. **Repository history:** the earlier attempt is replaced; this repo starts fresh.

Still open:

- **Settings UI:** tray app or a proper KCM page inside System Settings (decide at M7).

---

## 10. Beyond KDE (design hooks, not v1 work)

`OutputBackend` and `InputBackend` are traits so that a future GNOME backend (Mutter virtual monitors via the ScreenCast/RemoteDesktop portals) or wlroots backend (headless outputs + virtual pointer/touch protocols) can be added without touching session, media, network or the Android app.

---

## References
- `krfb-virtualmonitor` virtual display on KDE Wayland (usage, `kscreen-doctor` handling): https://www.nite07.com/en/posts/kde-virtual-display-sunshine/
- Plasma 6.8 resizable virtual screens: https://linuxiac.com/kde-plasma-6-8-adds-resizable-virtual-screens-and-larger-emojis/
- KWin handles monitor arrangement on Wayland (Plasma 6): https://phoronix.com/news/KWin-Takes-Over-KScreen-Duties
- Portal "Share virtual screen" creates a virtual output (bug 517296): https://bugs.kde.org/show_bug.cgi?id=517296
- KWin EIS (`org.kde.KWin.EIS.RemoteDesktop`) with touch emulation, in practice: https://github.com/isac322/kwin-mcp
- libei client API: https://libinput.pages.freedesktop.org/libei/api/group__libei.html
