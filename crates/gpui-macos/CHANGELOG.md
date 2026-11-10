# Changelog

## Unreleased: roll to zed-industries/zed 90d8514

From zed-industries/zed `2117a376b97104049f60e843afa92f18fa7f5748` (main, 2026-10-05)
to `90d8514059561f0e44af0a32e84e1c329cef3232` (main, 2026-10-09), 112 commits.
Confirmed on zed `main` (`main` was 12 commits ahead of this tip and 0
behind). All git pins in the root `Cargo.toml` and in `crates/native-gpui`
move to the same revision. rustc stays 1.98.1; `futures` stays 0.3.34;
`strum` stays 0.28. Vendored `crates/gpui` / iOS are not part of this roll.

This crate's features follow upstream `crates/gpui_macos/Cargo.toml`.
`font-kit` forwards to `gpui_apple/font-kit` instead of a direct
`font-kit` dependency. `screen-capture` also enables optional
`objc2-core-graphics`, `objc2-core-media`, and `objc2-screen-capture-kit`.
Dropped from this crate: `core-text`, `core-video`, `pathfinder_geometry`,
and the direct `font-kit` git dependency. Added `objc2-core-video` 0.3.2
(`CVBuffer`, `CVImageBuffer`, `CVHostTime`). `dispatch2` is 0.3 with
default features off and `std`, `block2`, `objc2`. `log` is 0.4 with
default features off and `std`. `uuid` is 1.1 with `serde`. `anyhow` is
1.0.86. `objc2-foundation` drops `NSBundle`, `NSNotification`, and
`NSUndoManager` from its direct feature list (AppKit's `NSImage` still
enables `NSBundle`; `gpui_apple` still enables `NSNotification`).
`mach2` 0.5 remains a macOS dev-dependency for `examples/idle_frame.rs`.

Upstream `main` commits touching `crates/gpui_macos` in this range:

- d544470 Reduce the dependency footprint (`#65176`). `font-kit` and the
  screen-capture objc crates become opt-in.
- 285d975 Improve Zed's startup time (`#65170`). `activate` marks a
  hidden window's view needs-display before `makeKeyAndOrderFront`.
- f495dd0 Track connected displays and each window's display (`#65059`).
  `MacDisplay::all` logs and yields nothing when the display list fails,
  instead of panicking. `PlatformDisplay::refresh_interval`,
  `Platform::on_displays_changed`, `PlatformWindow::on_display_changed`.
- 95d35c6 Share CoreText, Keychain, and thermal services (`#65058`).
  `open_type.rs` and `text_system.rs` leave this crate.
  `gpui_apple::AppleTextSystem`, `keychain`, and `thermal`. Display-link
  host time moves from the `core-video` crate to `objc2_core_video`.
- 90d8514 Fix blurred window backgrounds on macOS 27 (`#65361`).
  `BlurredView` uses `NSVisualEffectMaterial::UnderWindowBackground` and
  sets `NSVisualEffectBlendingMode::BehindWindow`.

Through the `gpui` pin:

- 1dd9bbf Hash a `GlobalElementId` once, when it is built (`#64209`).
  The struct's fields are private.
- 4f6d97b Measure frame pacing against each window's refresh rate
  (`#65351`). `Window::refresh_interval`.
- 679ad7c `uniform_list` lays out far-down items in f64 (`#65288`).
  Callers still pass f32 item heights.
- a2a92fe `gpui_platform`'s default features now include `multithreaded`,
  forwarded to `gpui_web` (`#65289`).

Kept: `patches/0001` through `patches/0006`. `patches/0007` is on this tip
too, re-applied after the roll.

- 0001 conflicted in the cocoa import block with `#65361`'s
  `NSVisualEffectBlendingMode`. Kept the blending mode and the patch's
  PNG/TIFF pasteboard types. `UnderWindowBackground` stays.
- 0002 and 0003 applied cleanly. 0002's callbacks keep the frame-journal
  `signal_at` / `signal_source`. 0003 still runs the finish-launching
  callback right after `finishLaunching`, after the accessory policy.
- 0004 conflicted in `gpui_macos.rs` where `#65058` removed
  `pub(crate) use text_system::*`. Kept the `on_first_present` export.
  Did not restore `text_system`; the module is gone. `platform.rs` and
  `window.rs` applied cleanly.
