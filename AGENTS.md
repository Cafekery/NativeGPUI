# Rules for working in this repo

`native-gpui` hosts **real AppKit views** inside a GPUI window on macOS 26, so a Mac app inherits
Apple's future designs instead of freezing them in shaders. GPUI owns layout; macOS owns the pixels
of anything whose design Apple changes between releases (`src/lib.rs`, crate docs).

Read [README.md](README.md) for the map — modules, examples, scripts, how to build and run. This
file is the rules: the invariants, and the traps that have already cost hours. Nothing here is
style preference. Every claim cites where you can check it.

---

## 1. One background

**Whatever you use for the background is used for the background everywhere** — the toolbar band,
the content pane behind it, the body. A window is **two surfaces, never three**:

| surface | what it is |
|---|---|
| the sidebar pane | vibrancy — `SidebarOptions::backdrop`, a behind-window `NSVisualEffectView` |
| everything else | one opaque colour — `tokens::WINDOW_BACKGROUND` |

Two values on the content side are visible as a step. A window's toolbar band and its body are the
same surface interrupted by a strip of chrome, and resolving them differently draws a line across
the window where no line exists. See `src/lib.rs` § *One background* and `tokens::WINDOW_BACKGROUND`
in `src/system/tokens.rs`.

**Measured on real Finder** rather than asserted (commit `65742ad`, `git show 65742ad`): its toolbar
band above the file list and the list body read the same, **(29,29,28)**, both key-invariant and
flat. Its sidebar and preview pane are **not colours** — they read **27 when key, 41 and 46 when
not**, which a fill cannot do. These are numbers about *another app on one macOS release*, which is
why they are here and deliberately not in a doc comment — see § 6.

**Corrected later, and the correction is the more useful fact** (commit `b77e236`): Finder's toolbar
draws **no backdrop at all**. That (29,29,28) is the list's own background showing through a
transparent toolbar, not a fill — proven by scrolling a 304-item folder in a Finder window and
finding blurred list rows *inside* the band at y 4, 46, 50 and 54. The number stands; the mechanism
sentence did not. It read as flat because at rest there is nothing under it to move, which is one
reading consistent with two states — the shape § 2 is about.

**What this crate was doing wrong** (commit `6a6a610`): the toolbar band above the content pane
showed the window-spanning backdrop while the body was painted an opaque colour, so the two never
matched — **67 against 30 in dark, 222 against 255 in light**. A **37-unit step** at the toolbar's
lower edge in dark, **33** in light. Worse than a wrong constant: the backdrop blends
behind-window, so its value moves with the desktop and *no constant could have reconciled them*.
The content pane now paints `tokens::WINDOW_BACKGROUND` across its own bounds, band included.

**The exception is the sidebar, and it is the only one.** Resist making the backdrop opaque to
match the content side. It is the one surface here that shows the desktop; swapping it for an
opaque material moved the sidebar **40 → 27** in the band and **43 → 32** in its interior — a
flatter panel, and the step it was meant to close is closed at the pane instead
(`SidebarOptions::backdrop`, `src/chrome/sidebar.rs`). A sidebar sampling the desktop is the one
thing macOS sidebars do.

In practice:

* With `sidebar::NativeSidebar`, the crate paints the content side for you, band included — **an
  app should paint no background at all.** `examples/storybook` does exactly this, and the sidebar
  demo was changed to match (`26d1f39`).
* Without a sidebar, paint `tokens::window_background()` and nothing else (`src/lib.rs`).
* A `Layer::Below` `NSVisualEffectView` only shows through where GPUI painted nothing, so an opaque
  `.bg(system_color(..))` over a material silently replaces the blur with an approximation of it —
  paint nothing, or below alpha 1 (`system_color`, `src/system/tokens.rs`).
* `NSWindow.backgroundColor` is **not** where a window's background comes from and is inert here:
  writing it from gpui's near-clear value to an opaque `windowBackgroundColor`, with a readback in
  the same call confirming the write landed, left the window capture byte-identical
  (`window_options`, `src/bridge/root.rs`). Paint the background in the view that owns the surface.

