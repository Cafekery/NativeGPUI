#!/usr/bin/env bash
# Each anchor is a platform fact that cost a measurement to find and that a reader would
# otherwise "simplify" into a bug. Run after any comment pass or file move.
#
# Patterns match prose, never an identifier, and are searched across all of src/ so a restructure
# cannot turn a surviving fact into a LOST. `--self-test` strips every comment from a copy of src/
# and requires all of them to go red: a check that still passes there is matching code and is
# worthless.
set -uo pipefail
cd "$(dirname "$0")/.."

anchors() {
  local where=$1 fail=0
  check() {
    if grep -rqiE "$2" "$where"; then
      printf 'ok    %s\n' "$1"
    else
      printf 'LOST  %s  (/%s/)\n' "$1" "$2"
      fail=1
    fi
  }

  check 'integerValue is cell state, not selection' "cell.?s state"
  check 'NSSlider rounds its origin on construction' "rounds (its )?(frame )?origin"
  check 'keyEquivalent does not round-trip'          "reads back as the empty"
  check 'gpui sizes from the window contentView'     "(pane.?cancelling|negative) origin"
  check 'transparent window shadow from content alpha' "content alpha"
  check 'glass needs something beneath it'           "beneath it|nothing to sample"
  check 'corner radius 0 means system default'       "system default"
  check 'setTarget: is weak'                         "is (a )?\*?weak"
  check 'non-key material flattens'                  "flat fill"
  check 'MinY renders above the anchor'              "renders above|above it"
  check 'RenderImage wants straight alpha'           "divide out alpha"
  check 'interaction state traps outside its range'  "traps|aborts the process"
  check 'stale writes would clobber a user edit'     "delete what the user|clobber"
  check 'AppKit callbacks defer off gpui.s borrow'   "holds its .?app.? borrow"
  check 'glass hitTest: override cannot work'        "never fires"
  check 'maskImage round-trips, so read-back diff ok' "round.?trips"
  check 'style cannot report an explicit variant'     "reads back as regular|cannot report"
  check 'makeFirstResponder ignores acceptsFirstResponder' "does not consult"
  check 'first responder is assigned after becomeFirst'    "overwritten by the call"
  check 'autorecalculate overwrites an explicit key loop'  "rederive the whole loop"
  check 'function keys reach a hosted field as insertions' "legacy ASCII spelling"
  check 'gpui.s key-equivalent walk skips hosted views'    "never descends"
  check 'gpui.s keyDown: does not forward up the chain'    "dies one link short"
  check 'only the ancestor chain differs for a switch'     "identical either way"
  check 'a rebuilt clip is appended, not restacked'        "lands topmost in its container"
  return $fail
}

if [[ ${1:-} == --self-test ]]; then
  strip=$(mktemp -d)
  trap 'rm -rf "$strip"' EXIT
  cp -R src "$strip/src"
  find "$strip/src" -name '*.rs' -exec sed -i '' -E '/^[[:space:]]*\/\//d' {} +
  echo "self-test: every anchor must go LOST against comment-stripped src/"
  if anchors "$strip/src" | grep -q '^ok'; then
    echo
    anchors "$strip/src" | grep '^ok' | sed 's/^ok /BAD  /'
    echo "FAIL: the above match code, not prose, and cannot detect a lost comment"
    exit 1
  fi
  echo "all anchors falsifiable"
  exit 0
fi

anchors src
