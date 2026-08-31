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
# Columns are tab-separated: windowNumber, ownerPID, layer, x, y, w, h, ownerName, title.
# Uses CGWindowListCopyWindowInfo, so it sees empty-title windows that Accessibility cannot.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
bin="${TMPDIR:-/tmp}/ng-winlist"
if [[ ! -x $bin || $here/winlist.swift -nt $bin ]]; then
  swiftc -O -o "$bin" "$here/winlist.swift"
fi
exec "$bin" "$@"
