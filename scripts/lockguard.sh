#!/usr/bin/env bash
# usage: lockguard.sh — abort with exit 90 if the display is locked or asleep.
#   env: LOCKGUARD_FAKE_SHOT=<png> reads that file instead of capturing the screen
set -uo pipefail

front=$(osascript -e 'tell application "System Events" to get name of first application process whose frontmost is true' 2>/dev/null || echo unknown)
# loginwindow always runs; only its being frontmost means locked.
if [ "$front" = loginwindow ]; then
  echo "LOCKGUARD ABORT: loginwindow is frontmost — screen locked" >&2
  exit 90
fi

if pgrep -qx ScreenSaverEngine 2>/dev/null; then
  echo "LOCKGUARD ABORT: ScreenSaverEngine running" >&2
  exit 90
fi

shot=${LOCKGUARD_FAKE_SHOT:-}
tmp=
if [ -z "$shot" ]; then
  tmp=$(mktemp -t lockguard).png
  screencapture -x -t png "$tmp" 2>/dev/null
  shot=$tmp
fi
mean=$(python3 - "$shot" <<'PY'
import sys
try:
    from PIL import Image
    import numpy as np
    print(f"{np.asarray(Image.open(sys.argv[1]).convert('RGB')).astype(float).mean():.2f}")
except Exception:
    print("-1")
PY
)
[ -n "$tmp" ] && rm -f "$tmp"

if [ "$mean" != "-1" ] && python3 -c "import sys; sys.exit(0 if float('$mean') < 1.0 else 1)"; then
  echo "LOCKGUARD ABORT: screen captures black (mean $mean) — asleep or shielded" >&2
  exit 90
fi

echo "LOCKGUARD OK: awake, frontmost=$front, screen mean $mean"