- 0005 applied cleanly. `#65170`'s redraw-before-reveal stays in
  `activate`. While `initial_show_pending`, `displayLayer` still returns
  without drawing, and `show_initial_windows` still orders and requests
  the first frame in one `CATransaction`. `#65059`'s
  `display_changed_callback` is untouched. Drop still stores `closed`,
  then takes the accesskit adapter.
- 0006 applied cleanly. `is_running` follows the frame journal's
  `signal.take`. Host time stays on `objc2_core_video`. `frame` still
  stops an idle link and still passes `signal_at` / `signal_source`.

Regenerated patch files apply onto the 90d8514 verbatim `src/` and
reproduce this tip.

Added `patches/0007-launch-callback-pool.patch` (re-applied from a9ed59c,
which this roll dropped): the launch work 0003 runs before `run` drains
its own autorelease pool, so a window opened in the finish-launching
callback is freed when it closes instead of staying in the window server,
and in `window_stack`, until the app quits.

Type-checked on Linux for `aarch64-apple-darwin`: `gpui_macos` lib, its
tests, and its examples (including `idle_frame`), with default features
and again with `font-kit,screen-capture`. `native-gpui` too. gpui_apple
`shaders.metallib` stubbed empty (the library build, the test build, and
`--features font-kit,screen-capture`). Nothing was executed. Linux cannot verify blurred
backgrounds on macOS 27, redraw-before-reveal against deferred initial
show, display connect/disconnect, idle display-link stop, synthetic-drag
stop after native menus, Spaces restore, tiled Fill, drag after objc2,
visibility/occlusion, traffic-light fullscreen exit, mouse tracking /
inactive hover, accessory activation policy, frame-journal timestamps, or
Metal presented-handler timing.

In-repo `crates/native-gpui` pins `gpui` and `gpui_platform` to the same
90d8514 revision as this roll (path-patched `gpui_macos`).

## Unreleased: roll to zed-industries/zed 2117a37

From zed-industries/zed `d3ccd5719486b39b6c18cf37a06233ef9833d6d8` (main, 2026-09-28)
to `2117a376b97104049f60e843afa92f18fa7f5748` (main, 2026-10-05), 112 commits.
Confirmed tip of zed `main` at start of work (`refs/heads/main` was that
SHA). All git pins in the root `Cargo.toml` and in `crates/native-gpui`
move to the same revision. rustc stays 1.98.1; `futures` stays 0.3.34;
`strum` stays 0.28. Features are unchanged. Vendored `crates/gpui` / iOS
are not part of this roll.

This crate's library dependency list: dropped `async-task`, `libc`, and
`mach2` (they left with the dispatcher). Added `core-video` 0.5.2
(`metal` feature) and `scheduler` (same zed revision). `dispatch2` stays
0.3.1. `mach2` 0.5 remains a macOS dev-dependency for
`examples/idle_frame.rs`.

Upstream `main` commits touching `crates/gpui_macos` in this range:

- 40180d9c Share the dispatcher and Metal renderer with iOS (`#64990`).
  `dispatcher.rs` is gone. `MacDispatcher` / `MacActivity` are
  `gpui_apple::AppleDispatcher` / `AppleActivity`. macOS drawing is
  unchanged except the test-only screenshot path, which uses shared
  storage on Apple Silicon.
- cd4fc8de Journal platform frame requests and skipped frames (`#64958`).
  Display-link, `displayLayer:`, and key-status frames pass
  `RequestFrameOptions { signal_at, signal_source }`. The link records
  the CoreVideo host time on `PlatformFrameSignal`.
- f37989fb Cross-platform headless windowing API (`#64969`). macOS maps
  headless to `ActivationPolicy::Accessory` (no Dock icon or menu bar)
  and windowed to `Regular`. Accessory is applied before the run loop;
  `Regular` stays in `did_finish_launching`.

Through the `gpui` pin:

- `Application::with_windowing`, `App::request_windowing`,
  `App::set_activation_policy`, `App::graphical_environment` (`#64969`).
  `Platform` methods have defaults.
- `KeystrokeEvent::input_preference` (`#64859`).
- `App::start_hang_monitor` (`#64944`). `App::is_test`.
- Idle refreshes no longer seal empty journal intervals (`#65057`,
  follow-up to `#64958`).