Two cheap confirmations that a surface is a material and not a colour: its value tracks whatever it
samples (`UnderWindowBackground` reads 38 over flat grey 29 and 44 over grey 60, while
`windowBackgroundColor` reads 30 over both), and it is **state-dependent** — the sidebar interior
moves 43 → 55 in dark when the window stops being key. A colour does neither
(`tokens::WINDOW_BACKGROUND`). That discriminator was independently reproduced this session on a
different window: sidebar interior **29 key → 41 non-key**, while the content body read **29 in
both**. Two agents, two windows, same conclusion — and the key-side values differ between them, so
treat the *movement* as the evidence, never the absolute number.

---

## 2. Verifying anything here without fooling yourself

**The rule that generalises all of this: when a single reading is consistent with two states, it is
not evidence for either.** Seven times in one session an agent reported something clean that was
not — the last of them inside the guard that exists to catch exactly this. Every one was an honest
reading that could not distinguish the assumed state from the actual one. None was caught by looking
harder at the same reading; each was caught by varying an axis the author had not.

Before trusting an instrument, **prove it can fail.** A metric that reads 0 on a subject that never
moved is not evidence about a moving subject. Every gate in `scripts/` carries a deliberate negative
arm for this reason: `anchors.sh --self-test`, `keys.sh` with `KEYS_SABOTAGE=want|nokeys`,
`menubar.sh --sabotage`, `lockguard.sh` with `LOCKGUARD_FAKE_SHOT`.

**Carry a known-positive through the same instrument, and let it disqualify the instrument.** A
known-positive that *fails* is the only thing that can disqualify an instrument rather than merely
cast doubt on it; suspicion alone has cost real time here, twice, on probes that turned out fine.
Worked example from this session: `NSView.cacheDisplay` renders a `Sidebar` material offscreen at
**30/46** while the same material on screen reads **27/41**. That is a known-positive failing, and it
settles that `cacheDisplay` cannot see vibrancy at all — independently measured by two agents on two
different subjects. Anything it reports about a material is void, and that is a *result*, not an
obstacle to work around.

**And then check that the negative arm itself can fail.** The sharpest illustration in this repo is
the comment guard's own self-test — the check whose entire job is proving other checks falsifiable.
Its first arm used to read:

```bash
set -uo pipefail
if anchors "$work" | grep -qF "ok    $label"; then   # BAD: the row survived its own file
```

`anchors()` returns nonzero whenever any row is LOST — which is true **by construction** here, since
a file was just stripped — and under `pipefail` that makes the pipeline false however `grep`
answered. So `BAD` could never print. It was blind exactly where it mattered: a file with a *single*
anchor still worked, because nothing else went LOST and the status stayed 0, while every multi-anchor
file — which is most of them, and `src/bridge/host.rs` carries well over a dozen — was unchecked. The
line `all anchors falsifiable and file-scoped` had been asserting something it never tested for as
long as it had existed.

**Fixed** by capturing before testing — `out=$(anchors "$work" || true)` and then a herestring, in
`scripts/anchors.sh` — and **proven live rather than re-asserted**: doctoring one row from prose to
the live identifier `WINDOW_BACKGROUND`, which survives comment-stripping and therefore *must* be
caught, now prints `BAD`, prints `FAIL` and exits 1; restored, it exits 0 with zero `BAD` rows. All
every row is now measured falsifiable rather than claimed to be. The script had already documented
this identical trap for its *other* arm, ten lines away, at `retired()`'s `${@:3}`.

Two rules fall out of it, and both cost someone time tonight:

* **Never read `$?` after a pipe.** Four separate agents did it in one session, which makes it a
  that had just printed `BAD` and `FAIL`, because the status belonged to `head`. Capture the output,
  then test it.
* **Doubt your instrument, but settle it by instrumenting it, not by reasoning about it.** Two probes
  were genuinely void before the real defect was found — one ran `cd "$(dirname $0)/.."` from
  `/tmp`, landed at `/`, copied nothing and passed vacuously — and a real defect was nearly abandoned
  on the strength of that. In the words of the agent who found it: *"My first instinct on a
  surprising red was to doubt my probe. That instinct was right twice and wrong the third time.
  Distinguishing 'my instrument is broken' from 'the subject is broken' needed the instrumented run,
  not more reasoning."*

### Screen capture

