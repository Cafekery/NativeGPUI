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

# label ;; extended regex ;; path from the repo root (src/… or examples/…)
read -r -d '' ANCHORS <<'TABLE'
integerValue is cell state, not selection ;; cell.?s state ;; src/bridge/host.rs
NSSlider rounds its origin on construction ;; rounds (its )?(frame )?origin ;; src/bridge/host.rs
keyEquivalent does not round-trip ;; reads back as the empty ;; src/bridge/host.rs
setTarget: is weak ;; is (a )?\*?weak ;; src/bridge/host.rs
stale writes would clobber a user edit ;; delete what the user|clobber ;; src/bridge/host.rs
AppKit callbacks defer off gpui.s borrow ;; holds its .?app.? borrow ;; src/bridge/host.rs
makeFirstResponder ignores acceptsFirstResponder ;; does not consult ;; src/bridge/host.rs
first responder is assigned after becomeFirst ;; overwritten by the call ;; src/bridge/host.rs
autorecalculate overwrites an explicit key loop ;; rederive the whole loop ;; src/bridge/host.rs
a rebuilt clip is appended, not restacked ;; lands topmost in its container ;; src/bridge/host.rs
function keys reach a hosted field as insertions ;; legacy ASCII spelling ;; src/bridge/appkit.rs
gpui.s key-equivalent walk skips hosted views ;; never descends ;; src/bridge/appkit.rs
gpui.s keyDown: does not forward up the chain ;; dies one link short ;; src/bridge/appkit.rs
only the ancestor chain differs for a switch ;; identical either way ;; src/bridge/appkit.rs
gpui sizes from the window contentView ;; (pane.?cancelling|negative) origin ;; src/chrome/sidebar.rs
corner radius 0 means system default ;; system default ;; src/chrome/window_chrome.rs
MinY renders above the anchor ;; renders above|above it ;; src/surfaces.rs
glass needs something beneath it ;; beneath it|nothing to sample ;; src/materials/glass.rs
glass hitTest: override cannot work ;; never fires ;; src/materials/glass.rs
glass container claims every point in its bounds ;; for every point inside its bounds ;; src/materials/glass.rs
container decline rests on a private subview ;; full.?bounds subview declines ;; src/materials/glass.rs
non-key material flattens ;; flat fill ;; src/materials/material.rs
maskImage round-trips, so read-back diff ok ;; round.?trips ;; src/materials/material.rs
interaction state traps outside its range ;; traps|aborts the process ;; src/materials/glass_private.rs
style cannot report an explicit variant ;; reads back as regular|cannot report ;; src/materials/glass_private.rs
RenderImage wants straight alpha ;; divide out alpha ;; src/system/tokens.rs
an extension needs the declared type, not the direct lookup ;; declared match wins ;; src/system/tokens.rs
drawInRect: selects a representation by output size ;; destination pixels ;; src/system/tokens.rs
a file icon does not vary with appearance ;; fixed artwork ;; src/system/tokens.rs
NSStringDrawing does not shape an OpenType feature ;; reliably shape ;; src/system/tokens.rs
custom glass fidelity was measured once, keyed ;; noise floor ;; src/materials/custom_glass.rs
a literal value fights its own write-back ;; wipes the control ;; src/lib.rs
a nested NSMenu does not inherit autoenablesItems ;; does not inherit ;; src/controls/menu.rs
setSubmenu: owns the host row.s action ;; dismiss the menu on a click|instead of opening the submenu ;; src/controls/menu.rs
a zero-framed item view is an invisible row ;; contributes zero height|completely invisible ;; src/controls/menu.rs
a menu item.s key mask already defaults to Command ;; own default is ;; src/controls/menu.rs
a view with no intrinsic width gets none ;; no width at all ;; src/chrome/window_chrome.rs
a momentary group holds no selection ;; reports -1 forever ;; src/chrome/window_chrome.rs
a search field must be wired before assignment ;; configured before assigned ;; src/chrome/window_chrome.rs
Finder's alternating band is an inset pill, not full-bleed ;; stops 10pt short of each edge ;; examples/finder_demo/list.rs
Finder draws column widths it does not store ;; stored 178 and drawn 197 ;; examples/finder_demo/header.rs
list offsets are measured from the pane edge ;; sits 26 in ;; examples/finder_demo/list.rs
the pill radius needed calibrating against AppKit ;; ideal quarter.disc ;; examples/finder_demo/list.rs
the list font size is a folder view option, not a metric ;; defaults to 13 ;; examples/finder_demo/list.rs
a toolbar item is its image plus 24pt ;; building the item at 20 and reading back 44 ;; examples/finder_demo/toolbar.rs
without navigational the run lands after the title ;; 471 against Finder.s 196 ;; examples/finder_demo/toolbar.rs
NSMenuToolbarItem shows an indicator by default ;; it cost 13pt ;; examples/finder_demo/toolbar.rs
the search item width drives the reflow, not the field ;; 205pt instead of 203 ;; examples/finder_demo/toolbar.rs
the sidebar width is chosen for where it leaves the list ;; pane began at 200 ;; examples/finder_demo/sidebar.rs
the preview pane width includes its divider ;; 594pt of list ;; examples/finder_demo/preview.rs
the even row fill differs by appearance, not just in value ;; 0.047 alpha ;; examples/finder_demo/palette.rs
icon size must land on a shipped representation ;; 64px representation ;; examples/finder_demo/ui.rs
relative date formatting supplies the narrow no-break space ;; 2pt a row ;; examples/finder_demo/data.rs
NSCalendar normalises an out-of-range day component ;; no carrying of its own ;; examples/finder_demo/data.rs
the window background is one colour, not two ;; visible as a step|one surface interrupted ;; src/system/tokens.rs
NSWindow.backgroundColor is inert here ;; is then inert|left the window capture byte-identical ;; src/bridge/root.rs
the sidebar backdrop must not go opaque ;; [Rr]esist making it opaque|flatter panel ;; src/chrome/sidebar.rs
menu bar answers a chord only after the window declines ;; only after the window declines ;; src/bridge/menus.rs
gpui.s delegate re-locks its own mutex on a tag miss ;; re-?locks a mutex it is still holding ;; src/bridge/menus.rs
AppKit adds rows to the bar it is handed ;; rows of its own ;; src/bridge/menus.rs
presentation lag is an async present ;; hands the surface to Core Animation asynchronously ;; src/bridge/appkit.rs
queue depth measured at two counts ;; measured as exactly 2 at the default count of 3 ;; src/bridge/appkit.rs
hosted views led by exactly two frames ;; the lead was exactly 2\.000 frames at every velocity ;; src/bridge/host.rs
a nested CATransaction commits late ;; a nested commit lands after the one the layout came in on ;; src/bridge/host.rs
gpui's own flag names the presentation path ;; reading which path is presenting us ;; src/bridge/host.rs
an unsettled queue is visible at rest ;; visible at rest rather than only under motion ;; src/bridge/host.rs
a bundled app already has a main menu at startup ;; already has a main menu ;; src/bridge/menus.rs
performKeyEquivalent true is not evidence of effect ;; not.{0,4} evidence the action ran ;; examples/menu_bar.rs
a window capture cannot see a material sampling ;; reports its own flat tint ;; src/system/tokens.rs
screencapture writes Display P3, greys cannot detect it ;; 52,120,246 ;; AGENTS.md
Finder's opaque surface is one colour across band and body ;; 29,29,28 ;; AGENTS.md
browser-row-three-routes ;; Three routes agree on the twenty-four ;; examples/finder_demo/column.rs
browser-rowheight-needs-item-delegate ;; against a matrix delegate it raises ;; examples/finder_demo/column.rs
column-divider-not-separator ;; over this body would read 51 ;; examples/finder_demo/column.rs
header-label-inset-icon-size ;; coincide and at 32pt they do not ;; examples/finder_demo/header.rs
header-ink-not-box ;; a label box is not its ink ;; examples/finder_demo/header.rs
header-press-wash-untokened ;; No public colour reaches it ;; examples/finder_demo/header.rs
header-name-pinned ;; out of the reorderable list is what enforces it here ;; examples/finder_demo/header.rs
header-drag-listener-order ;; the frame that begins a drag has no position yet ;; examples/finder_demo/header.rs
header-ghost-both-axes ;; an active drag's view at the cursor in ;; examples/finder_demo/header.rs
header-divider-hit-order ;; hands the grab zone to the divider ;; examples/finder_demo/header.rs
header-reorder-stable-target ;; a narrow column dropped past a wide one ;; examples/finder_demo/header.rs
material-paints-its-style ;; skips a transparent background ;; src/materials/material.rs
icon grid pitch is the cell ;; there is no gap between cells to leave out ;; examples/finder_demo/icon.rs
icon surround is constant not a ratio ;; the surround is a constant that does not scale ;; examples/finder_demo/icon.rs
icon view stores positions not an order ;; stores one position per item rather than an order ;; examples/finder_demo/icon.rs
selection plate is out of flow ;; drawn out of flow, inset negatively ;; examples/finder_demo/icon.rs
selection plate is not a selection colour ;; does not change when the window loses key ;; examples/finder_demo/icon.rs
absolute tiles give the canvas no height ;; absolute and so contribute no height ;; examples/finder_demo/icon.rs
drop hit-tests rather than stopping propagation ;; itself a bubble-phase mouse listener ;; examples/finder_demo/icon.rs
the elide budget is deliberately tight ;; generous hands the string to the line clamp ;; examples/finder_demo/icon.rs
artwork answers a square box on both arms ;; box is square whichever path answers ;; examples/finder_demo/ui.rs
a filmstrip size between rungs is resampled ;; a size between two of them is resampled ;; examples/finder_demo/gallery.rs
the gallery pane already shows the inspector ;; already renders the trailing preview pane in gallery view ;; examples/finder_demo/gallery.rs
header-rule-not-flush ;; nine rules, it runs y 56.0 to 75.0 ;; examples/finder_demo/header.rs
header-rule-peak-is-hairline ;; the 63 belongs to the hairline ;; examples/finder_demo/header.rs
header-one-leading-inset ;; there is no stagger and one constant is right ;; examples/finder_demo/header.rs
header-lead-trail-asymmetry ;; asymmetric is leading against ;; examples/finder_demo/header.rs
content thumbnails are out of reach ;; nothing in this demo reads the filesystem ;; examples/finder_demo/icon.rs
airdrop-coretypes-route ;; no restriction is being worked around ;; examples/finder_demo/toolbar.rs
airdrop-toolbar-unverified ;; draws this same artwork is an assumption ;; examples/finder_demo/toolbar.rs
crumb-volume-not-folder ;; wrong rather than merely generic ;; examples/finder_demo/toolbar.rs
sidebar-mostly-symbols ;; reaches into its own artwork only where ;; examples/finder_demo/sidebar.rs
sidebar-emphasis-is-focus ;; drawn emphasized .+ only while its outline ;; examples/finder_demo/sidebar.rs
sidebar-rep-point-not-pixel ;; point size and a pixel size are different ;; src/chrome/sidebar.rs
sidebar-drag-defers-commit ;; return does not opt back out ;; src/chrome/sidebar.rs
sidebar-ivars-not-inherited ;; leaves the superclass.s uninitialised ;; src/chrome/sidebar.rs
sidebar-eject-ratio ;; ratio is the statistic to compare ;; src/chrome/sidebar.rs
the try_resolve control lives in the tokens page ;; only live control on this ;; src/system/tokens.rs
a popover tracks its positioning view, not a rect ;; stays put while the element leaves ;; src/surfaces.rs
positioningRect does nothing on a shown popover ;; writing it moves nothing ;; src/surfaces.rs
an explicit close must not call back ;; the incoming one is stored and clears it ;; src/surfaces.rs
a toolbar backdrop is what the item glass samples ;; stadium inverts into an outline ;; src/chrome/window_chrome.rs
the band's rim is the backdrop's, not the separator's ;; rim belongs to the backdrop ;; src/chrome/window_chrome.rs
the demo keeps the transparent titlebar on purpose ;; lands on top of the preview pane ;; examples/finder_demo/toolbar.rs
TABLE

