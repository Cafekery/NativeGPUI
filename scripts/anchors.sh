#!/usr/bin/env bash
# Each anchor is a platform fact that cost a measurement to find and that a reader would otherwise
# "simplify" into a bug. Run after any comment pass or file move.
#
# Three properties this guard has to have, each learned by it failing to:
#
# * Matching is whitespace-normalised. A line-based grep reports a reflowed sentence as missing,
#   and on the `retired` arm — which asserts absence — it reports a merely rewrapped claim as gone,
#   which fails silently.
# * Every anchor names the file whose reader the fact protects, and only that file is searched.
#   Searching all of src/ let a fact leave the code it explains while a copy of the same words in
#   an unrelated file kept the check green. Ten of these patterns still match in several files.
# * Both arms are self-tested, in opposite directions: `--self-test` requires every anchor to go
#   LOST when its own file's comments are stripped, and every retired claim to go STALE when the
#   sentence is planted behind another directory.
#
# A file move breaks a check until its path is updated here. That is deliberate: a guard that
# adapts silently to a move is the thing being guarded against.
set -uo pipefail
cd "$(dirname "$0")/.."

# label ;; extended regex ;; path under src/
read -r -d '' ANCHORS <<'TABLE'
integerValue is cell state, not selection ;; cell.?s state ;; bridge/host.rs
NSSlider rounds its origin on construction ;; rounds (its )?(frame )?origin ;; bridge/host.rs
keyEquivalent does not round-trip ;; reads back as the empty ;; bridge/host.rs
setTarget: is weak ;; is (a )?\*?weak ;; bridge/host.rs
stale writes would clobber a user edit ;; delete what the user|clobber ;; bridge/host.rs
AppKit callbacks defer off gpui.s borrow ;; holds its .?app.? borrow ;; bridge/host.rs
makeFirstResponder ignores acceptsFirstResponder ;; does not consult ;; bridge/host.rs
first responder is assigned after becomeFirst ;; overwritten by the call ;; bridge/host.rs
autorecalculate overwrites an explicit key loop ;; rederive the whole loop ;; bridge/host.rs
a rebuilt clip is appended, not restacked ;; lands topmost in its container ;; bridge/host.rs
function keys reach a hosted field as insertions ;; legacy ASCII spelling ;; bridge/appkit.rs
gpui.s key-equivalent walk skips hosted views ;; never descends ;; bridge/appkit.rs
gpui.s keyDown: does not forward up the chain ;; dies one link short ;; bridge/appkit.rs
only the ancestor chain differs for a switch ;; identical either way ;; bridge/appkit.rs
gpui sizes from the window contentView ;; (pane.?cancelling|negative) origin ;; chrome/sidebar.rs
corner radius 0 means system default ;; system default ;; chrome/window_chrome.rs
transparent window shadow from content alpha ;; content alpha ;; chrome/window_chrome.rs
MinY renders above the anchor ;; renders above|above it ;; surfaces.rs
glass needs something beneath it ;; beneath it|nothing to sample ;; materials/glass.rs
glass hitTest: override cannot work ;; never fires ;; materials/glass.rs
non-key material flattens ;; flat fill ;; materials/material.rs
maskImage round-trips, so read-back diff ok ;; round.?trips ;; materials/material.rs
interaction state traps outside its range ;; traps|aborts the process ;; materials/glass_private.rs
style cannot report an explicit variant ;; reads back as regular|cannot report ;; materials/glass_private.rs
RenderImage wants straight alpha ;; divide out alpha ;; system/tokens.rs
TABLE

# Newlines collapse to spaces, so a phrase split across lines still matches.
flat() { find "$@" -name '*.rs' -exec cat {} + 2>/dev/null | tr '\n' ' ' | tr -s ' \t'; }

anchors() {
  local root=$1 fail=0 label pat path
  while IFS= read -r row; do
    [[ -z $row ]] && continue
    label=${row%% ;; *}
    path=${row##* ;; }
    pat=${row#* ;; }
    pat=${pat%% ;; *}
    if flat "$root/$path" | grep -qiE "$pat"; then
      printf 'ok    %s\n' "$label"
    else
      printf 'LOST  %s  (%s /%s/)\n' "$label" "$path" "$pat"
      fail=1
    fi
  done <<<"$ANCHORS"
  return $fail
}

# The mirror of an anchor: a fixed limitation must not outlive its fix. Add a line whenever a fix
# retires a documented one.
retired() {
  local fail=0
  # `${@:3}` and not `$@`: find exits nonzero on a nonexistent operand, and under pipefail that
  # would make the pipeline false no matter what grep found.
  gone() {
    if flat "${@:3}" | grep -qiE "$2"; then
      printf 'STALE %s  (/%s/)\n' "$1" "$2"
      fail=1
    else
      printf 'gone  %s\n' "$1"
    fi
  }
  local -a where=("${@:1}")
  gone 'cmd chords never reach a hosted field' "do not reach a hosted field|Cmd-A/C/V do not" "${where[@]}"
  gone 'caret keys are the window.s own gap'   "separate open gap|anything this crate configures" "${where[@]}"
  gone 'editing needs an upstream gpui change' "needs a change in gpui|[Ff]ixing it needs" "${where[@]}"
  return $fail
}

strip_comments() { find "$1" -name '*.rs' -exec sed -i '' -E '/^[[:space:]]*\/\//d' {} +; }

if [[ ${1:-} == --self-test ]]; then
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  bad=0

  # Stripping ONE file's comments must lose exactly the anchors that name it. Everything else in
  # src/ stays, so a duplicate of the phrase elsewhere cannot keep the check green.
  echo "self-test: each anchor must go LOST when only its own file is stripped"
  while IFS= read -r row; do
    [[ -z $row ]] && continue
    label=${row%% ;; *}
    path=${row##* ;; }
    rm -rf "$work/src"
    cp -R src "$work/src"
    strip_comments "$work/src/$path"
    if anchors "$work/src" | grep -qF "ok    $label"; then
      printf 'BAD   %s  survives its own file being stripped\n' "$label"
      bad=1
    fi
  done <<<"$ANCHORS"
  (( bad )) && { echo "FAIL: the above match code or another file, not the comment they guard"; exit 1; }
  echo "all anchors falsifiable and file-scoped"

  # A retired claim is falsified the other way round: plant the sentence and require it to be
  # caught. It goes in the second directory so dropping a later argument also fails.
  mkdir -p "$work/empty" "$work/planted"
  cat > "$work/planted/stale.rs" <<'STALE'
// Cmd-A/C/V do not reach a hosted field.
// Arrow keys are a separate open gap, the window rather than anything this crate configures.
// Fixing it needs a change in gpui.
STALE
  echo "self-test: each retired claim must go STALE when planted behind another directory"
  planted_out=$(retired "$work/empty" "$work/planted")
  if grep -q '^gone' <<<"$planted_out"; then
    grep '^gone' <<<"$planted_out" | sed 's/^gone /BAD  /'
    echo "FAIL: the above cannot detect the claim they retire, or skip a later directory"
    exit 1
  fi
  echo "all retired claims falsifiable"
  exit 0
fi

anchors src
status=$?
retired src examples || status=1
exit $status
