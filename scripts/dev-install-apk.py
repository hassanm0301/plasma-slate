#!/usr/bin/env python3
"""Dev helper: `adb install -r` an APK and click through EMUI/MIUI style confirmation dialogs
("CONTINUE", "INSTALL", "OK") that some vendors show for adb installs.

Usage: scripts/dev-install-apk.py [path/to/app.apk]
"""
import re
import subprocess
import sys
import time

apk = sys.argv[1] if len(sys.argv) > 1 else "android/app/build/outputs/apk/debug/app-debug.apk"
BUTTONS = ["CONTINUE", "INSTALL", "INSTALL ANYWAY", "OK", "UPDATE"]


def adb(*args, **kw):
    return subprocess.run(["adb", *args], capture_output=True, text=True, **kw)


def find_button():
    adb("shell", "uiautomator", "dump", "/sdcard/slate-ui.xml")
    xml = adb("exec-out", "cat", "/sdcard/slate-ui.xml").stdout
    for node in re.finditer(r'<node [^>]*?text="([^"]*)"[^>]*?clickable="true"[^>]*?bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"', xml):
        text = node.group(1).strip().upper()
        if text in BUTTONS:
            x1, y1, x2, y2 = map(int, node.group(2, 3, 4, 5))
            return text, (x1 + x2) // 2, (y1 + y2) // 2
    return None


proc = subprocess.Popen(["adb", "install", "-r", apk], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
deadline = time.time() + 120
while proc.poll() is None and time.time() < deadline:
    time.sleep(1.5)
    btn = find_button()
    if btn:
        print(f"tapping {btn[0]}")
        adb("shell", "input", "tap", str(btn[1]), str(btn[2]))
out = proc.communicate(timeout=30)[0]
print(out.strip().splitlines()[-1] if out.strip() else "no output")
sys.exit(0 if "Success" in out else 1)