# Newlines collapse to spaces, so a phrase split across lines still matches.
# Comment markers are stripped BEFORE the newlines collapse. Without that, a phrase straddling a
# doc-comment line break flattens to "instead of /// opening the submenu" and reports LOST while
# the sentence is perfectly intact — a false red that turns up right after a clean run.
flat() {
  find "$@" \( -name '*.rs' -o -name '*.md' \) -exec cat {} + 2>/dev/null |
    sed -E 's|^[[:space:]]*//[/!]?||; s|^[[:space:]]*\*[^/]||' |
    tr '\n' ' ' | tr -s ' \t'
}

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
  gone 'the shadow follows content alpha'    "derives the drop shadow from|square shadow" "${where[@]}"
  gone 'the drawable default stands on cost' "extra buffer costs more in long frames|default stands because" "${where[@]}"
  gone 'the browser row height is unmeasured' "only the list view was measured" "${where[@]}"
  gone 'the header label inset is 46'      "coincide at 46" "${where[@]}"
  gone 'the icon tile pitch includes a gap' "last column ends up clipped" "${where[@]}"
  gone 'header and row insets are staggered' "neither the ink nor the box carries across" "${where[@]}"
  gone 'airdrop is a private SF Hello symbol' "SF Hello" "${where[@]}"
  gone 'a window-scoped capture misses the popover' "capture scoped to the GPUI window misses it" "${where[@]}"
  gone 'the capsules did not render at all' "capsules did not render at all" "${where[@]}"
  return $fail
}

