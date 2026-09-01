#!/usr/bin/env bash
# Everything that must hold before the tree is called clean. The four things it runs, `z_order`,
# `glass_hit`, `decoration_hit` and `tabular_figures`, each open a small unfocused corner window for
# a fraction of a second and read out of AppKit or gpui's text system, so it takes no keyboard focus
# and captures no pixels: still safe to run while someone else is driving the display.
#
# Contention is fine; a LOCKED session is not. `loginwindow` occludes every window at layer 2001 and
# up, so a gpui window stops getting display ticks and never paints. `z_order` then reports "the
# window painted fewer than 6 times", `glass_hit` and `decoration_hit` "the window rendered fewer
# than 4 frames", and `hosted controls reach their handler` needs a paint within 15 s. Those are the
# frame-count entry gates refusing to report rather than reporting a false green — so under a lock
# the verdict cannot be reached at all, and the answer is to unlock, not to skip the step.
#
# Contention is not free either, in one direction: starvation can only REMOVE frames, so a busy
# machine can manufacture a false red on those four and can never manufacture a green.
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
# Reads container subview order and the key view loop out of AppKit after a slot is rebuilt under
# an unchanged pool key, which is the one stale case a paint-order diff cannot see by itself.
step 'z-order holds paint order'    cargo run --release --example z_order
# A viewport-sized glass container used to answer `hitTest:` itself, making every hosted control
# painted before it unclickable. It reads its verdict out of AppKit rather than off the screen, so
# it captures no pixels — but it still needs its window to paint, and measured under a locked
# session it reports "no row was evaluated — the window rendered fewer than 4 frames within 12
# tries". In-process is not the same as needing no frames.
step 'glass container declines'     cargo run --release --example glass_hit
# The same property one class over, for the three decorations `material`, `native_label` and
# `native_rule` place — which `glass_hit` does not put on screen and therefore says nothing about.
# A stock `NSVisualEffectView`, `NSTextField` and `NSBox` each answer `hitTest:` with self, so a
# band of material carrying hosted titles and dividers over an interactive region eats every click
# and drag in its rect while looking perfectly correct.
#
# What it gates is the SIBLING RETURN, not a `nil`: a nil hit cannot tell "the view declined" from
# "nothing was there", so each declining row requires gpui's own view back exactly, at the centre
# of the view AppKit actually placed. The run also carries a row that must CLAIM — a hosted switch
# under the same material — without which deleting the layer container, hiding the views or placing
# nothing at all would read as four passes.
step 'decorations decline'          cargo run --release --example decoration_hit
# `system_font_tabular` is a `tnum` feature list, and if the shaper ever stops honouring the tag it
# still returns a `Font` — a proportional one, under a name promising a column that does not shift.
# Shapes ten of each digit through gpui's own text system, with a monospaced face and a nonsense
# tag as the two arms that keep a null meaningful.
#
# Unlike the two steps above it, this one runs fine under a locked session, both instrument arms
# included. Shaping is synchronous. That is the distinction worth carrying: "opens a window" is not
# what starves, "needs the window to have PAINTED" is. Exactly 2 renders are reachable while
# loginwindow occludes — measured, and with a control on an unrelated build to show the ceiling is
# the lock's and not any one change's — so a probe gated on 4 or 6 frames cannot pass by
# construction.
step 'tabular figures apply'         cargo run --release --example tabular_figures

# These docs carry measured platform facts, and a stale one on a public constructor already
# shipped. `-D warnings` rather than a grep for known lint names: an allowlist silently passes
# every lint it does not name, and gains nothing when a toolchain adds one.
own_docs() {
  touch src/lib.rs
  # Filtering is display-only; the verdict comes from cargo's status via PIPESTATUS.
  RUSTDOCFLAGS="-D warnings" cargo doc --no-deps 2>&1 |
    grep -vE '^\s*(Documenting|Finished|Generated|Compiling|Checking)|future version of Rust|future-incompat'
  return "${PIPESTATUS[0]}"
}
step 'docs resolve'                 own_docs

# A hosted control still reaches its handler. `element_key` -> `place` -> `wire` -> `on_change` is
# the crate's extension point, and until this step the only thing standing behind it was that the
# components happen to work when a human looks at a window. In-process, no synthetic input, no
# capture.
#
# All three arms are REQUIRED, and the reason is the pass arm rather than the sabotage arms. Its
# verdict rests on `cleared_fires == 0`, which is an absence, and two states produce it: the
# handler was correctly cleared, or the frame-60 `performClick:` never landed at all. The pass arm
# cannot tell those apart and reads PASS either way.
#
# `noclear` is what settles it. It runs the identical click and gets `cleared_fires == 1`, so the
# click demonstrably lands and demonstrably reaches a handler when one is registered — which is
# what rules out the second state for the pass arm. It is the control, not a self-test bolted on
# the side. Running pass alone would be a green that measures nothing.
#
# It also covers a third state nobody guards directly: a switch that exists while `performClick:`
# silently declines, on a disabled or offscreen control. Both arms share that path, so if the click
# stopped landing, `noclear` would go FAIL -> PASS and the three-arm requirement reports a broken
# gate instead of a passing one.
#
# Verified on the shipped binary: pass exit 0 / verdict=PASS, nohandler exit 90 / VOID, noclear
# exit 1 / FAIL.
#
# The stall guard was measured false here and is now fixed; the history is the load-bearing part.
# The probe claimed exit 90 if its window stopped painting within 15 s, and that test lived inside
# `render` — the one place that stops running under exactly the starvation it named, so the case it
# named was the case it could not reach. Each arm run directly under `timeout`, session confirmed
# locked, returned exit 124 — the timeout's status, not the probe's — with zero bytes on stdout AND
# stderr: it hung rather than reporting, and ate its whole budget where `z_order` and `glass_hit`
# fail in seconds and say why. The guard now sleeps on a thread of its own, which nothing in that
# failure mode can starve. Which numbers that produces depends entirely on the session, and the two
# cases have been confused once already: under a LOCK all three arms REPORT rather than hang, exit
# 90 in 15 s reading `frames=2 ... VOID (window painted 2 frames; both arms need 75)` — that is the
# starved reading, and 2 renders is the ceiling a lock allows. Unlocked, measured on this binary,
# the three arms read 0 / 90 / 1 as above. A starvation guard that never fires is worse than none,
# and a comment that leaves a starved reading standing as the normal one is the same defect in prose.
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

# The public surface itself. Nothing in-tree imports `native_gpui::chrome::`, so a `pub mod` put
# back by a later pass would be invisible to every other step here — the crate would build, the
# examples would build, and the API would have silently widened again. Metadata-only probes against
# the built rlib: no link, no frameworks, no window.
#
# Four arms, and the negative one counts errors rather than reading an exit code. Proven necessary
# by sabotage: restoring `pub mod chrome;` left the neg probe still exiting 1, because its other
# three paths still fail — so a reappeared module hides behind its neighbours and an exit-code
# check calls that ok. The count is the discriminator.
step 'public surface holds'         ./scripts/surface.sh

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