* **`screencapture -x -o -l <windowid>` is the only sanctioned capture.** Window-scoped and
  occlusion-immune: it contains the window's own pixels and nothing else, so the wallpaper and the
  desktop cannot leak into a reading — a stronger guarantee than setting the desktop to something
  plain (`scripts/uidiff.py`). **Never `-R`, never full-screen.** The owner's own apps are on this
  display.
* **A window capture cannot see a material sampling the desktop.** It composites over *nothing*
  behind the window, so a behind-window material reports its own flat tint however colourful the
  desktop is, and the null reads as "not sampling". A flat reading does **not** mean a flat fill
  (`tokens::WINDOW_BACKGROUND`, guarded by anchor row *a window capture cannot see a material
  sampling*). Proven directly this session: a 16-hue window placed immediately behind the subject
  produced **byte-identical** output. Three agents nearly concluded the opposite from a flat
  reading. Use **key vs non-key** instead: a material moves, an opaque colour does not.
* **`screencapture` writes Display P3.** Comparing a captured **saturated** colour against an sRGB
  semantic `NSColor` is therefore invalid. Resolved in-process, both spaces, both appearances —
  reproduce with `NSColor.usingColorSpace(.sRGB)` against `.displayP3` inside
  `NSAppearance.performAsCurrentDrawingAppearance`:

  | colour | | sRGB | P3 |
  |---|---|---|---|
  | `controlAccentColor` | either | 0,122,255 | **52,120,246** |
  | `selectedContentBackgroundColor` | dark | 0,89,209 | **36,88,202** |
  | `selectedContentBackgroundColor` | light | 0,100,225 | **41,98,217** |
  | `unemphasizedSelectedContentBackgroundColor` | dark | 70,70,70 | 70,70,70 |
  | `windowBackgroundColor` | dark | 30,30,30 | 30,30,30 |
  | `windowBackgroundColor` | light | 255,255,255 | 255,255,255 |

  The two saturated colours **follow the user's accent preference** — the values above are the
  default blue on this machine, and both `controlAccentColor` and `selectedContentBackgroundColor`
  move if that preference changes. Resolve them at measurement time; never hard-code them into a
  harness. The neutral rows are appearance-dependent but accent-independent, and identical across the
  two spaces in every appearance, which is exactly why they cannot validate a pipeline.

  Finder's focused column-view selection pill measures a dead-flat **(36,88,202)**, uniform to ±1 over
  40 scanlines — the P3 triple exactly, not a mismatch. Stronger still, a storybook badge that
  *explicitly asked for* `system_color(SystemColor::ControlAccent)` was captured at **(52,120,246)**:
  the app requested sRGB 0,122,255 and the capture recorded P3, so nothing has to be inferred about
  intent.

  **The trap is that greys cannot detect this.** In the same measurement the greys matched exactly —
  ancestor pill (70,70,70) against `unemphasizedSelectedContentBackgroundColor`, body (29,29,29) —
  because **the neutral axis is shared between the two spaces**, as the table shows on every neutral
  row. So "my greys agree with the semantic colours" is consistent with both *"I am in the right
  colour space"* and *"I am in the wrong one"*, and it reads as confirmation. **If you validated a
  capture pipeline on greys alone, you validated nothing.**

  Working practice:

  * **The saturated known-positive does not have to be planted — it is very often already in your
    frame, in an element you are ignoring.** The accent value above sat in one agent's first profile
    of the night, disconfirming their conclusion, unread because the badge was not the subject.
    Before trusting a grey validation, resolve what the most colourful thing in the capture should be.
  * Prefer `controlAccentColor` as the positive: it moves **122** on red where the selection pill
    moves 36, so a partially-applied conversion shows up on the accent long before the pill.
  * If you convert through the embedded ICC profile, **assert the profile is present first**
    (`Image.info['icc_profile']` is non-nil on every PNG `screencapture` writes). A profile-less PNG
    makes the conversion an identity and the saturated arm goes quiet in precisely the way that reads
    as a pass.
  * Tolerance **±1**, not "close enough". The resolver pairs are byte-exact, so a converted pill at
    (2,91,207) is a defect to fail on rather than slop to absorb.

  Scope, so nobody re-runs work that is fine: every neutral-axis result in this file stands — the
  67 → 30 step, 255 in light, the sidebar 43 ↔ 55 and 27 ↔ 41/46, the (29,29,28) surface table. Only
  saturated comparisons are suspect: selection blues, accent tints, anything checked against a
  semantic accent colour. Convert, or compare capture-to-capture in one space.