# A `.rs` file's guarded prose is its comments, so stripping them is what falsifies an anchor. A
# `.md` file is prose end to end — there is nothing to strip *around* — so the equivalent is
# emptying it. Without that distinction a markdown anchor survives its own file being stripped and
# the self-test correctly reports it unfalsifiable, which is how this was found.
strip_comments() {
  find "$1" -name '*.rs' -exec sed -i '' -E '/^[[:space:]]*\/\//d' {} +
  find "$1" -name '*.md' -exec sh -c ': > "$1"' _ {} \;
}

if [[ ${1:-} == --self-test ]]; then
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  bad=0

  # Stripping ONE file's comments must lose exactly the anchors that name it. Every other file
  # stays, so a duplicate of the phrase elsewhere cannot keep the check green.
  echo "self-test: each anchor must go LOST when only its own file is stripped"
  while IFS= read -r row; do
    [[ -z $row ]] && continue
    label=${row%% ;; *}
    path=${row##* ;; }
    rm -rf "$work/src" "$work/examples"
    rm -f "$work"/*.md
    cp -R src "$work/src"
    cp -R examples "$work/examples"
    # Root-level docs are anchorable too, so they have to be in the scratch tree or every
    # markdown row reports LOST for the wrong reason — absent rather than stripped.
    cp ./*.md "$work"/ 2>/dev/null || true
    strip_comments "$work/$path"
    # Capture, then test. `anchors` returns nonzero whenever ANY row is LOST — which is by
    # construction here, since a file was just stripped — and under `pipefail` that makes
    # `anchors … | grep` false however grep answered, so `BAD` could never print. This arm
    # asserted "all anchors falsifiable" while checking nothing, everywhere a file holds more
    # than one anchor. Same trap `retired()` documents at its own `${@:3}`.
    out=$(anchors "$work" || true)
    if grep -qF "ok    $label" <<<"$out"; then
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
// On a transparent window macOS derives the drop shadow from content alpha, so an unpainted band
// reaching a corner gives a rounded window and a square shadow.
// The default stands because the extra buffer costs more in long frames than the lead is worth.
// The browser (column) view keeps the compact row: only the list view was measured.
// The tile pitch includes the gap, or the last column ends up clipped.
// The header inset and the row inset stagger: neither the ink nor the box carries across.
// airdrop, airdrop.circle and airdrop.square are in SF Hello's catalogue marked private.
// The popover's own NSWindow, which is why a capture scoped to the GPUI window misses it.
// With an opaque backdrop behind a transparent titlebar the capsules did not render at all.
// The header label inset and the icon size coincide at 46.
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

anchors .
status=$?
retired src examples || status=1
exit $status
