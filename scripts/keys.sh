#!/usr/bin/env bash
# Drive examples/keys.rs with real keystrokes and print its results.
#
# The keystrokes come from System Events, not from a synthesised NSEvent, so they enter the app
# through the WindowServer exactly as a physical key does. That is also why the app cannot send
# them to itself: driving System Events needs an Automation grant, and asking for one from inside a
# freshly ad-hoc-signed bundle prompts, and the prompt steals key focus — which is the one thing a
# keyboard measurement cannot survive. So the app leads and this script follows: the app writes an
# AppleScript plus a sequence number, this script runs it and acknowledges the sequence number, and
# the app only reads state once the acknowledgement lands.
#
#   ./scripts/keys.sh                       # menus installed
#   KEYS_MENUS=0 ./scripts/keys.sh          # the arm the cmd keys were broken in
#   KEYS_SABOTAGE=want ./scripts/keys.sh    # every expectation inverted; every row must go red
#   KEYS_SABOTAGE=nokeys ./scripts/keys.sh  # full handshake, no keystroke; no row may pass
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

dir=${KEYS_DIR:-/tmp/ng-keys}
budget=${KEYS_BUDGET:-300}

app=$(./scripts/bundle.sh keys) || exit 1

rm -rf "$dir"
mkdir -p "$dir"

# A second live copy of this binary is fatal, not a nuisance. The app's frontmost test used to
# compare the process NAME, which a stale instance satisfies — one whole arm came back with no
# keystroke delivered and every row still claiming it was measured in front. The app now compares
# its own pid, and this refuses to start a second one at all. Back-to-back runs get a grace period
# because the previous instance may still be quitting.
for _ in $(seq 1 100); do
	stale=$(pgrep -x Keys | tr '\n' ' ')
	[ -z "$stale" ] && break
	sleep 0.1
done
if [ -n "$stale" ]; then
	echo "refusing to run: Keys already alive (pids $stale)"
	exit 1
fi

# `open` is what makes the window take key: LaunchServices activates a bundle and ignores a bare
# binary. --env passes the arm selection through, since `open` does not inherit our environment.
open -n --stdout "$dir/stdout.txt" --stderr "$dir/stderr.txt" \
	--env "KEYS_DIR=$dir" \
	--env "KEYS_MENUS=${KEYS_MENUS:-1}" \
	--env "KEYS_SABOTAGE=${KEYS_SABOTAGE:-none}" \
	"$app" || exit 1

# Unambiguous because the check above proved no Keys was running.
pid=""
for _ in $(seq 1 100); do
	pid=$(pgrep -x Keys | head -1)
	[ -n "$pid" ] && break
	sleep 0.05
done
[ -z "$pid" ] && { echo "Keys never started"; exit 1; }
echo "pid $pid"

last=""
deadline=$(( $(date +%s) + budget ))
while [ ! -f "$dir/done" ]; do
	if [ -f "$dir/req" ]; then
		seq=$(cat "$dir/req" 2>/dev/null)
		if [ -n "$seq" ] && [ "$seq" != "$last" ]; then
			osascript "$dir/req.applescript" > "$dir/fg.staged" 2> "$dir/err.txt"
			mv -f "$dir/fg.staged" "$dir/fg"
			# Two injectors, because neither covers the matrix alone. System Events types letters
			# and letter chords; its `click at` is a measured no-op. cliclick posts real CGEvents
			# and its clicks land, but it cannot hold a modifier over a letter. -r puts the pointer
			# back where the user left it. Frontmost is re-checked here because a click or key
			# aimed at a stale coordinate lands in whoever owns that pixel now.
			if [ -s "$dir/req.cliclick" ]; then
				want=$(cat "$dir/who" 2>/dev/null)
				if [ -n "$want" ] && [ "$(cat "$dir/fg")" = "$want" ]; then
					# shellcheck disable=SC2046  # the app generates this argument list
					cliclick -r $(cat "$dir/req.cliclick") >> "$dir/err.txt" 2>&1
				fi
			fi
			printf '%s' "$seq" > "$dir/ack.staged" && mv -f "$dir/ack.staged" "$dir/ack"
			last="$seq"
		fi
	fi
	kill -0 "$pid" 2>/dev/null || break
	[ "$(date +%s)" -gt "$deadline" ] && { echo "budget exhausted"; break; }
	sleep 0.01
done

for _ in $(seq 1 60); do
	kill -0 "$pid" 2>/dev/null || break
	sleep 0.1
done
# Only ever this pid, never a pattern: a bare pkill -f on a shared machine is a loaded gun.
kill -0 "$pid" 2>/dev/null && kill "$pid"

echo
printf '%-42s %-8s %s\n' ASSERTION VERDICT DETAIL
awk -F'\t' '{
	if ($2 == "NOTE") printf "%-42s %-8s %s\n", $1, "note", $4
	else printf "%-42s %-8s pre[%s] post[%s] want[%s] key[%s] fg[%s]\n", $1, $2, $3, $4, $5, $6, $7
}' "$dir/results.tsv" 2>/dev/null

[ -s "$dir/err.txt" ] && { echo; echo "osascript stderr:"; cat "$dir/err.txt"; }
exit 0
