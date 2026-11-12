#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."

app=$(./founder/scripts/bundle.sh | tail -1)
[ -f "$app/Contents/Resources/THIRD_PARTY_NOTICES.html" ] ||
  { echo "error: no third-party notices in $app; install cargo-about" >&2; exit 1; }
codesign --verify --deep --strict "$app"

zip=dist/Founder.zip
rm -f "$zip" "$zip.sha256"
ditto -c -k --keepParent "$app" "$zip"
(cd dist && shasum -a 256 Founder.zip > Founder.zip.sha256)
cat "$zip.sha256"
echo "$zip"
