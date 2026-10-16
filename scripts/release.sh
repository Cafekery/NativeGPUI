#!/usr/bin/env bash
# usage: release.sh — build Found'er.app and zip it into dist/Found'er-<version>.zip with its sha256
set -euo pipefail
cd "$(dirname "$0")/.."

app=$(./scripts/bundle.sh finder_demo | tail -1)
[ -f "$app/Contents/Resources/THIRD_PARTY_NOTICES.html" ] ||
  { echo "error: no third-party notices in $app; install cargo-about" >&2; exit 1; }
codesign --verify --deep --strict "$app"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
zip="dist/Found'er-$version.zip"
rm -f "$zip" "$zip.sha256"
ditto -c -k --keepParent "$app" "$zip"
(cd dist && shasum -a 256 "$(basename "$zip")" > "$(basename "$zip").sha256")
cat "$zip.sha256"
echo "$zip"
