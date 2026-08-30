#!/usr/bin/env bash
# Everything that must hold before the tree is called clean. Compiles nothing that runs, so it is
# safe to run while someone else is driving the display.
set -uo pipefail
cd "$(dirname "$0")/.."

fail=0
step() {
  printf '\n== %s\n' "$1"
  shift
  if "$@"; then
    printf 'PASS\n'
  else
    printf 'FAIL\n'
    fail=1
  fi
}

# Two configurations, because `custom_glass` and `glass_variants` carry
# required-features = ["private-api"] and a default build skips them by construction.
step 'build (default features)'     cargo build --release --examples
step 'build (private-api)'          cargo build --release --examples --features private-api
step 'anchors can fail'             ./scripts/anchors.sh --self-test
step 'anchors present'              ./scripts/anchors.sh
step 'formatting'                   cargo fmt --all --check
step 'unit tests'                   cargo test --release --lib

# Warnings do not fail a cargo build, so the zero-warning state would otherwise rot silently.
# Scoped to this crate's own files: a lint added by a future toolchain shows up as a named file
# and line rather than as a blanket build failure across every dependency.
#
# Proven able to fail by injecting `use std::io::Write;` into src/: one hit naming the file and
# line, zero after removing it. Cargo replays diagnostics for cached units, so a warm build does
# not silently pass, and --message-format=short emits paths relative to the crate root.
own_warnings() {
  local out status
  out=$(cargo build --release --examples --features private-api --message-format=short 2>&1)
  status=$?
  # A broken build emits no warnings from our files, so this must not pass vacuously.
  (( status )) && { printf 'build failed; warning state unknown\n'; return 1; }
  out=$(printf '%s\n' "$out" | grep -E '^(src|examples)/.*: warning:' || true)
  [[ -z $out ]] && return 0
  printf '%s\n' "$out"
  return 1
}
step 'no warnings in src/ or examples/' own_warnings

if (( fail )); then
  printf '\n== VERDICT: FAIL\n'
else
  printf '\n== VERDICT: PASS\n'
fi
exit $fail
