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
# `cargo fmt --check` exits 0 even when rustfmt REJECTS an option, printing only a warning to
# stderr. Verified: appending a nightly-only `imports_granularity` gave exit 0 with 19 warning
# lines. So a config typo, or an option that silently needs nightly, would rot undetected.
formatting() {
  local err status
  # Captures stderr only, so $? is still rustfmt's own verdict on the diff.
  err=$(cargo fmt --all --check 2>&1 >/dev/null)
  status=$?
  [[ -n $err ]] && { printf '%s\n' "$err"; return 1; }
  return "$status"
}
step 'formatting'                   formatting
step 'unit tests'                   cargo test --release --lib

# Warnings do not fail a cargo build, so the zero-warning state would otherwise rot silently.
# Scoped to this crate's own files: a lint added by a future toolchain shows up as a named file
# and line rather than as a blanket build failure across every dependency.
#
# Cargo does NOT reliably replay diagnostics for units it did not rebuild. This step therefore
# passed while `examples/gallery.rs` carried a live dead-code warning, and an earlier version of
# this comment asserted the opposite. Touching this crate's own sources forces its units to be
# re-checked, and touches nothing a dependency owns.
own_warnings() {
  local out status
  find src examples -name '*.rs' -exec touch {} + || return 1
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
