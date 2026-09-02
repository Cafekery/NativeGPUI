# How native-gpui works

GPUI draws a whole window into one Metal layer. macOS draws its controls with `NSView`s. This crate puts real `NSView`s over (or under) GPUI's Metal view, at exactly the rectangles GPUI's layout computed, and keeps them there frame after frame. GPUI owns layout, identity and state; AppKit owns pixels, tracking and accessibility.

Read this top to bottom once; afterwards [`src/bridge/host.rs`](../src/bridge/host.rs) is the file to keep open.

## The view tree

```
NSWindow
└── contentView
    ├── NGContainer  "below"          Layer::Below: materials and glass that frost the desktop
    ├── GPUI's NSView (Metal layer)   re-classed as NGKeyPathView, see Keyboard
    │   └── NGContainer  "above"      Layer::Above: everything that draws over GPUI content
    │       ├── NGKeyViewTerminus     zero-sized; both ends of the Tab loop
    │       ├── NGContainer (clip) ── NSSwitch
    │       ├── NGContainer (clip) ── NSButton
    │       └── …
    └── NSSplitViewController's views, when a NativeSidebar is installed
```

`NGContainer` ([`bridge/appkit.rs`](../src/bridge/appkit.rs)) is flipped, so GPUI's top-left coordinates go into `setFrame:` verbatim, and it declines mouse points that land on nothing but itself, so a click on empty container space falls through to GPUI.

## One frame, start to finish

1. Your `Render` returns a tree wrapped once in `native_root(...)`.
2. **Layout.** A hosted element's `request_layout` asks AppKit for the control's `fittingSize` (cached per class by `fitting_size`) and reserves that box in GPUI's layout. No view exists yet.
3. **Paint.** `paint` receives a `Slot` (pool key, host, bounds, clip mask) and calls `slot.place(layer, |mtm, target| ...)`. The host looks the key up in the window's pool: on first sight it runs the closure to create the view; every time it diff-writes the clip frame, the view frame and the hidden flag. The element then pushes only the properties that changed (`if switch.state() != wanted { switch.setState(wanted) }`) and registers its handler.
4. **Commit.** `native_root` paints last and calls `NativeHost::commit`: views that were not placed this frame are removed, subviews are restacked to paint order if the order changed, and the key view loop is rewritten.
5. **Present.** GPUI encodes its Metal frame. The vendored gpui presents the drawable inside the same Core Animation transaction and flushes it, so the AppKit frame changes from step 3 and the Metal frame reach the screen in the same refresh.

Everything an element does is per frame and idempotent: there is no "create" phase to get wrong, and a component is one `layout` plus one `paint`.

## Identity: why a view survives scrolling

The pool key is a hash of GPUI's `GlobalElementId`, the path of element ids from the root. The same id next frame means the same `NSView`, so a text field keeps its text and first-responder status, a slider keeps its drag, and a switch scrolled through a `uniform_list` is the same switch on the way back.

A view is rebuilt when its layer, Rust type or parent changes, and dropped by `commit` when its element was not painted. Long lists must use `uniform_list`: only the visible rows place a view.

## Clipping

Each view sits in its own clip container sized to `bounds ∩ content mask`, so a control half scrolled out of a GPUI scroll container is cut exactly where GPUI cuts its own content. Because the clip is a real view frame, hit-testing is clipped too. An empty intersection hides the container instead of resizing the control to nothing.

## Events

Controls use AppKit's target/action. Every window has one `NGTarget`; `wire(control, target, key)` sets the control's tag to its pool key and points its action at `ngAction:`. When it fires:

