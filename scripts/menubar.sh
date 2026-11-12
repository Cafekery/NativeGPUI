#!/usr/bin/env bash
# usage: menubar.sh [--sabotage | --proc <name> | --dump <file>]
#   check a process's menu bar, read from outside via System Events, against the table below.
#   no argument launches examples/menu_bar; --sabotage launches it with no bar installed.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

sabotage=0
proc=""
tmp=${TMPDIR:-/tmp}
dump=$tmp/ng-menubar-ax.tsv
offline=0
while [ $# -gt 0 ]; do
	case $1 in
	--sabotage) sabotage=1 ;;
	--dump)
		shift
		dump=${1:-}
		offline=1
		[ -r "$dump" ] || { echo "--dump needs a readable file"; exit 1; }
		;;
	--proc)
		shift
		proc=${1:-}
		[ -z "$proc" ] && { echo "--proc needs a process name"; exit 1; }
		;;
	*) echo "unknown argument: $1"; exit 1 ;;
	esac
	shift
done
if ((offline)) && { ((sabotage)) || [ -n "$proc" ]; }; then
	echo "--dump reads a saved file and cannot be combined with --sabotage or --proc"
	exit 1
fi
if ((sabotage)) && [ -n "$proc" ]; then
	echo "--sabotage launches its own arm and cannot be combined with --proc"
	exit 1
fi

# menu | title | key equivalent | modifier mask. `/` in a title lists alternatives, because AppKit
# flips some titles from the subject app's state. Mask: 1 shift, 2 option, 4 control, 8 no command,
# 16 Globe.
read -r -d '' TABLE <<'ROWS'
{app}|About {app}||8
{app}|Hide {app}|H|0
{app}|Hide Others|H|2
{app}|Show All||8
{app}|Quit {app}|Q|0
File|Close|W|0
Edit|Undo|Z|0
Edit|Redo|Z|1
Edit|Cut|X|0
Edit|Copy|C|0
Edit|Paste|V|0
Edit|Paste and Match Style|V|3
Edit|Delete||8
Edit|Select All|A|0
View|Show Toolbar/Hide Toolbar|T|2
View|Customize Toolbar…||8
View|Show Sidebar/Hide Sidebar|S|4
View|Enter Full Screen/Exit Full Screen|F|24
Window|Minimize|M|0
Window|Zoom||8
Window|Bring All to Front||8
Help|{app} Help|?|0
ROWS

if ((offline)); then
	app=$(awk -F'\t' 'NR == 1 { print $1 }' "$dump")
	[ -z "$app" ] && { echo "could not read an app name from $dump"; exit 1; }
	# `Apple` is the system menu, prepended by macOS and not installed by this crate.
	[ "$app" = Apple ] && app=$(awk -F'\t' '$1 != "Apple" { print $1; exit }' "$dump")
	echo "checking saved dump $dump as app [$app]; launching nothing"
elif [ -n "$proc" ]; then
	app=$proc
	count=$(pgrep -x "$app" | grep -c '')
	if [ "$count" -ne 1 ]; then
		echo "expected exactly one running $app, found $count"
		exit 1
	fi
	pid=$(pgrep -x "$app")
	echo "reading already-running $app pid $pid (not launched by this script, not killed by it)"
else
	cargo build --release --example menu_bar || exit 1
	bin=${CARGO_TARGET_DIR:-target}/release/examples/menu_bar
	app=menu_bar

	if pgrep -x "$app" >/dev/null; then
		echo "a menu_bar is already running; refusing to guess which one to read"
		exit 1
	fi

	NG_ARM=hold NG_HOLD=40 NG_MENUS=$((1 - sabotage)) "$bin" >"$tmp/ng-menubar-out.txt" 2>&1 &
	launched=$!
	for _ in $(seq 1 50); do
		pid=$(pgrep -x "$app" | head -1)
		[ -n "$pid" ] && break
		sleep 0.1
	done
	[ -z "${pid:-}" ] && { echo "menu_bar never started"; cat "$tmp/ng-menubar-out.txt"; exit 1; }
	trap 'kill "$pid" 2>/dev/null; wait "$launched" 2>/dev/null' EXIT
	sleep 2
fi

if ! ((offline)) && ! osascript scripts/menubar.applescript "$pid" >"$dump" 2>"$tmp/ng-menubar-ax.err"; then
	echo "System Events could not reach the process (needs an Accessibility grant for whatever runs this):"
	cat "$tmp/ng-menubar-ax.err"
	exit 1
fi
if grep -q '^NOBAR' "$dump"; then
	echo "read the process but it has no menu bar 1 at all"
fi

printf '%-10s %-24s %-3s %-4s %s\n' MENU TITLE KEY MODS VERDICT
fail=0
found=0
while IFS='|' read -r menu title key mods; do
	[[ -z $menu ]] && continue
	menu=${menu//\{app\}/$app}
	title=${title//\{app\}/$app}
	if awk -F'\t' -v m="$menu" -v t="$title" -v k="$key" -v d="$mods" \
		'BEGIN { alts = split(t, alt, "/") }
		 $1 == m && $3 == k && $4 == d {
			for (i = 1; i <= alts; i++)
				if ($2 == alt[i]) hit = 1
		 }
		 END { exit !hit }' "$dump"; then
		printf '%-10s %-24s %-3s %-4s %s\n' "$menu" "$title" "$key" "$mods" ok
		found=$((found + 1))
	else
		printf '%-10s %-24s %-3s %-4s %s\n' "$menu" "$title" "$key" "$mods" MISSING
		fail=$((fail + 1))
	fi
done <<<"$TABLE"

echo
echo "$found found, $fail missing, $(wc -l <"$dump" | tr -d ' ') rows read"
if ((sabotage)); then
	((found == 0)) && { echo "SABOTAGE PASS: no row survived a bar that was never installed"; exit 0; }
	echo "SABOTAGE FAIL: $found rows found with no menu bar — the reader is not reading this app"
	exit 1
fi
exit $((fail > 0))
