#!/usr/bin/env bash
# usage: surface.sh — type-check the four surface/*.rs arms (pos, posctl, neg, negctl)
#   against the built rlib, default features only. Exits non-zero if any arm goes the wrong way.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

target=${CARGO_TARGET_DIR:-target}
rlib=$target/release/libnative_gpui.rlib
if ! built=$(cargo build --release --lib --message-format=json 2>/dev/null); then
  echo "surface: cargo build --release --lib failed; surface state unknown" >&2
  exit 1
fi
[ -f "$rlib" ] || { echo "surface: $rlib not produced" >&2; exit 1; }

# Where cargo keeps dependency artifacts differs by version (deps/ or build/*/*/out), so ask it.
search=()
while IFS= read -r dir; do
  search+=(-L "dependency=$dir")
done < <(printf '%s\n' "$built" | grep -o '"filenames":\[[^]]*\]' \
  | grep -oE '"[^"]+\.(rlib|rmeta|dylib)"' | tr -d '"' | sed 's,/[^/]*$,,' | sort -u)
[ "${#search[@]}" -gt 0 ] || { echo "surface: cargo reported no dependency artifacts" >&2; exit 1; }

out=$(mktemp -d) || exit 1
trap 'rm -rf "$out"' EXIT

check() {
  rustc --edition 2021 --emit=metadata \
    --extern "native_gpui=$rlib" "${search[@]}" \
    --out-dir "$out" "scripts/surface/$1.rs" 2>&1
}

fail=0
arm() {
  local name=$1 want=$2 desc=$3 output status
  output=$(check "$name")
  status=$?
  if [ "$status" -eq "$want" ]; then
    printf 'ok    %-6s %s\n' "$name" "$desc"
    return 0
  fi
  printf 'FAIL  %-6s %s (exit %d, wanted %d)\n' "$name" "$desc" "$status" "$want"
  printf '%s\n' "$output" | sed 's/^/        /'
  fail=1
}

arm pos    0 'every public path the crate promises resolves'
arm posctl 1 'control: the same file plus one name that never existed'

# One error per probe: the exit code alone cannot tell four rejections from one.
probes=$(grep -cE '^ *// probe:' scripts/surface/neg.rs)
output=$(check neg)
status=$?
errors=$(printf '%s\n' "$output" | grep -c '^error\[E')
if [ "$status" -ne 0 ] && [ "$errors" -eq "$probes" ]; then
  printf 'ok    %-6s %s\n' 'neg' "each of the $probes removed paths is still gone, and gone separately"
else
  printf 'FAIL  %-6s %d of %d removed paths rejected — the rest are reachable again\n' \
    'neg' "$errors" "$probes"
  printf '%s\n' "$output" | sed 's/^/        /'
  fail=1
fi
arm negctl 0 'control: the removal probes, aimed at paths that do exist'

exit "$fail"
