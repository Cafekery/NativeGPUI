#!/usr/bin/env bash
# Abort if the display is locked or asleep. A capture taken then returns a shield window or stale
# black and still produces plausible numbers, which is the whole hazard. Exit 90 so a harness can
# tell "blocked" from "failed". Call before AND after anything that produces a number.
#
# `loginwindow` is ALWAYS a running process, so its presence means nothing — the signal is it being
# FRONTMOST. A first version of this file aborted every time for exactly that reason.
#
# LOCKGUARD_FAKE_SHOT points the black-screen arm at a file instead of the screen, so each arm can
# be proven able to fire without waiting for a real lock.
set -uo pipefail

front=$(osascript -e 'tell application "System Events" to get name of first application process whose frontmost is true' 2>/dev/null || echo unknown)
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
