#!/usr/bin/env bash
# Dev experiment: measure tablet-side decode and capture-to-decode latency for codec/decoder
# variants. Needs a running slated (paired or --auto-approve), the tablet on adb, and a moving
# test pattern on the virtual output.
#   scripts/dev-latency-matrix.sh <server-ip> ["h264:true h264:false hevc:true hevc:false"]
set -euo pipefail
cd "$(dirname "$0")/.."
. scripts/dev-env.sh
HOST="${1:?server ip (or 127.0.0.1 for USB)}"
VARIANTS="${2:-h264:true h264:false hevc:true hevc:false}"
PKG=io.github.hassanm0301.plasmaslate
pkill -f "window_title slate-matrix" 2>/dev/null || true
(setsid ffplay -loglevel error -window_title slate-matrix -f lavfi "testsrc2=size=960x600:rate=60" >/dev/null 2>&1 &)
for v in $VARIANTS; do
  codec="${v%%:*}"; vendor="${v##*:}"
  adb shell am force-stop $PKG
  sleep 1
  adb logcat -c
  adb shell am start -n $PKG/.MainActivity --es connect_host "$HOST" --ez connect_usb "$([ "$HOST" = 127.0.0.1 ] && echo true || echo false)" \
    --es debug_codec "$codec" --ez debug_vendor_low_latency "$vendor" >/dev/null
  sleep 4
  out=$(kscreen-doctor -o | sed 's/\x1b\[[0-9;]*m//g' | grep -oP 'Virtual-PlasmaSlate-\w+' | head -1 || true)
  [ -n "$out" ] && scripts/kwin-send-window-to-output.sh slate-matrix "$out" >/dev/null
  sleep "${SECS:-30}"
  adb logcat -d -s VideoDecoder:I | grep -oP 'perf .*' | tail -12 | python3 -c '
import sys,re,statistics as st
rows=[dict(re.findall(r"(\w+)=([\w./:-]+)",l)) for l in sys.stdin]
rows=[r for r in rows if float(r.get("fps",0))>30]
if not rows: print("'"$v"': no samples"); sys.exit()
d=[float(r["decode_ms"]) for r in rows]; l=[float(r["latency_ms"]) for r in rows]; f=[float(r["fps"]) for r in rows]
print("'"$v"': fps~%.0f decode_ms median %.1f latency_ms median %.1f (n=%d)" % (st.median(f), st.median(d), st.median(l), len(rows)))' || true
done
adb shell am force-stop $PKG
pkill -f "window_title slate-matrix" || true
