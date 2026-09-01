#!/usr/bin/env bash
# What is actually on the screen, from the window server.
#
# Every other census this project reached for asks about processes YOU started: spawn-and-wait,
# `ps`, `pgrep`, `lsappinfo`. Three agents in one session had all of those come back clean while a
# window sat on another agent's measurement subject. Four process-level checks are one mechanism
# wearing four hats — they answer "what did I start", never "what is on the screen".
#
# The case that motivated this: a probe with a deliberate `panic!()` negative control exits with a
# real status, and macOS then opens a Problem Reporter window owned by a DIFFERENT process. Nothing
# that asks about your own pid can see it.
#
#   ./scripts/winlist.sh          every on-screen window, frontmost first
#   ./scripts/winlist.sh <pid>    only that pid's; exit 1 if it has none
#
# SIX tab-separated fields, not nine — geometry is ONE field with commas inside it:
#   $1 windowNumber   $2 ownerPID   $3 layer   $4 x,y,w,h   $5 ownerName   $6 title
# An earlier version of this comment listed x/y/w/h separately, and an agent reading it took
# ownerName as $8. That returns empty for every window, so their launcher reported "the app
# did not come up" and their cleanup — guarded on a non-empty pid — skipped, orphaning a
# window while announcing a failed launch. Falsify any extractor against a window you know
# exists before trusting an empty result from it.
# Uses CGWindowListCopyWindowInfo, so it sees empty-title windows that Accessibility cannot.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
bin="${TMPDIR:-/tmp}/ng-winlist"
if [[ ! -x $bin || $here/winlist.swift -nt $bin ]]; then
  swiftc -O -o "$bin" "$here/winlist.swift"
fi
exec "$bin" "$@"