* `-l` **without** `-o` includes the drop shadow, and the window is **not centred** in the result —
  measured margins 112/112 left/right, 76 top, 148 bottom. Find the window rect by the **bounding
  box of alpha ≥ 250**, never by arithmetic on the image size. Use `-o` unless the shadow *is* the
  subject (`scripts/uidiff.py`). *(The margin figures are a session measurement and are not
  currently recorded in a source comment — see §6.)*
* **Run `./scripts/lockguard.sh` before *and* after every capture round.** A capture taken while the
  display is locked or asleep returns a shield window or stale black and still produces plausible
  numbers, which is the whole hazard. **Exit 90 means the numbers are void.** Note its own trap:
  `loginwindow` is *always* a running process, so its presence means nothing — the signal is it
  being **frontmost**. A first version of that script aborted every time for exactly that reason.

### Key window

* **`activate` does not make a window key.** A bare Cargo binary is not a bundle, and
  LaunchServices will not activate it, so `activate(ignoringOtherApps:)` is ignored and the window
  can fail to take key (`scripts/bundle.sh`, measured on macOS 26.5). `open -n Foo.app` on the
  bundler's output takes key on every launch — which is why keyboard and key-state work must be
  bundled first (`scripts/keys.sh`).
* **A frontmost gate passes while the window is not key.** Read `isKeyWindow` / `NSApp.keyWindow`
  directly, or use a subject property that is itself key-dependent. `examples/menu_bar.rs` gates on
  `app.keyWindow().is_some() || app.mainWindow().is_some()` and reports *blocked*, not *pass*, when
  neither resolves. Related: `keys.sh` used to compare the process **name** for frontmost-ness,
  which a stale instance satisfies — one whole arm came back with no keystroke delivered and every
  row still claiming it was measured in front.

### Cleanup, and knowing what is on the screen

**Cleanup needs two mechanisms that ask different questions: process-level *and* window-server.**
Spawn-and-wait, `ps`, `pgrep` and `lsappinfo` are **one mechanism wearing four hats** — they answer
"what did I start", never "what is on the screen". Three agents in one session had all four come
back clean while a window sat on another agent's measurement subject (`scripts/winlist.sh`).

The case that motivated it: a probe with a deliberate `panic!()` negative control exits with a real
status, and macOS then opens a **Problem Reporter window owned by a different process**. Nothing
that asks about your own pid can see it.

`./scripts/winlist.sh` is the window-server census (`CGWindowListCopyWindowInfo`, so it sees
empty-title windows Accessibility cannot). **Six** tab-separated fields, not nine — geometry is
**one** field with commas inside it:

```
$1 windowNumber   $2 ownerPID   $3 layer   $4 x,y,w,h   $5 ownerName   $6 title
```

An earlier version of that comment listed x/y/w/h separately and an agent took `ownerName` as `$8`.
That returns empty for every window, so their launcher reported "the app did not come up" and their
cleanup — guarded on a non-empty pid — skipped, orphaning a window while announcing a failed launch.
**Falsify any extractor against a window you know exists before trusting an empty result from it.**

A further consequence, if you share the display with another agent: **an offscreen harness is not
automatically safe.** It creates no window while it works, but a panic in it still produces a
Problem Reporter dialog owned by another process, which lands wherever it lands and which the
capturing agent will absorb silently. Whoever holds the screen holds it exclusively; a harness that
creates no `NSWindow` at all is genuinely unaffected and can carry on. Announce taking and releasing
the screen by name, and settle cleanup with a window-server census plus an exhaust check —
`~/Library/Logs/DiagnosticReports` unchanged means nothing crashed.

### The same shape inside the gates themselves

* `cargo fmt --all --check` **exits 0 even when rustfmt rejects an option**, printing only a warning
  to stderr — verified: a nightly-only `imports_granularity` gave exit 0 with 19 warning lines. So
  `check.sh` fails on non-empty stderr as well as on status.
* **Cargo does not reliably replay diagnostics for units it did not rebuild.** The zero-warning step
  passed while `examples/gallery.rs` carried a live dead-code warning, and an earlier version of
  that comment asserted the opposite. `check.sh` now touches this crate's own sources first.
