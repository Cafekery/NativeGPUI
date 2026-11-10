#!/usr/bin/env bash
# usage: bundle.sh — build Found'er into dist/Found'er.app at the repo root and print its path
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 1
root=$PWD

product="Found'er"
icon_name=Founder
bundle_id=dev.cafekery.founder
app="dist/$product.app"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' founder/Cargo.toml | head -1)
build_number=$(git rev-list --count HEAD 2>/dev/null || echo 0)

cargo build --release -p founder || exit 1

bin="${CARGO_TARGET_DIR:-target}/release/founder"
[ -x "$bin" ] || { echo "no binary at $bin"; exit 1; }

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/$product"

if cargo about --version >/dev/null 2>&1; then
  (cd founder && cargo about generate --fail -o "$root/$app/Contents/Resources/THIRD_PARTY_NOTICES.html" about.hbs) ||
    exit 1
else
  echo "warning: no third-party notices; install cargo-about with: cargo install cargo-about --locked --features cli" >&2
fi

icon="founder/assets/$icon_name.icon"
icon_keys=
if [ -d "$icon" ]; then
  if xcrun actool --output-format human-readable-text --app-icon "$icon_name" \
    --output-partial-info-plist "$PWD/$app/Contents/Resources/icon.plist" --target-device mac \
    --minimum-deployment-target 26.0 --platform macosx \
    --compile "$PWD/$app/Contents/Resources" "$PWD/$icon" >/dev/null; then
    icon_keys=$'\t<key>CFBundleIconFile</key>\t\t<string>'$icon_name$'</string>\n\t<key>CFBundleIconName</key>\t\t<string>'$icon_name'</string>'
  else
    echo "warning: actool could not compile $icon" >&2
  fi
  rm -f "$app/Contents/Resources/icon.plist"
fi

doc_keys=$(cat <<'TYPES'
	<key>CFBundleDocumentTypes</key>
	<array>
		<dict>
			<key>CFBundleTypeName</key>		<string>Folder</string>
			<key>CFBundleTypeRole</key>		<string>Viewer</string>
			<key>LSHandlerRank</key>		<string>Alternate</string>
			<key>LSItemContentTypes</key>
			<array>
				<string>public.folder</string>
				<string>public.volume</string>
			</array>
		</dict>
	</array>
TYPES
)

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>	<string>en</string>
	<key>CFBundleAllowMixedLocalizations</key><true/>
	<key>CFBundleExecutable</key>		<string>$product</string>
	<key>CFBundleIdentifier</key>		<string>$bundle_id</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>CFBundleName</key>			<string>$product</string>
$icon_keys
$doc_keys
	<key>CFBundleDisplayName</key>		<string>$product</string>
	<key>CFBundlePackageType</key>		<string>APPL</string>
	<key>CFBundleShortVersionString</key>	<string>$version</string>
	<key>CFBundleVersion</key>		<string>$build_number</string>
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
	<key>NSLocalNetworkUsageDescription</key>	<string>$product asks nearby computers what they are, to show each server with its own icon as Finder does.</string>
	<key>NSBonjourServices</key>	<array>
		<string>_device-info._tcp</string>
		<string>_smb._tcp</string>
		<string>_afpovertcp._tcp</string>
		<string>_rfb._tcp</string>
	</array>
</dict>
</plist>
PLIST

# TCC keys a grant to the designated requirement; an ad-hoc signature's cdhash changes every build.
identity=${NG_CODESIGN_IDENTITY:-}
self_signed="NativeGPUI Dev"
keychain="$HOME/Library/Keychains/nativegpui-dev.keychain-db"
keychain_password=nativegpui

find_identity() {
  security find-identity -v -p codesigning 2>/dev/null |
    sed -n 's/^ *[0-9]*) [0-9A-F]* "\(.*\)"$/\1/p' | grep -m1 "$1"
}

# codesign finds a name only via the user's keychain search list, and signs silently only unlocked.
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
if [ -z "$identity" ]; then
  codesign --force --sign - --identifier "$bundle_id" --timestamp=none "$app" 2>/dev/null ||
    echo "warning: codesign failed" >&2
fi

# A provenance-tagged app leaves a ._ file per write on ExFAT/FAT. A tagged shell can't strip the
# tag and a bundle that ran tagged stays tracked, so launchd strips it before the first launch.
provenance() { xattr -r "$app" 2>/dev/null | grep -c ': com\.apple\.provenance$'; }
if [ "$(provenance)" != 0 ]; then
  label="dev.cafekery.nativegpui.bundle.$$"
  launchctl submit -l "$label" -- /usr/bin/xattr -dr com.apple.provenance "$PWD/$app"
  for _ in $(seq 1 100); do
    [ "$(provenance)" = 0 ] && break
    sleep 0.1
  done
  launchctl remove "$label" 2>/dev/null
  left=$(provenance)
  if [ "$left" != 0 ]; then
    echo "error: $left items in $app still carry com.apple.provenance; its copies to ExFAT would leave ._ files" >&2
    exit 1
  fi
fi

echo "$app"
