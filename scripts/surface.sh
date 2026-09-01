#!/usr/bin/env bash
# The public surface is exactly what the crate claims. Four arms, type-check only.
#
# Nothing else here can see this failure class. The crate is never imported by its own name inside
# this tree — examples reach it as a path dependency and use only the paths they happen to need —
# so a re-export dropped from `src/lib.rs`, or a private grouping module handed back as `pub mod`,
# changes nothing any other check reads. Both configurations build, every warning stays absent,
# every example still runs, and the surface a consumer sees has silently moved.
#
# Why four and not one. A positive arm alone cannot tell "the path exists" from "the file compiled
# for some other reason", and a negative arm alone cannot tell "the path is gone" from "the arm
# never reached the crate". So each direction carries a control that must come out the other way:
# pos+posctl prove a present path is really being resolved, neg+negctl prove an absent one is
# really absent. Two early versions of the negative arm were red for reasons unrelated to the
# removals — one named a `SystemColor` variant that does not exist, one wrote `let _ = generic_fn;`
# which needs a type annotation whether or not the path resolves — and both would have been read as
# proof. negctl is what catches that.
#
# `--emit=metadata` stops after type-checking, so there is no link step, no AppKit, no window and
# no display: the whole matrix is well under a second and is safe to run beside anything.
#
# Default features only. `private-api` is off by default and the semantic `Material` set and the
# public glass API are the supported surface; probing the feature-gated names here would make the
# arms disagree between configurations for a reason that is not a defect.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

rlib=target/release/libnative_gpui.rlib
if ! cargo build --release --lib >/dev/null 2>&1; then
  echo "surface: cargo build --release --lib failed; surface state unknown" >&2
  exit 1
fi
# A missing rlib would make every arm fail identically and look like a real finding.
[ -f "$rlib" ] || { echo "surface: $rlib not produced" >&2; exit 1; }

out=$(mktemp -d) || exit 1
trap 'rm -rf "$out"' EXIT

check() {
  rustc --edition 2021 --emit=metadata \
    --extern "native_gpui=$rlib" -L dependency=target/release/deps \
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

# The negative arm needs one error PER PROBE, and its exit code alone cannot say so: with four
# probes and one path handed back, rustc still exits 1 on the other three and the arm reads green.
# That is not hypothetical — restoring `pub mod chrome;` and running this file produced exactly
# that, four oks and a reappeared module. So the count is the verdict, not a footnote beside it.
#
# The expected number comes from the probes themselves, so a fifth removal needs no edit here.
# Anchored to the line start because neg.rs's own header names the marker, and an unanchored
# count reads that mention as an extra probe.
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
