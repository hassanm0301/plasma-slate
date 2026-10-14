# ADR 0002 — Inject touch through KWin's EIS endpoint

Status: accepted (2026-10-14) · Evidence: `docs/spikes/m0-findings.md` S3

## Decision
The daemon calls `org.kde.KWin.EIS.RemoteDesktop.connectToEIS(4)` and drives the returned libei connection with
the `reis` crate as a sender context. Touch points from the tablet (normalized 0..1) are mapped into the region
whose `mapping_id` equals the tablet's output name, i.e. into the output's *current* global logical geometry,
so rearranging/rescaling the output never breaks touch.

## Why
- Touch is a separate input path in KWin: the mouse cursor does not move (verified).
- Window move by touch and taps work; regions update with layout changes.
- No root/udev setup (unlike uinput), no portal prompt.

## Consequences
- Requires `X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.EIS.RemoteDesktop` in the daemon's `.desktop` file.
- One EIS connection per daemon; a touch "slot" namespace per tablet (touch IDs are offset per session).
