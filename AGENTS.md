# Rules for working in this repo

`native-gpui` hosts real AppKit views inside a GPUI window on macOS 26. GPUI owns layout; macOS owns the pixels of anything whose design Apple changes between releases. [README.md](README.md) is the map; this file is the invariants.

## Code

Code carries a comment only if it is incomprehensible without one: a one-line platform fact a reader would otherwise "fix" into a bug. No history, no measurements, no rationale essays. Public items get at most a one-line doc where the name does not already say it.

Behaviour-preserving refactors only. Keep the public surface (`scripts/surface.sh` checks it), env-var names, printed verdicts and exit codes of the probes.

## One background

A window is one surface, not two: `SidebarOptions::backdrop` spans it and both panes show that material, so with `sidebar::NativeSidebar` the crate paints nothing and an app paints no background at all; only a sidebar installed with `backdrop: None`, or a window without a sidebar, is the opaque `tokens::WINDOW_BACKGROUND`. A `Layer::Below` material shows only where GPUI painted nothing. `NSWindow.backgroundColor` is inert here.

## Two layers

`Layer::Above` is a subview of GPUI's Metal view: it draws over GPUI content and takes input. `Layer::Below` is a sibling beneath it: GPUI composites over it and it can never receive input; it needs the non-opaque window `window_options()` gives.

## One gpui

`gpui` is a git dependency on its upstream repository at one revision; `[patch]` swaps `gpui_macos` for a fork built from that same revision. Never a crates.io `gpui`, never a vendored copy, never two revisions: `cargo tree -i gpui` shows one `gpui`. Apps build their `Application` with `gpui_platform::application()` and `gpui_platform`'s `font-kit` feature on; `MacPlatform::new` asserts the main thread.

## Presentation

Keeping a hosted view on the same frame as the GPUI row it sits on takes two changes to gpui: presenting the drawable inside the Core Animation transaction and flushing it every frame, and drawing the frame an input event dirtied inside that event rather than at the display-link tick (the trackpad reports at 125 Hz against a 120 Hz display; a frame drawn only at the tick alternates between zero and two events whenever the clocks cross). Neither is in upstream gpui at the current pin. They belong upstream or in the platform forks, never in this crate. The `gpui_macos` fork carries both as `patches/0002`.

## Platform facts

* The menu bar and GPUI's keymap are exclusive: binding a chord in GPUI takes it away from macOS. A GPUI app has no menu bar until `install_menus`.
* A component's value must be the state its `on_change` writes; a literal wipes the control a frame after each keystroke.
* Read a control's value synchronously inside the AppKit action; dispatch the handler deferred. GPUI holds its `App` borrow at fire time.
* Never diff a property AppKit normalises or owns (`keyEquivalent = "\r"` reads back empty, `NSPopUpButton.integerValue` is cell state). Compare against your own last-written intent.
* Materials are key-dependent; the crate pins them `Active`.
* Long lists must use `uniform_list`, so only visible rows place a view.
* Under `appears_transparent` the content rect is the frame: inset chrome by `frame.height - contentLayoutRect.height`.
* GPUI content over a blur of GPUI content needs a second window: one Metal layer is either under a material or over it. The Finder demo's preview pane is a `glass` frosting the rows in the main window, and a borderless, non-activating, transparent child window over it carries only the card.
* Cursor rects and GPUI's cursor styles apply only in the key window. A window that can never be key (the pane's child panel) gets its `cursor_region` cursor from the chrome host's own `mouseMoved:`.
* A hosted field in a window that is not key edits nothing, however it was made first responder: a field asked to edit takes key for its window first, and whoever put it in a child panel hands key back to the main window when the edit ends.
* GPUI's Metal layer carries CA's width/height-sizable autoresizing mask. AppKit already sizes a layer-backed view's layer from the view, so the mask only matters when the view does not fill its superview, and then CA adds the superview's resize delta a second time (old + 2x delta) and every frame displays scaled. The sidebar pane clears the mask when it adopts the view.

## Verifying

* A single reading consistent with two states is evidence for neither. Vary an axis; carry a known-positive through the same instrument.
* `screencapture -x -o -l <windowid>` is the only sanctioned capture: never `-R`, never full-screen. A window capture composites over nothing behind the window, so a material sampling the desktop reads flat; use key vs non-key instead. `screencapture` writes Display P3, so compare saturated colours in one space.
* `activate` does not make a bare Cargo binary key; bundle with `scripts/bundle.sh` and `open -n` for key-window work.
* Cleanup needs the window server, not just `ps`: `scripts/winlist.sh`.
* Run `scripts/lockguard.sh` before and after anything that produces a number; exit 90 means the numbers are void.

## House rules

* Conventional Commits, lowercase imperative, one concern per commit, no AI-attribution trailers. Push only when asked.
* The worktree may be shared: `git commit -o <paths>`, never `git add -A`.
* Edition 2021; `rustfmt.toml` pins `style_edition = "2024"`.
* `private-api` is off by default; both feature configs must build with zero warnings. `scripts/check.sh` is the gate.
* Never reference another repository in committed code or docs.