- `test-support` is safe to enable in every build (`#65043`).
- Redundant `flushing_effects` reentrancy guard removed (`#65016`).

Through the `gpui_apple` pin: `#64990` (dispatcher, Metal `from_layer`,
crate now `cfg` macOS or iOS) and `#65043`. Through `gpui_platform`:
`linux(allowed_modes)` and `bench_text_system` (`#64969`, `#64842`).
`collections` and `gpui_util` are quiet.

Kept: `patches/0001` through `patches/0006`. 0001–0003 and 0005 applied
cleanly. 0002's callbacks keep the frame-journal `signal_at` /
`signal_source`. 0004 conflicted in `MacPlatform::run`: the
`sharedApplication` trace stays around that call, then `#64969`'s
accessory policy. 0006 conflicted in `frame`: the idle display link
still stops, and the callback still drains `WindowFrameSource::take_signal`.
Regenerated patch files apply onto the 2117a37 verbatim `src/` and
reproduce this tip.

Type-checked on Linux for `aarch64-apple-darwin`: `gpui_macos` lib, its
tests, and its examples (including `idle_frame` and the display-link
host-time tests), clippy (8 warnings, all in code that came from
upstream), and `native-gpui`. gpui_apple `shaders.metallib` stubbed
empty. Nothing was executed. Linux cannot verify synthetic-drag stop
after native menus, Spaces restore, tiled Fill, drag after objc2,
visibility/occlusion, traffic-light fullscreen exit, mouse tracking /
inactive hover, idle display-link stop, accessory activation policy,
frame-journal timestamps, or Metal presented-handler / screenshot-path
timing.

In-repo `crates/native-gpui` pins `gpui` and `gpui_platform` to the same
2117a37 revision as this roll (path-patched `gpui_macos`).

## Unreleased: roll to zed-industries/zed d3ccd57

From zed-industries/zed `bcf6582ce3500df93a8a39366640173e6786cea6` (main, 2026-09-21)
to `d3ccd5719486b39b6c18cf37a06233ef9833d6d8` (main, 2026-09-28), 93 commits.
Confirmed tip of zed `main` at start of work. All git pins in the root
`Cargo.toml` move to the same revision. rustc stays 1.98.1; `futures`
stays 0.3.34; `strum` stays 0.28. Features and this crate's dependency
list are unchanged. Vendored `crates/gpui` / iOS / native-gpui are not
part of this roll.

Upstream `main` commits touching `crates/gpui_macos` in this range:

- 382ff7a6 Stop synthetic mouse drags after button release (`#64175`).
  `synthetic_drag` checks `NSEvent.pressedMouseButtons` and stops when
  the original button is no longer physically down, and when the window
  is gone. Native menu tracking can swallow the mouse-up; replaying the
  stale drag after that flickered hover.

Through the `gpui` pin:

- ba42ab9d Keystroke interceptors receive standalone modifier keystrokes
  (`#64537`).
- e71963a5 `WrappedLineLayout::index_for_position` no longer panics when
  hit-testing past a wrapped line's width (`#64672`).
- e9ddcdaf `From<u64>` for `ElementId` (`#64702`).
- 9183ba4c Query whether a11y is forcefully disabled for the app
  (`#64365`).
- bda9c0bd `TextLayout::paint_background` / `paint_foreground` (`#64788`).
- e683fd7b `bench_metrics` for multi-metric Criterion benches (`#64753`).