* `menubar.sh` matched menu rows on **title**, but AppKit rewrites some titles from the *subject
  app's* state — Show/Hide Toolbar, Show/Hide Sidebar, Enter/Exit Full Screen. It reported 2 MISSING
  against any app that actually had a toolbar or a sidebar showing, i.e. against every real app, and
  never against the probe, which has neither surface. **A check that passed only on the
  configuration it was written for.**
* Same shape in the crate: `install_menus` used to detect its own bar by its first menu's title
  being the app's name. A **bundled** app already has a `mainMenu` at startup and AppKit titles its
  first item with the app name, so the test matched the system's bar on the very first call and the
  app came up with one empty menu and no ⌘W, ⌘Q, ⌘M or ⌘H at all. Unbundled it passed — **the
  failure existed only in the configuration every shipped app is in** (`src/bridge/menus.rs`).

---

## 3. The anchor system

`scripts/anchors.sh` guards load-bearing *why* comments: a platform fact that cost a measurement to
find and that a reader would otherwise "simplify" into a bug. **Run it after any comment pass or
file move.** Row count, which moves as facts are found and retired:
`grep -c ' ;; ' scripts/anchors.sh`.

Row format, one per line in the `ANCHORS` heredoc:

```
label ;; extended regex ;; path from the repo root
NSSlider rounds its origin on construction ;; rounds (its )?(frame )?origin ;; src/bridge/host.rs
```

Four properties, each learned by the guard failing to have it:

1. **Matching is whitespace-normalised**, and comment markers are stripped *before* newlines
   collapse. A line-based grep reports a reflowed sentence as missing; without the marker stripping,
   a phrase straddling a doc-comment line break flattens to `instead of /// opening the submenu` and
   reports LOST while the sentence is intact.
2. **Every row names the one file whose reader the fact protects, and only that file is searched.**
   Searching all of `src/` let a fact leave the code it explains while a copy of the same words in an
   unrelated file kept the check green. The script's own note: ten of these patterns still match in
   several files, so the scoping is doing real work.
3. **A row must anchor the sentence, never an identifier the compiler already checks.** A comment
   pass deletes comments, not code — so any pattern a live identifier satisfies passes
   unconditionally and certifies nothing. A proposed row matching `ROW_PILL_RADIUS` was rejected for
   exactly this: the constant is live in `examples/finder_demo/list.rs`, so the row could never go
   red. The shipped row for that fact anchors `ideal quarter.disc` instead.
4. **Both arms are self-tested, in opposite directions.** `./scripts/anchors.sh --self-test` strips
   **one file's** comments at a time and requires every row naming that file to go **LOST** — every
   other file stays, so a duplicate of the phrase elsewhere cannot keep it green. This arm was itself
   vacuous until it was fixed and proven live; § 2 tells that story, and it is the reason to trust the
   green now rather than to assume it.

The mirror is the **`retired` arm**: a fixed limitation must not outlive its fix. Its claims are the
`gone` lines in `retired()` — currently ⌘ chords never reaching a hosted field, caret keys being the
window's own gap, editing needing an upstream gpui change, the shadow following content alpha, the
drawable default standing on cost. Its self-test plants each sentence behind another directory and
requires it to be caught as **STALE**. **Add a `gone` line whenever a fix retires a documented
limitation** — and phrase the pattern against the wording that is in the tree *now*, not the wording
you remember, or the arm asserts absence of a sentence that was never there.

**A file move breaks a row until its path is updated here. That is deliberate** — a guard that
adapts silently to a move is the thing being guarded against.

History worth knowing: until commit `b073e2c` the live arm ran `anchors src`, so it **only ever read
`src/`**. Stripping every comment from all twelve `examples/finder_demo/` modules left the guard
exiting 0 with zero LOST rows — every measured fact in an example, which is most of the Finder work,
was unguarded. Paths are now repo-relative and the self-test copies both trees.

**Do not edit `scripts/anchors.sh` while other agents hold the worktree.** Send proposed rows to the
main agent.

---

## 4. Platform facts you will otherwise rediscover the hard way

Each of these is a doc comment in the tree, most of them anchor-guarded.

