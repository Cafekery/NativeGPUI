#!/usr/bin/env bash
# usage: bundle.sh [example] [features] — build an example into dist/<Name>.app and print its path
set -uo pipefail

name=${1:-storybook}
features=${2:-}

if [ "$name" = storybook ] && [ -z "$features" ]; then
  features=private-api
  echo "storybook: defaulting to --features private-api" >&2
fi

cd "$(dirname "$0")/.." || exit 1

title=$(python3 -c "import sys; print(sys.argv[1].replace('_',' ').title().replace(' ',''))" "$name")
# The name the app wears. The example's own name stays the code's: the bundle id, and the icon
# document and asset the icon keys name, are built from it and outlive any renaming.
product=$title
[ "$name" = finder_demo ] && product="Found'er"
app="dist/$product.app"
bundle_id="dev.cafekery.nativegpui.$name"

build=(cargo build --release --example "$name")
[ -n "$features" ] && build+=(--features "$features")
"${build[@]}" || exit 1

bin="target/release/examples/$name"
[ -x "$bin" ] || { echo "no binary at $bin"; exit 1; }

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/$product"

# Icon Composer's document, compiled the way Xcode would: the layered icon lands in Assets.car
# and the flattened .icns beside it, for anything that still reads one.
icon="assets/$title.icon"
icon_keys=
if [ -d "$icon" ]; then
  if xcrun actool --output-format human-readable-text --app-icon "$title" \
    --output-partial-info-plist "$PWD/$app/Contents/Resources/icon.plist" --target-device mac \
    --minimum-deployment-target 26.0 --platform macosx \
    --compile "$PWD/$app/Contents/Resources" "$PWD/$icon" >/dev/null; then
    icon_keys=$'\t<key>CFBundleIconFile</key>\t\t<string>'$title$'</string>\n\t<key>CFBundleIconName</key>\t\t<string>'$title'</string>'
  else
    echo "warning: actool could not compile $icon" >&2
  fi
  rm -f "$app/Contents/Resources/icon.plist"
fi

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>	<string>en</string>
	<key>CFBundleExecutable</key>		<string>$product</string>
	<key>CFBundleIdentifier</key>		<string>$bundle_id</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>CFBundleName</key>			<string>$product</string>
$icon_keys
	<key>CFBundleDisplayName</key>		<string>$product</string>
	<key>CFBundlePackageType</key>		<string>APPL</string>
	<key>CFBundleShortVersionString</key>	<string>0.1.0</string>
	<key>CFBundleVersion</key>		<string>1</string>
	<key>LSMinimumSystemVersion</key>	<string>26.0</string>
	<key>NSHighResolutionCapable</key>	<true/>
	<key>NSPrincipalClass</key>		<string>NSApplication</string>
	<key>NSSupportsAutomaticTermination</key><true/>
	<key>NSSupportsSuddenTermination</key>	<true/>
	<key>NSDesktopFolderUsageDescription</key>	<string>$product lists your Desktop the way Finder does.</string>
	<key>NSDocumentsFolderUsageDescription</key>	<string>$product lists your Documents the way Finder does.</string>
	<key>NSDownloadsFolderUsageDescription</key>	<string>$product lists your Downloads the way Finder does.</string>
	<key>NSRemovableVolumesUsageDescription</key>	<string>$product lists connected disks under Locations.</string>
	<key>NSNetworkVolumesUsageDescription</key>	<string>$product lists mounted network shares under Locations.</string>
	<key>NSFileProviderDomainUsageDescription</key>	<string>$product lists cloud folders such as iCloud Drive.</string>
	<key>NSAppDataUsageDescription</key>		<string>$product lists other apps' data folders while browsing your Library.</string>
</dict>
</plist>
PLIST

# TCC keys a grant to the app's designated requirement. Ad-hoc signing makes that requirement the
# cdhash, which every rebuild changes, so every rebuild is a new app that has to be granted again; a
# real identity makes it identifier-plus-certificate, which survives rebuilds.
identity=${NG_CODESIGN_IDENTITY:-}
self_signed="NativeGPUI Dev"
keychain="$HOME/Library/Keychains/nativegpui-dev.keychain-db"
keychain_password=nativegpui

find_identity() {
  security find-identity -v -p codesigning 2>/dev/null |
    sed -n 's/^ *[0-9]*) [0-9A-F]* "\(.*\)"$/\1/p' | grep -m1 "$1"
}

# No identity on this Mac: keep one of ours in a keychain of its own, with a password the script
# knows so later builds sign unattended. codesign resolves a name only through the user's keychain
# search list, and only an unlocked keychain whose key allows codesign signs without a dialog.
own_identity() {
  local dir status
  [ -f "$keychain" ] || security create-keychain -p "$keychain_password" "$keychain" || return 1
  local -a list
  while IFS= read -r line; do list+=("$(printf '%s' "$line" | sed 's/^ *"//;s/"$//')"); done \
    < <(security list-keychains -d user)
  printf '%s\n' "${list[@]}" | grep -qxF "$keychain" ||
    security list-keychains -d user -s "${list[@]}" "$keychain" || return 1
  security unlock-keychain -p "$keychain_password" "$keychain" || return 1
  security set-keychain-settings "$keychain"
  if ! security find-certificate -c "$self_signed" "$keychain" >/dev/null 2>&1; then
    dir=$(mktemp -d) || return 1
    cat > "$dir/req.cfg" <<CFG
[req]
distinguished_name = dn
x509_extensions    = ext
prompt             = no
[dn]
CN = $self_signed
[ext]
basicConstraints   = critical,CA:false
keyUsage           = critical,digitalSignature
extendedKeyUsage   = critical,codeSigning
CFG
    /usr/bin/openssl req -x509 -newkey rsa:2048 -sha256 -days 3650 -nodes \
      -config "$dir/req.cfg" -keyout "$dir/key.pem" -out "$dir/cert.pem" >/dev/null 2>&1 &&
      /usr/bin/openssl pkcs12 -export -inkey "$dir/key.pem" -in "$dir/cert.pem" \
        -name "$self_signed" -passout "pass:$keychain_password" -out "$dir/id.p12" >/dev/null 2>&1 &&
      security import "$dir/id.p12" -k "$keychain" -P "$keychain_password" -A >/dev/null 2>&1 &&
      security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
        -k "$keychain_password" "$keychain" >/dev/null 2>&1
    status=$?
    rm -rf "$dir"
    [ $status -eq 0 ] || return 1
  fi
  printf '%s' "$self_signed"
}

if [ -z "$identity" ]; then
  identity=$(find_identity '^Developer ID Application:') ||
    identity=$(find_identity '^Apple Development:') ||
    identity=$(find_identity '^Mac Developer:') ||
    identity=$(own_identity)
fi

if [ -n "$identity" ]; then
  sign=(codesign --force --sign "$identity" --identifier "$bundle_id" --timestamp=none
    --options runtime)
  [ "$identity" = "$self_signed" ] && sign+=(--keychain "$keychain")
  "${sign[@]}" "$app" 2>/dev/null ||
    { echo "warning: codesign as '$identity' failed" >&2; identity=; }
fi
# Last resort: an ad-hoc bundle launches, but its privacy grants die with the next build.
if [ -z "$identity" ]; then
  codesign --force --sign - --identifier "$bundle_id" --timestamp=none "$app" 2>/dev/null ||
    echo "warning: codesign failed" >&2
fi

echo "$app"