Through the `gpui_apple` pin: `96e95eda` `#64623` skip sprites whose
atlas texture was released (crash fix), and `b89471c5` `#64641` drop
`block` as a direct dependency. Through `gpui_platform`: `e91b82c1`
`#64718` wgpu Linux headless. `collections` and `gpui_util` are quiet
(`#62473` is this range's old pin).

Kept: `patches/0001` through `patches/0006`. All six applied cleanly.
Regenerated patch files apply onto the d3ccd57 verbatim `src/` and
reproduce this tip.

Type-checked on Linux for `aarch64-apple-darwin` only (gpui_apple
`shaders.metallib` stub). Linux cannot verify synthetic-drag stop after
native menus, Spaces restore, tiled Fill, drag after objc2,
visibility/occlusion, traffic-light fullscreen exit, mouse tracking /
inactive hover, idle display-link stop, or Metal presented-handler
timing.

In-repo `crates/native-gpui` now pins `gpui` and `gpui_platform` to the
same d3ccd57 revision as this roll (path-patched `gpui_macos`).

## Unreleased: idle display link

- `patches/0006-idle-display-link.patch`: a window with nothing to draw
  stops its display link, and gpui's frame waker and `schedule_frame`
  restart it, ticking on the next main-queue turn. A visible idle window
  no longer wakes the main thread every refresh. `examples/idle_frame.rs`
  checks both halves on a Mac with a display.

## Unreleased: roll to zed-industries/zed bcf6582

From zed-industries/zed `250b6581b5b346855cccfb909839d47711cf910b` (main, 2026-09-14)
to `bcf6582ce3500df93a8a39366640173e6786cea6` (main, 2026-09-21), 116 commits.
Confirmed tip of zed `main` at start of work. All git pins move to the same
revision. rustc stays 1.98.1; `futures` stays 0.3.34. `media`, `block`, and
`metal` leave this crate's dependency list. New: `objc2-core-graphics`,
`objc2-core-media`. `objc2-app-kit` gains `NSTrackingArea`;
`objc2-screen-capture-kit` gains `dispatch2`, `objc2-core-media`, `SCStream`.

Upstream `main` commits touching `crates/gpui_macos` in this range:

- 0eda7703 Release the accessibility adapter when a window is dropped
  (`#64143`). `MacWindow::drop` takes `accesskit_adapter` so the
  content-view retain cycle does not keep the native view / `CAMetalLayer`
  alive.
- aec7395e Fix cargo shear warnings (`#64340`). Unused deps (including
  `metal` here) leave the manifest.
- 613a80b9 Fix traffic lights animation during fullscreen exit (`#64339`).
  `is_exiting_fullscreen` holds the buttons until the exit animation
  finishes.
- 4b47ceb9 Migrate screen capture to objc2 (`#64251`). Drops the `media`
  crate and the `block` crate from this package; `screen_capture.rs` uses
  `objc2-screen-capture-kit` / `objc2-core-media`.
- f6838a7c Fix hover feedback when another application is focused
  (`#64301`). Normal windows use `ActiveAlways` tracking, matching popups.
- db7f9cee Improve mouse tracking for normal windows (`#64234`). View
  tracking area; `acceptsMouseMovedEvents` off for normal windows so a
  covered GPUI window does not hover under another app.

Through the `gpui` pin:

- 0d08af1e Shared GPUI and scheduler implementations (`#64261`).
- 94c997e0 Debug selectors include cached views (`#64433`).
- 0ef92145 `PlatformAtlas::get_or_insert_with` takes an owned `AtlasKey`;
  shared `AtlasState` / `AtlasBackend` (`#64331`).
- b0f53ad2 Inspector runtime cost cut when the inspector is closed
  (`#64309`).
- f0fb48c7 `ForegroundJournal` aware of sleep and window visibility
  (`#64269`).
- 5c9efb75 Avoid main-thread waits when unlocking web task queues
  (`#64298`).
- 07df4386 Remove `ztracing` from GPUI (`#64237`).
- b68add5b / 9e6e1416 Dynamic font installation without a main-thread
  mutex (`#63498`, `#64204`); `App::on_missing_glyphs`.
- f792c2d7 `ShapedLineCursor` (`#64185`).
- cf4deb27 `Bounds::centered` uses the display work area (`#62073`).

Through the `gpui_apple` pin: `#64331` (owned atlas key / `AtlasState`)
and `#64340` (cargo shear). Through `gpui_platform`: `#64309`
(`inspector_render` bench) and `#64340`. Through `gpui_util`: `#62473`
(custom Scoop paths; this is R) and `#64340`. `collections` is quiet.

Kept: `patches/0001` through `patches/0005`. 0001–0004 applied cleanly.
0005 three-way-conflicted in `MacWindow::drop`: keep the patch's
`closed.store` (deferred show must not touch a dying window) and
upstream's `accesskit_adapter.take` (`#64143`). Regenerated patch files
apply onto the bcf6582 verbatim `src/` and reproduce this tip.

Type-checked on Linux for `aarch64-apple-darwin` only (gpui_apple
`shaders.metallib` stub; no `media` stub after `#64251`). Linux cannot
verify Spaces restore, tiled Fill, drag after objc2, visibility/occlusion,
traffic-light fullscreen exit, mouse tracking / inactive hover, or Metal
presented-handler timing.

## Unreleased: roll to zed-industries/zed 250b658

From zed-industries/zed `69164008341295ad481bb11c0334a712ca8c23e3` (main, 2026-09-07)
to `250b6581b5b346855cccfb909839d47711cf910b` (main, 2026-09-14), 76 commits.
All git pins move to the same revision. rustc 1.97.1 -> 1.98.1; `futures` 0.3.32
-> 0.3.34. `objc2-screen-capture-kit` is a new dependency; `objc2-app-kit`
gains NSDraggingItem / NSImage / NSOpenPanel / NSPanel / NSPasteboard /
NSSavePanel / NSScreen / NSWorkspace.

Upstream `main` commits touching `crates/gpui_macos` in this range:

- c7801b0c Honor "Tiled windows have margins" for the Fill titlebar action
  (`_zoomFill:` with `zoom:` fallback).
- 33c6212b Restore windows to their original Space via AppKit restorable
  state (`encodeRestorableStateWithCoder:` / `restoreStateWithCoder:`).
- 45077524 `Platform::prevent_idle_sleep` (`NSProcessInfo`
  `beginActivityWithOptions`, RAII `ActivityGuard`).
- 907b55f7 `register_url_scheme` on objc2.
- d3865b09 `prompt_for_path` on objc2 (drops `block` from `platform.rs`).
- 86b2cf96 `get_sources` (screen capture) on objc2
  (`objc2-screen-capture-kit`).
- 72c53bf0 `start_external_drag` on objc2 (`NSDraggingItem` /
  `NSPasteboardWriting`).
- 3db02c2c Window visibility (`NSWindow.occlusionState`) and
  `Platform::on_system_sleep` (`NSWorkspaceWillSleepNotification`).

Through the `gpui` pin:

- 20fa2fa8 Less LLVM IR bloat.
- b95b188b `TestWindow::simulate_scale_factor_change`.
- 58962741 Asset cache owns completed entries: `App::fetch_asset` now
  returns `Option<A::Output>` instead of `(Shared<Task<A::Output>>, bool)`.
- d9e1c024 Long-press activates tooltips.
- 5b4a2153 Font-family suggestions list only available families.
- 1a89a92e Unpaced frame-driven renderer sessions for benches.
- 7960b2a7 Web canvas font fallback (emoji / optional CJK).

Through the `gpui_apple` pin:

- fb38178d Headless Metal wraps each render in an autorelease pool
  (`#64099`). `gpui_platform` is quiet in this range.

Kept: `patches/0001` through `patches/0005`. 0001 three-way-conflicted on
the `std::sync` import (keep upstream `Once` for Spaces restore and the
patch's `AtomicU64`); incoming browser/promise/image drags are intact and
outgoing `start_external_drag` stays the objc2 path. 0002–0005 applied
cleanly. Regenerated patch files apply onto the 250b658 verbatim `src/`
and reproduce this tip.

Type-checked on Linux for `aarch64-apple-darwin` only (media /
gpui_apple build-script stubs). Linux cannot verify Spaces restore, tiled
Fill, drag after objc2, visibility/occlusion, or Metal presented-handler
timing.

## Unreleased: frames with input

- `patches/0002-frames-with-input.patch`: the frame an input event dirties is drawn inside the event and a tick within 8 ms of it stands aside (also during a live resize; a bounds-change frame stands in for the following input frame); every frame presents with the transaction and flushes it.

## Unreleased: roll to zed-industries/zed 6916400

From zed-industries/zed `d56dca8084e88c06df35a44ee103f41d21d06e10` (2026-09-02)
to `69164008341295ad481bb11c0334a712ca8c23e3` (main, 2026-09-07), 69 commits.
All git pins move to the same revision.

No upstream commit touches `crates/gpui_macos` or `crates/gpui_apple`, so
`src/` is byte-identical to the previous roll and the crate's dependency list
is unchanged (none of upstream's workspace version bumps are dependencies of
this crate). What changes arrives through the `gpui` pin:

- 3ce72bab `Hitbox::is_hovered_at(position, window)`.
- f0d8b0b0 `HoverListenerMode` and `hover_listener_mode(..)` on
  `Interactivity` / `StatefulInteractiveElement`: opt out of keyboard input
  suppressing `on_hover`.
- b1a7ef0c Padding snapped to the device pixel grid before clamping scroll
  (fixes sub-pixel scrollability at fractional rem sizes).
- b3326e13 `TouchDragEvent` and a new `PlatformInput::TouchDrag` variant.
- a85cf449 Touch mispredict jitter prevented in the gesture recognizer.
- d85eade7 `Window::pending_input()` / `set_pending_input_timeout_paused(..)`
  with `PendingInputStatus` / `PendingInputTimeoutStatus` (which-key
  indicator support); `key_dispatch.rs` reworked around it.
- 9decdcc1 `Window` now calls `PlatformWindow::text_input_state_changed`
  with `FocusGained`/`FocusLost` when the focused handler starts or stops
  accepting text. `MacWindow` keeps the trait default.
- 6b5e15ed `bindgen` 0.71 -> 0.72 in `gpui` and `media` build scripts.

Kept: `patches/0001-accept-browser-promise-and-image-drags.patch`, applied
three-way onto the 6916400 `window.rs` with no conflicts; the regenerated
body is byte-identical to the d56dca8 one.

## Unreleased: roll to zed-industries/zed d56dca8

From wingleeio/zed `a6c1ad501f90c9437d2553bde691958f150364c5` (2026-07-29) to
zed-industries/zed `d56dca8084e88c06df35a44ee103f41d21d06e10` (main, 2026-09-02).
Git pins for gpui, collections, gpui_util, media move to the same revision;
gpui_apple is a new pin.

Upstream `main` commits touching `crates/gpui_macos` and `crates/gpui_apple`
between the a6c1ad5 merge base (f14fea9b, 2026-07-19) and d56dca8:

- 914e1c98 Pasteboard custom type strings retained (`StrongPtr`).
- 2e2fb0a2 Unify dependencies.
- 8e4e5a39 `Platform::set_window_appearance` (app appearance follows theme).
- 06b6160d Drop the pre-macOS 12 blur path and its private CGS APIs.
- a11083f9 Appearance-change callback deferred.
- f52fd9ac, a8491e63 macOS file drag-out (`NSDraggingSource`, move vs copy).
- e99616cd `titlebar_double_click(is_resizable, is_minimizable)`.
- be8c6f9f Renderer resource management.
- 1271f8b0 rustc 1.97.
- 52b24181 Extract shared Apple renderer into `gpui_apple`. `build.rs`,
  `metal_atlas.rs`, `metal_renderer.rs`, `shaders.metal` leave this crate.
- 2893b86b, 242fe31a Simple fullscreen (borderless, covers the notch).
- 71507659, 7f2a2c3c `restart(binary_path, arguments)`, `on_quit` returns bool.
- fecc3273 `bench-support` feature; headless renderer exposed through it.
- ee6badf4 gpui_apple builds under corgi (shader compiled from a staged copy).
- a60addb9 `PlatformWindow::prompt` on objc2 `NSAlert`.

Not in this roll: a6c1ad5 was not on zed `main`. It was a wingleeio branch,
four commits past main at f14fea9b: a5d7d36b and a6bf17cf (no wrap before
closing punctuation), 14baea02 (EdgeFade), a6c1ad50 (BackdropBlur, horizontal
EdgeFade, macOS glass fixes). None were merged upstream, so gpui at d56dca8
has no `BackdropBlur`/`EdgeFade`, and `window.rs` no longer forces a 60pt blur
radius or a black fallback layer for blurred windows.

Kept: `patches/0001-accept-browser-promise-and-image-drags.patch`, re-applied
three-way onto the d56dca8 `window.rs`. All behavioral hunks apply; the one
dropped hunk was a line re-wrap inside the blur-radius block upstream deleted.

## Accept browser, promise and image drags

Registers `public.file-url`, `public.url`, PNG/TIFF data and the file-promise
types alongside `NSFilenamesPboardType`; promised files are fulfilled into a
temp dir, web URLs pass through `ExternalPaths` for the app to download, raw
image data is written to a temp file. Finder list stays first.

## 87ce812e Stand alone outside the Zed workspace

Workspace-inherited versions made concrete; zed crates pinned by git.

## 4510af11 gpui_macos from wingleeio/zed at a6c1ad5, verbatim
