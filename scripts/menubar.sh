#!/usr/bin/env bash
# Read the standard menu bar back out of a *second* process and check it against a table.
#
# `examples/menu_bar.rs` asks AppKit about its own menus in-process, which is the stronger reading
# for selectors and targets — nothing else can report an `NSMenuItem`'s action. This is the
# independent one: System Events walks the accessibility tree from outside, so it cannot be fooled
# by the probe agreeing with itself, and it sees two things the in-process read does not.
#
# * The Apple menu, which is not in `NSApp.mainMenu` at all — the system prepends it.
# * The rows AppKit adds to the Window menu (Minimize All, Fill, Center, Move & Resize, Arrange in
#   Front). They are inserted when the menu is built for a client, so an in-process
#   `NSMenu::update` right after startup reports four rows where a reader sees fifteen. Whether
#   `setWindowsMenu:` took is therefore checked by object identity in the probe, not by counting.
#
# Reading needs no keyboard focus — only an Accessibility grant for whatever runs this. It takes
# no screenshot and sends no keystroke.
#
#   ./scripts/menubar.sh                    # launch the probe; every row below must be found
#   ./scripts/menubar.sh --sabotage         # same, with no bar installed; every row must go MISSING
#   ./scripts/menubar.sh --proc FinderDemo  # read an app that is already running
#   ./scripts/menubar.sh --dump <file>      # re-check a dump taken earlier, launching nothing
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

sabotage=0
proc=""
dump=/tmp/ng-menubar-ax.tsv
offline=0
while [ $# -gt 0 ]; do
	case $1 in
	--sabotage) sabotage=1 ;;
	# Check a dump taken earlier instead of reading a live app. This is how the title-alternation
	# fix above was verified against the app that exposed it, without relaunching anyone's app or
	# taking the screen: the failing case is a file.
	--dump)
		shift
		dump=${1:-}
		offline=1
		[ -r "$dump" ] || { echo "--dump needs a readable file"; exit 1; }
		;;
	# Read an app that is ALREADY running instead of launching the probe. This is how the fix
	# gets a second, independent consumer: an app that inherits the standard set rather than one
	# built to exercise it. `open -n dist/FinderDemo.app` then `--proc FinderDemo`.
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

# menu | title | key equivalent | modifier mask. A title may list `/`-separated alternatives,
# because AppKit rewrites some of these from the SUBJECT APP's state and the title is therefore not
# ours to predict: it flips Show/Hide Toolbar, Show/Hide Sidebar and Enter/Exit Full Screen while
# validating the row. Matching those on title alone made this script report 2 MISSING against any
# app that actually had a toolbar or a sidebar showing — i.e. against every real app — and never
# against the probe, which has neither surface so its titles never flip. Measured on a second
# consumer app: both rows present with the same key and mask, enabled, titled Hide rather than Show.
# That is a check that passed only on the configuration it was written for.
#
# Pipe-separated, not tab: bash collapses runs of
# tabs when IFS is one, so a row with no key equivalent silently shifts its mask into the key
# column and six correct rows reported MISSING.
#
# `AXMenuItemCmdModifiers` is a mask over the four Apple counts from Command: 1 shift, 2 option,
# 4 control, 8 *no* command. 16 is the Globe key, which is how macOS 26 spells full screen.
#   0 ⌘   1 ⇧⌘   2 ⌥⌘   3 ⌥⇧⌘   4 ⌃⌘   8 none   24 Globe
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
	# The app name the expectation table substitutes has to come from the dump, not from a flag:
	# reading it out of the data is what makes an offline check a check.
	app=$(awk -F'\t' 'NR == 1 { print $1 }' "$dump")
	[ -z "$app" ] && { echo "could not read an app name from $dump"; exit 1; }
	# `Apple` is the system menu, which the AX read sees first and this crate does not install.
	[ "$app" = Apple ] && app=$(awk -F'\t' '$1 != "Apple" { print $1; exit }' "$dump")
	echo "checking saved dump $dump as app [$app]; launching nothing"
elif [ -n "$proc" ]; then
	app=$proc
	# Exactly one, or the readback could describe either and neither of us would know which.
	count=$(pgrep -x "$app" | grep -c '')
	if [ "$count" -ne 1 ]; then
		echo "expected exactly one running $app, found $count"
		exit 1
	fi
	pid=$(pgrep -x "$app")
	echo "reading already-running $app pid $pid (not launched by this script, not killed by it)"
else
	cargo build --release --example menu_bar || exit 1
	bin=target/release/examples/menu_bar
	app=menu_bar

	# A second live copy would make `pgrep` ambiguous and the readback could describe either.
	if pgrep -x "$app" >/dev/null; then
		echo "a menu_bar is already running; refusing to guess which one to read"
		exit 1
	fi

	NG_ARM=hold NG_HOLD=40 NG_MENUS=$((1 - sabotage)) "$bin" >/tmp/ng-menubar-out.txt 2>&1 &
	launched=$!
	for _ in $(seq 1 50); do
		pid=$(pgrep -x "$app" | head -1)
		[ -n "$pid" ] && break
		sleep 0.1
	done
	[ -z "${pid:-}" ] && { echo "menu_bar never started"; cat /tmp/ng-menubar-out.txt; exit 1; }
	# Only ever this pid, and only one we launched ourselves: a bare pkill on a shared machine is
	# a loaded gun, and an `--proc` target belongs to whoever started it.
	trap 'kill "$pid" 2>/dev/null; wait "$launched" 2>/dev/null' EXIT
	sleep 2
fi

if ((offline)); then
	status=0
else

osascript scripts/menubar.applescript "$pid" >"$dump" 2>/tmp/ng-menubar-ax.err
status=$?
fi
if ((status)); then
	echo "System Events could not reach the process (needs an Accessibility grant for whatever runs this):"
	cat /tmp/ng-menubar-ax.err
	exit 1
fi
if grep -q '^NOBAR' "$dump"; then
	echo "read the process but it has no menu bar 1 at all"
fi

# The bar is what the reader saw, so a missing menu shows up as every one of its rows going MISSING
# rather than as one silent skip.
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
	# Inverted: with no bar installed, a row that is still found means the reader is describing
	# something other than this app's menus.
	((found == 0)) && { echo "SABOTAGE PASS: no row survived a bar that was never installed"; exit 0; }
	echo "SABOTAGE FAIL: $found rows found with no menu bar — the reader is not reading this app"
	exit 1
fi
exit $((fail > 0))