1. The value is read **synchronously**, while the action is on the stack, through a `ValueKind` (`integerValue`, `doubleValue`, `stringValue`) or a custom `NativeRead` for values that only exist during the action (a clicked path item, a menu row's tag).
2. The GPUI handler runs **deferred**, on the next turn. An action can fire while GPUI already holds its `App` borrow, for example from inside a tracking loop, and a synchronous update would drop the event.

Handlers are registered every paint (`slot.on_change(kind, handler)`) and removed with the view. The rule that follows from "push the value every paint": the value you pass a component must be the state its `on_change` writes. A literal wipes the user's edit one frame after each keystroke.

## Keyboard and focus

- AppKit's first responder and GPUI's focus are independent. A mouse-down GPUI sees is one no hosted view claimed, so `native_root` hands first responder back to GPUI's view.
- AppKit's automatic key view loop cannot be used: it walks into hosted views' private subviews and cannot end at GPUI's view, which never accepts first responder. `commit` writes the loop instead: terminus → hosted views in paint order → terminus, and the terminus hands focus to GPUI. Tab leaves the hosted views in both directions.
- GPUI's view is re-classed to `NGKeyPathView` so that while a hosted control is first responder, `performKeyEquivalent:` and `keyDown:` behave like a plain `NSView`'s. GPUI's own overrides would otherwise swallow the keys the control needs.

## Layers

`Layer::Above` is a subview of GPUI's Metal view: it draws over GPUI content and takes input. `Layer::Below` is a sibling beneath it: GPUI composites over it and it can never receive input; it needs the non-opaque window `window_options()` sets up and a rect GPUI leaves unpainted.

A window is two surfaces: the sidebar pane is vibrancy and everything else is one opaque colour, `tokens::WINDOW_BACKGROUND`. That is also why the storybook's sidebar looks different from its body: it is the system's sidebar material, exactly as in Finder.

One thing a single window cannot do: GPUI content over a blur of GPUI content. GPUI is one Metal layer, so it is either entirely under a material (`Layer::Above`, which blurs it) or entirely over it (`Layer::Below`, which cannot see it). Finder's preview pane is both, a blur of the list with a card on top, and the Finder demo does it with a borderless, non-activating child window laid over the list: a `Layer::Below` material there samples the parent window behind it, and the child's own GPUI paints the card. See `PreviewPane` in [`examples/finder_demo/preview.rs`](../examples/finder_demo/preview.rs).

## Materials and glass

`material(id, Material::Sidebar)` hosts an `NSVisualEffectView`. `glass(id)` hosts an `NSGlassEffectView`; `glass_container` wraps neighbouring panels in an `NSGlassEffectContainerView` so they merge the way the system's do. Both are ordinary hosted elements, so they clip, scroll and stack with everything else. The `private-api` feature exposes the undocumented material values and glass variants the storybook's Glass Variants and Custom Glass pages show.

## Sidebar

`NativeSidebar::install(window, cx, options)` wraps GPUI's view in an `NSSplitViewController` with a real sidebar item: outline view, vibrancy, the toolbar toggle. Each frame you call `update(&items)`, which diffs against what the outline view shows and applies inserts, removes and moves. `metrics()` gives the insets to lay GPUI content out beside it; `on_select` reports the row.

## Menus

`install_menus(cx)` builds the standard bar with AppKit's own selectors and no target, so macOS supplies the key equivalents, localisation, enabling and the Hide/Show title flips. `add_menu` and `extend_menu` add your rows, each dispatching a GPUI `Action`. The bar and GPUI's keymap are exclusive: a chord bound in GPUI never reaches the bar.

## Surfaces

Popovers, sheets and the share picker are their own `NSWindow`s. `native_anchor(&anchor, child)` records where a GPUI element painted; `present_popover` and friends point at it.

## Tokens

`tokens::init(cx)` snapshots the system's colours and fonts into a `SystemTheme` global and republishes it when the appearance or accent changes. `system_color`, `system_font`, `sf_symbol` and `file_icon` are the accessors; every value is rasterised or resolved by macOS, never approximated.

## The vendored gpui

`vendor/gpui` is gpui 0.2.2 with three changes, applied through `[patch.crates-io]`. See the diff with `diff -ru ~/.cargo/registry/src/*/gpui-0.2.2/src vendor/gpui/src`.

1. **Present inside the transaction.** The Metal layer has `presentsWithTransaction`, and each frame is `commit`, `waitUntilScheduled`, `present`, followed by `[CATransaction flush]`. Without it GPUI's frame reached the screen 0–2 refreshes after the AppKit changes made alongside it, depending on how backed up its drawable queue was, and hosted views visibly trailed the rows they sit on.
2. **Input-driven frames.** The frame an input event dirties is drawn inside that event, not at the next display-link tick. The trackpad reports at 125 Hz against a 120 Hz display; a frame that only draws at the tick alternates between carrying zero and two scroll events whenever the two clocks cross, which reads as a stall followed by a double step. A tick within 8 ms of an input frame presents nothing new.
3. **A deadlock.** GPUI held its window lock across `resignKeyWindow`, which re-enters the same handler.

To move to a newer gpui: copy the release into `vendor/gpui`, reapply the diff, and run `scripts/check.sh`.
