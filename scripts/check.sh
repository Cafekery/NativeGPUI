#!/usr/bin/env bash
# The gate: both feature configs build, formatting, unit tests, the in-process probes, docs, the
# public surface, and zero warnings. Opens a few small unfocused windows; needs an unlocked session.
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

step 'build (default features)'     cargo build --release --examples
step 'build (private-api)'          cargo build --release --examples --features private-api

# `cargo fmt --check` exits 0 when rustfmt rejects an option and only warns on stderr.
formatting() {
  local err status
  err=$(cargo fmt --all --check 2>&1 >/dev/null)
  status=$?
  [[ -n $err ]] && { printf '%s\n' "$err"; return 1; }
  return "$status"
}
step 'formatting'                   formatting
step 'unit tests'                   cargo test --release --lib
step 'finder-fs tests'              cargo test --release -p finder-fs
step 'convert tests'                cargo test --release -p convert
step 'z-order holds paint order'    cargo run --release --example z_order
step 'glass container declines'     cargo run --release --example glass_hit
step 'decorations decline'          cargo run --release --example decoration_hit
step 'tabular figures apply'        cargo run --release --example tabular_figures
step 'frame cadence holds'          ./scripts/perf.sh

own_docs() {
  touch src/lib.rs
  RUSTDOCFLAGS="-D warnings" cargo doc --no-deps 2>&1 |
    grep -vE '^\s*(Documenting|Finished|Generated|Compiling|Checking)|future version of Rust|future-incompat'
  return "${PIPESTATUS[0]}"
}
step 'docs resolve'                 own_docs

# All three arms: the pass arm's verdict is an absence, and `noclear` proves the click lands.
dispatch_arms() {
  local bin=target/release/examples/ngdispatch code
  "$bin" >/dev/null 2>&1; code=$?
  printf '  pass arm      exit %-3s want 0\n' "$code"; [[ $code -eq 0 ]] || return 1
  NGD_SABOTAGE=nohandler "$bin" >/dev/null 2>&1; code=$?
  printf '  nohandler arm exit %-3s want 90 (VOID)\n' "$code"; [[ $code -eq 90 ]] || return 1
  NGD_SABOTAGE=noclear "$bin" >/dev/null 2>&1; code=$?
  printf '  noclear arm   exit %-3s want 1  (FAIL)\n' "$code"; [[ $code -eq 1 ]] || return 1
}
step 'hosted controls reach their handler' dispatch_arms
step 'public surface holds'         ./scripts/surface.sh

# Cargo does not replay diagnostics for units it did not rebuild, so touch our sources first.
own_warnings() {
  local out status
  find src examples -name '*.rs' -exec touch {} + || return 1
  out=$(cargo build --release --examples --features private-api --message-format=short 2>&1)
  status=$?
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
