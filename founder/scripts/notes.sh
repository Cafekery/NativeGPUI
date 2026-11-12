#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

version=${1:?usage: notes.sh <version>}
notes=$(awk -v v="$version" '/^## /{on = ($2 == v); next} on' CHANGELOG.md | sed '/./,$!d')
[ -n "${notes//[[:space:]]/}" ] || { echo "error: no notes for $version in founder/CHANGELOG.md" >&2; exit 1; }
printf '%s\n' "$notes"
