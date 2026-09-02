#!/usr/bin/env bash
# usage: bundle.sh [example] [features] — build an example into dist/<Name>.app and print its path
set -uo pipefail

name=${1:-storybook}
features=${2:-}

if [ "$name" = storybook ] && [ -z "$features" ]; then
  features=private-api
  echo "storybook: defaulting to --features private-api"
fi

cd "$(dirname "$0")/.." || exit 1

title=$(python3 -c "import sys; print(sys.argv[1].replace('_',' ').title().replace(' ',''))" "$name")
app="dist/$title.app"

build=(cargo build --release --example "$name")
[ -n "$features" ] && build+=(--features "$features")
"${build[@]}" || exit 1

bin="target/release/examples/$name"
[ -x "$bin" ] || { echo "no binary at $bin"; exit 1; }

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/$title"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>	<string>en</string>
	<key>CFBundleExecutable</key>		<string>$title</string>
	<key>CFBundleIdentifier</key>		<string>dev.cafekery.nativegpui.$name</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>CFBundleName</key>			<string>$title</string>
	<key>CFBundleDisplayName</key>		<string>$title</string>
	<key>CFBundlePackageType</key>		<string>APPL</string>
	<key>CFBundleShortVersionString</key>	<string>0.1.0</string>
	<key>CFBundleVersion</key>		<string>1</string>
	<key>LSMinimumSystemVersion</key>	<string>26.0</string>
	<key>NSHighResolutionCapable</key>	<true/>
	<key>NSPrincipalClass</key>		<string>NSApplication</string>
	<key>NSSupportsAutomaticTermination</key><true/>
	<key>NSSupportsSuddenTermination</key>	<true/>
</dict>
</plist>
PLIST

# An unsigned bundle is refused activation, and its TCC grant is keyed to the signature.
codesign --force --sign - --timestamp=none "$app" 2>/dev/null || echo "warning: codesign failed"

echo "$app"