**Presentation lag.** GPUI's presented Metal surface trails the `CATransaction` that carried the
same frame's layout by `CAMetalLayer.maximumDrawableCount - 1` frames. Writing a hosted view its
*current* layout therefore puts it ahead of its own GPUI row by that many frames of velocity —
measured as exactly **2.000 frames** at 4, 8 and 16 points per frame, the same integer multiple on
every row of every capture. `NativeHost::place` writes the **presented** frame's layout instead,
which reads 0.000 at those velocities and 0.000 at rest after an abrupt stop
(`native_root`, `src/bridge/root.rs`; `NativeHost::place`, `src/bridge/host.rs`). It is an integer
and not a fraction because the cause is presentation *order*, not easing: wrapping each write in its
own `CATransaction` makes it **worse** by a frame, because a nested commit lands after the one the
layout came in on. `NG_SKEW` in `examples/gallery` re-measures both arms; `NG_LAG=0` is the
uncompensated one.

**The menu bar and GPUI's keymap are exclusive.** AppKit offers a ⌘-chord to the key window's view
hierarchy first and to the menu bar **only after the window declines**, so GPUI's keymap wins every
chord it binds and the bar never sees it. The standard set therefore binds **nothing** into GPUI's
keymap — these chords are AppKit's. The rule an app author needs, and it is not guessable: **binding
a chord in GPUI takes it away from macOS.** Bind ⌘W in your keymap and Close stops closing
(`src/bridge/menus.rs`). A GPUI app has **no** menu bar until `install_menus` asks for one, so until
it does ⌘W, ⌘Q, ⌘M, ⌘H and the whole Edit menu reach nothing at all. Rows carry AppKit's own
selectors and **no target**, so macOS supplies the key equivalent, its localisation, the
enable/disable logic and the Show/Hide title flips.

**The two layers** (`src/lib.rs` § *Two layers*):

* `Layer::Above` — subview of GPUI's Metal view. Draws over GPUI content **and takes input**.
  Controls live here; macOS 26 glass placed here samples and blurs GPUI's own rendering.
* `Layer::Below` — sibling *beneath* the Metal view, so GPUI text and icons composite **over** it
  and **nothing here can receive input**. Window backgrounds and sidebar materials live here, and it
  needs a non-opaque window.

**Window options** (`window_options`, `src/bridge/root.rs`). `Transparent` makes `NSWindow.isOpaque`
false, which is the difference between a `Layer::Below` material showing the desktop and it
rendering flat grey; a real `NSSplitViewItem` sidebar needs it too. It costs nothing when no material
is placed — opaque GPUI drawing is bit-identical either way. `appears_transparent` becomes
`NSFullSizeContentViewWindowMask`, so **GPUI's content view covers the traffic lights and any
toolbar**: a full-bleed opaque background hides them completely. Inset opaque chrome by the live
titlebar band read as `frame.height - window.contentLayoutRect().height` and **never**
`frame.height - contentRectForFrameRect(frame).height` — under this style mask the content rect *is*
the frame, so the latter reads a constant 0.00 while the band is really 32pt or more. The drop
shadow does **not** come from what you paint: an unpainted band reaching a corner leaves the shadow
byte-identical.

**A component's value must be the state its `on_change` writes.** A component pushes its value on
every paint, so a literal — `native_text_field("name", "")` wired to an `on_change` that stores
elsewhere — wipes the control a frame or two after each keystroke, once the pending write drains. It
reads as intermittent input loss rather than as a wiring mistake (`src/lib.rs`).

**Materials are key-dependent; pin them Active.** At AppKit's default `followsWindowActiveState`, a
non-key `NSVisualEffectView` collapses to a flat fill (`src/materials/material.rs`). The sidebar
backdrop is pinned `Active` deliberately: one that flattened the moment the window stopped being key
would take the whole window with it (`src/chrome/sidebar.rs`).

**Long lists must use `uniform_list`,** not `div().overflow_y_scroll()`. Only visible rows place a
view, so the pool stays bounded however long the list is: a 400-row list with a switch and a slider
per row holds `NativeHost::len` at **39** under `uniform_list` and **811** under a plain scrolling
`div` — the whole list materialised, past the switch budget and far past the slider one. The bound
holds under motion, not just at rest (`src/lib.rs`).

