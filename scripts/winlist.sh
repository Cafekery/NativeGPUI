#!/usr/bin/env bash
# usage: winlist.sh [pid] — on-screen windows from the window server, frontmost first.
#   With a pid, only that pid's windows; exit 1 if it has none.
# Six tab-separated fields, geometry being one of them:
#   $1 windowNumber  $2 ownerPID  $3 layer  $4 x,y,w,h  $5 ownerName  $6 title
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
bin="${TMPDIR:-/tmp}/ng-winlist"
if [[ ! -x $bin || $here/winlist.swift -nt $bin ]]; then
  swiftc -O -o "$bin" "$here/winlist.swift"
fi
exec "$bin" "$@"
