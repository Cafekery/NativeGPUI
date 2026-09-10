# Testing and tools

Nothing here is a unit test of arithmetic. The claims this crate makes are about pixels, paint order, focus and timing, so the checks drive real windows and read the results back out of AppKit or the window server.

## The gate

`./scripts/check.sh` is what "done" means. In order:

| step | what it proves |
|---|---|
| build (default) / build (private-api) | both feature configurations compile |
| formatting | `cargo fmt --check`, with rustfmt's stderr treated as a failure |
| unit tests | `cargo test --lib`: clipping arithmetic, colour conversion, token caches |
| `z_order` | after `commit`, each container's `subviews` and the key view loop equal paint order |
| `glass_hit` | a `glass_container` declines mouse points that belong to something else; a member still claims its own |
| `decoration_hit` | `material`, `native_label`, `native_rule` decline points that belong to GPUI; a switch under them still claims its own |
| `tabular_figures` | `system_font_tabular` shapes digits to equal advances |
| docs resolve | `cargo doc` under `-D warnings` |
| `ngdispatch` | a hosted control reaches its GPUI handler (exit 0), a cleared handler stays silent (exit 90), and the sabotaged arm really fails (exit 1) |
| `surface.sh` | four type-check arms against the built rlib: every promised public path resolves, removed paths stay removed, and both controls go the other way |
| no warnings | `src/` and `examples/` build with zero warnings |

Each probe is an example under `examples/` with its verdict and exit code in the first line of its doc comment. They open small unfocused windows, so the session must be unlocked.

## Scripts

| script | use |
|---|---|
| `bundle.sh <example> [features]` | builds a signed `.app` in `dist/`. A bare Cargo binary is never made key by `activate`, so anything about focus, key equivalents or the menu bar runs from a bundle: `open -n dist/Storybook.app` |
| `winlist.sh [pid]` | the window server's view of what is on screen: number, owner pid, layer, geometry, owner, title. The only reliable "is it really up / really gone" |
| `lockguard.sh` | exits 90 if the display is locked or asleep. Run it before and after anything that produces a number; a 90 voids the numbers |
| `keys.sh` | the keyboard suite: real keystrokes through System Events into a bundled window, results read back from AppKit |
| `perf.sh` | frame cadence: the bundled Finder demo under a 125 Hz synthetic trackpad train and a right-edge resize drag, unique on-screen frames per second read back from the window server, with minimums |
| `menubar.sh` | reads the menu bar back out of a second process through Accessibility |
| `surface.sh` | the public-surface arms, see above |
| `uidiff.py a.png [b.png]` | structure (rows, columns, edges) out of window captures |

## Signing and privacy prompts

TCC stores a privacy grant against the app's designated requirement, so an ad-hoc bundle is granted
per build: its requirement is `cdhash H"..."`, which every rebuild changes, and Desktop, Documents,
Downloads and "data from other apps" ask again. `bundle.sh` signs with the first identity
`security find-identity -v -p codesigning` offers, Developer ID before Apple Development, and with
nothing to offer it makes a `NativeGPUI Dev` certificate in a keychain of its own and signs with that
(`NG_CODESIGN_IDENTITY` overrides). Either way the requirement becomes identifier-plus-certificate and
outlives the build. Ad-hoc is the last resort when no identity can be had, and it costs a prompt per
build. The Info.plist carries the usage strings for what the Finder demo reaches: Desktop, Documents,
Downloads, removable and network volumes, file-provider domains, and `NSAppDataUsageDescription` for
"data from other apps".

The grant follows the identifier and the certificate in the signature, never the file name, so copy a
bundle (`ditto "dist/Found'er.app" /tmp/ngv/Probe.app`) instead of re-bundling or re-signing it: a copy
shares the grant, a hand-made or ad-hoc bundle is a new app and asks again. A test launch that must not
ask at all gets a home of its own:

```sh
open -n --env HOME=/tmp/ngv/fixtures --env NG_NO_CRAWL=1 "dist/Found'er.app"
```

## Capturing the screen

`screencapture -x -o -l <windowid>` is the only sanctioned capture. Never `-R`, never full screen. A window capture composites over nothing behind the window, so a material sampling the desktop reads flat; compare key against non-key instead. `screencapture` writes Display P3; compare saturated colours in one space.

## Environment knobs

| variable | effect |
|---|---|
| `NG_PAGE=<id>` | the storybook opens on that page |
| `NG_ROWS=<n>` | row count of the gallery's scroll list (default 400) |
| `NG_QUIT=<frames>` | the Finder demo exits after that many frames |
| `NG_LABEL_HOSTED=0` | the label page renders GPUI text instead of `NSTextField`s |
| `NGD_SABOTAGE=nohandler\|noclear` | the failing arms of `ngdispatch` |
| `NG_CRAWL_TRACE=1` | the index writes every folder it walks, every refresh with its cost, every rescan FSEvents asks for, the catch-up plan and each save to stderr |

## Index benchmarks

`crates/finder-fs/examples` holds the rigs for the whole-Mac index. Each prints wall time, CPU seconds and memory footprint.

| example | measures |
|---|---|
| `index_probe <store> [crawl\|sync root…]` | loading a snapshot, then a cold crawl or one launch's catch-up, then the first search |
| `index_probe <store> follow root…` | the live follower under whatever churn the disk sees, every 5 s for `NG_FOLLOW_SECS` (60) |
| `launch_bench <root> <store> <mode>` | one launch; several at once on one store show one owner walking and the rest reading |
| `replay_probe <event id> root…` | what FSEvents replays since an id: batches, folders, rescans, and when history ends |
| `index_bench` | a cold crawl of `/Applications`, `/usr`, `/Library` and `~/Developer`, read latency during it, save, load and search |

A cold run needs both caches gone: delete the store and run `sudo purge` to empty the file system cache. Crawl a fixed tree rather than the live disk so runs compare, and run anything heavy on a machine nobody is using: a crawl of a whole Mac takes minutes of CPU.

## Frame rate

The storybook header shows two numbers: frames rendered in the last second and the longest gap between two of them. Scrolling any page on a ProMotion display should read 120 fps with a worst gap near 8 ms; a worst gap of 16 ms or more during a steady scroll is a dropped refresh.

How the scroll timing was established, so the next person does not have to rediscover it: the trackpad reports at 125 Hz (`ioreg -l | grep ReportInterval` on the trackpad: 8000 µs) while the display refreshes at 120 Hz. With frames drawn only at the display-link tick, the two clocks cross five times a second and each crossing produces a frame with no scroll event followed by one with two: a stall and a double step. The app-side symptom is measurable without any screen capture: log the time of every drawn frame and every scroll event (a listen-only `CGEvent` tap) and count events per frame; the fix makes the histogram 1:1. Screen-side, ScreenCaptureKit window capture at 120 Hz drops about 6 % of composites itself, so use it at 60 Hz and compare per-sample displacement against the number of refreshes the sample spans, never raw unique-frame counts.

## Verifying a claim

A single reading consistent with two states is evidence for neither. Vary an axis and carry a known-positive through the same instrument: a probe that cannot fail proves the probe. Cleanup needs the window server, not just `ps`.