**Budget**, per hosted view per frame, and only for views whose geometry moved — the host skips
`setFrame` when it has not, which is why a static screen costs nothing: plain view ~1.2 µs, switch
~3.5 µs, slider ~16 µs. Hundreds of switches are comfortable, thousands reachable; a couple of dozen
*moving* sliders is the ceiling. Materials cost more and it lands in the **WindowServer** rather than
this process, so a CPU measurement calls them free — budget a couple of dozen `withinWindow`
materials or a few dozen glass panels per 120 Hz frame, and reach for `glass_container` when there
are many (`src/lib.rs`).

**Never diff a property AppKit normalises or owns.** `NSPopUpButton.integerValue` is cell state, not
the selection; `keyEquivalent = "\r"` reads back as the empty string; `_setCornerRadius: 0` reads
back as the system default; a private glass style reads back as *regular*. Compare against your own
last-written intent, or ask the object that owns the fact. Each of these has an anchor row.

**Read a control's value synchronously inside the AppKit action, but dispatch the handler deferred.**
GPUI holds its `App` borrow at fire time and a synchronous update silently drops the event
(`src/bridge/host.rs`).

---

## 5. House rules

* **Comments earn their place by recording a non-obvious measured platform fact** a reader would
  otherwise simplify into a bug. That is the whole premise of the anchor table. No measurement
  tables, no hardware provenance, no session history, no agent names.
* **Never reference another repository** in committed code or docs — no names, paths, links or PR
  numbers. Land findings as this crate's own measured facts.
* **Conventional Commits**, lowercase imperative subject, one concern per commit — the only types in
  this history are `fix feat docs test refactor build chore`. **No AI-attribution trailers**: there
  are zero in this history and it stays that way.
* **Commit locally. Never push.** The owner pushes.
* **The worktree may be shared with other agents.** Commit with `git commit -o <paths> -m …` and
  never `git add -A` — a plain commit publishes whatever a sibling left in the shared index, which
  has already happened once and forced a history rewrite.
* Edition is **2021** (`Cargo.toml`), so let-chains do not compile. `rustfmt.toml` pins
  `style_edition = "2024"`; without it `cargo fmt` reorders every import block.
* `private-api` is **off by default**, and `custom_glass` / `glass_variants` carry
  `required-features`, so a default `--examples` build **skips them by construction**. Both configs
  must build.

## 6. Where the numbers live

### Facts deliberately kept out of source

Two of the measurements in § 1 are **not** in any doc comment, and that is a decision, not an
oversight. Do not "fix" it by landing them:

* **The real-Finder values** — (29,29,28), and 27 key / 41 and 46 non-key — are measurements of
  *Apple's app*, not facts about this crate's code. A comment in our tree asserting what Finder's
  pixels are would rot the next time Apple ships a release, and nothing in our build would notice.
* **The 37/33-unit step** describes a defect that no longer exists. It is a before-value; it lives in
  the message of commit `6a6a610`, which is the right place for a before-value.

What survives in source is the **mechanism**, which is what a future reader actually needs, and it is
anchor-guarded: a material's value tracks whatever it samples while a colour's does not
(`UnderWindowBackground` reads 38 over flat grey 29 and 44 over grey 60; `windowBackgroundColor`
reads 30 over both), plus the key-state confirmation 43 → 55 in dark — `tokens::WINDOW_BACKGROUND`.
The cost of the tempting wrong fix is on `SidebarOptions::backdrop` (40 → 27 in the band, 43 → 32 in
the interior). `NSWindow.backgroundColor` being inert is on `window_options`.

This file is therefore the only greppable home for the Finder values. Keep it that way, and keep them
labelled as a measurement of another app on one macOS release.

### Facts with no home in the tree at all

The `-l`-without-`-o` shadow margins (112/112/76/148) and the alpha-bbox rule are session
measurements that nothing in `src/` or `scripts/` records; `scripts/uidiff.py` documents the `-o`
half of the rule only. They are true of one window on one display and are here, in § 2, because a
comment could not have been anchored to anything.

If you measure something that a future reader would otherwise simplify into a bug, and it is a fact
about **this crate's own code**, land it as a doc comment and add an anchor row. If it is a fact
about another app, another release, or a defect you just removed, it belongs here or in the commit
message — not in a comment that nothing can keep honest.
