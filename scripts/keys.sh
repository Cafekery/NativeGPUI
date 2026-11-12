#!/usr/bin/env bash
# usage: keys.sh — drive examples/keys.rs with real keystrokes and print its results.
#   env: KEYS_DIR, KEYS_BUDGET, KEYS_MENUS=0|1, KEYS_SABOTAGE=none|want|nokeys
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

dir=${KEYS_DIR:-${TMPDIR:-/tmp}/ng-keys}
budget=${KEYS_BUDGET:-300}

cargo build --release --example keys || exit 1
app=dist/Keys.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "${CARGO_TARGET_DIR:-target}/release/examples/keys" "$app/Contents/MacOS/Keys"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key>		<string>Keys</string>
	<key>CFBundleIdentifier</key>		<string>dev.cafekery.nativegpui.keys</string>
	<key>CFBundleName</key>			<string>Keys</string>
	<key>CFBundlePackageType</key>		<string>APPL</string>
	<key>LSMinimumSystemVersion</key>	<string>26.0</string>
	<key>NSHighResolutionCapable</key>	<true/>
	<key>NSPrincipalClass</key>		<string>NSApplication</string>
</dict>
</plist>
PLIST
codesign --force --sign - --identifier dev.cafekery.nativegpui.keys "$app" 2>/dev/null || exit 1

rm -rf "$dir"
mkdir -p "$dir"

for _ in $(seq 1 100); do
	stale=$(pgrep -x Keys | tr '\n' ' ')
	[ -z "$stale" ] && break
	sleep 0.1
done
if [ -n "$stale" ]; then
	echo "refusing to run: Keys already alive (pids $stale)"
	exit 1
fi

open -n --stdout "$dir/stdout.txt" --stderr "$dir/stderr.txt" \
	--env "KEYS_DIR=$dir" \
	--env "KEYS_MENUS=${KEYS_MENUS:-1}" \
	--env "KEYS_SABOTAGE=${KEYS_SABOTAGE:-none}" \
	"$app" || exit 1

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
			# System Events cannot click; cliclick cannot hold a modifier over a letter.
			if [ -s "$dir/req.cliclick" ]; then
				want=$(cat "$dir/who" 2>/dev/null)
				if [ -n "$want" ] && [ "$(cat "$dir/fg")" = "$want" ]; then
					# shellcheck disable=SC2046
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
kill -0 "$pid" 2>/dev/null && kill "$pid"

echo
printf '%-42s %-8s %s\n' ASSERTION VERDICT DETAIL
awk -F'\t' '{
	if ($2 == "NOTE") printf "%-42s %-8s %s\n", $1, "note", $4
	else printf "%-42s %-8s pre[%s] post[%s] want[%s] key[%s] fg[%s]\n", $1, $2, $3, $4, $5, $6, $7
}' "$dir/results.tsv" 2>/dev/null

[ -s "$dir/err.txt" ] && { echo; echo "osascript stderr:"; cat "$dir/err.txt"; }
exit 0
