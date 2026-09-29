#!/usr/bin/env bash
# usage: perf.sh — on-screen frame cadence, read back from the window server: the bundled storybook
# under a synthetic trackpad scroll train on its long buttons page, and the bundled Finder demo
# under a right-edge resize drag. Prints unique frames per second over the moving window; fails
# under FPS_MIN_SCROLL (100) and FPS_MIN_RESIZE (60). Needs an awake, unlocked session; void (exit
# 90) otherwise.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

./scripts/lockguard.sh >/dev/null || exit 90
finder=$(./scripts/bundle.sh finder_demo | tail -1) || exit 1
storybook=$(./scripts/bundle.sh storybook | tail -1) || exit 1

tools=/tmp/ng-perf-tools
mkdir -p "$tools"
for tool in ngtrain2 npcap2; do
  src=.wip/tools/perf/$tool.swift
  [ "$tools/$tool" -nt "$src" ] || swiftc -O -suppress-warnings -o "$tools/$tool" "$src" || exit 1
done
[ "$tools/ngdrag" -nt scripts/ngdrag.swift ] || swiftc -O -suppress-warnings -o "$tools/ngdrag" scripts/ngdrag.swift || exit 1

launch() { # <app> <process name> [env...]
  local app=$1 name=$2 before pid; shift 2
  before=$(pgrep -x "$name" | tr '\n' ' ')
  open -n "$app" "$@" || return 1
  for _ in $(seq 1 60); do
    sleep 0.25
    for p in $(pgrep -x "$name"); do case " $before " in *" $p "*) ;; *) pid=$p ;; esac; done
    [ -n "${pid:-}" ] && [ "$(./scripts/winlist.sh 2>/dev/null | awk -F'\t' -v p="$pid" '$2==p && $3==0' | wc -l | tr -d ' ')" -ge 1 ] && { echo "$pid"; return 0; }
  done
  return 1
}
window_of() { ./scripts/winlist.sh | awk -F'\t' -v p="$1" '$2==p && $3==0 {print $1"\t"$4}' | sort -t, -k3 -n -r | head -1; }
# The window server captures a window hanging off the display as one frozen image.
place() { osascript -e "tell application \"System Events\" to set position of window 1 of process \"$1\" to {100, 140}" >/dev/null 2>&1; sleep 0.5; }

unique_fps() { # <cap> <from> <to>
  python3 - "$@" <<'PY'
import re, sys
rows = []
for line in open(sys.argv[1]):
    m = re.match(r"F t=([\d.]+) s=(-?\d+) h=(\S+)", line)
    if m: rows.append((float(m[1]), m[3]))
a, b = float(sys.argv[2]), float(sys.argv[3])
t0 = rows[0][0]
win = [r for r in rows if a <= r[0] - t0 <= b]
uniq = sum(1 for x, y in zip(win, win[1:]) if y[1] != x[1] and y[1] != "-")
print(f"{uniq / (b - a):.1f}")
PY
}

fail=0
probe() { # <name> <fps> <min>
  if python3 -c "import sys; sys.exit(0 if float('$2') >= $3 else 1)"; then
    printf 'ok    %-8s %s unique frames/s (min %s)\n' "$1" "$2" "$3"
  else
    printf 'FAIL  %-8s %s unique frames/s (min %s)\n' "$1" "$2" "$3"; fail=1
  fi
}

# A trackpad-rate scroll train: 4 px per event at 125 Hz for 1.5 s.
pid=$(launch "$storybook" Storybook --env NG_PAGE=buttons) || { echo "no storybook window"; exit 1; }
sleep 1.5; place Storybook
win=$(window_of "$pid"); wid=${win%%	*}; IFS=, read -r wx wy ww wh <<<"${win#*	}"
cap=/tmp/ng-perf-scroll.txt; rm -f "$cap"
NPCAP_HZ=125 "$tools/npcap2" "$wid" 3.2 "$cap" 600 300 1400 1000 0 0 1280 80 >/dev/null 2>&1 &
cappid=$!; sleep 0.8
"$tools/ngtrain2" "$pid" $((wx + 650)) $((wy + 400)) 4 125 1.5 100000 >/dev/null 2>&1
wait $cappid; kill "$pid" 2>/dev/null
probe scroll "$(unique_fps "$cap" 0.9 2.2)" "${FPS_MIN_SCROLL:-100}"

# The demo autosaves its frame, so each resize run would otherwise start 200 pt wider.
defaults delete dev.cafekery.nativegpui.finder_demo "NSWindow Frame ng.finder.window" >/dev/null 2>&1
pid=$(launch "$finder" "Found'er") || { echo "no finder window"; exit 1; }
sleep 1.5; place "Found'er"
win=$(window_of "$pid"); wid=${win%%	*}; IFS=, read -r wx wy ww wh <<<"${win#*	}"
cap=/tmp/ng-perf-resize.txt; rm -f "$cap"
NPCAP_HZ=120 "$tools/npcap2" "$wid" 3 "$cap" 0 0 $(((ww + 200) * 2)) $((wh * 2)) 0 0 1200 80 >/dev/null 2>&1 &
cappid=$!; sleep 0.8
"$tools/ngdrag" $((wx + ww - 2)) $((wy + 20)) $((wx + ww + 198)) $((wy + 20)) 120 10 >/dev/null 2>&1
wait $cappid; kill "$pid" 2>/dev/null
probe resize "$(unique_fps "$cap" 0.9 1.9)" "${FPS_MIN_RESIZE:-60}"

exit $fail
