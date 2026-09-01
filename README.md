# native-gpui

Real AppKit controls, materials and macOS 26 liquid glass hosted inside a **GPUI** window.

GPUI owns layout; macOS owns the pixels of anything whose design Apple changes between releases. A
hosted `NSSwitch` becomes the macOS 27 toggle without this crate being touched — anything drawn in
GPUI shaders is frozen on the day it was written (`src/lib.rs`).

No Swift, no build script, no C shim: GPUI's window vends its `NSView` through
`raw-window-handle`, and `objc2-app-kit` binds all of AppKit including `NSGlassEffectView`.

> **Working in here? Read [AGENTS.md](AGENTS.md) first.** It holds the invariants — the one-background
> rule, how to verify anything without fooling yourself, the anchor system, and the platform facts
> that have already cost hours. This file is only the map.

---

## How it fits together

`native_root` wraps your root element once per window, creates that window's `NativeHost`, and
commits every native view's geometry in one pass **after** your tree paints. Components
(`native_switch`, `glass`, `material`, …) are ordinary GPUI elements that reserve layout space and
place a pooled AppKit view into it. Views are keyed by GPUI element identity, so a component's
`paint` does three things: derive the pool key, `place` the view, push only the properties that
changed.

Two positions are available. `Layer::Above` is a subview of GPUI's Metal view and takes input;
`Layer::Below` is a sibling beneath it, so GPUI composites over it and nothing there can receive
input. See AGENTS.md § 4.

```rust
use gpui::{App, Application, Context, Render, Window, div, prelude::*};
use native_gpui::{install_menus, native_root, native_switch, window_options};

struct Root {
    on: bool,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // No .bg(): with a sidebar the crate paints the background; without one, paint
        // tokens::window_background() and nothing else. See AGENTS.md § 1.
        native_root(div().size_full().child(
            native_switch("wifi", self.on).on_change(cx.listener(|this, on: &bool, _, cx| {
                this.on = *on;
                cx.notify();
            })),
        ))
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        install_menus(cx); // ⌘W, ⌘Q, ⌘M, ⌘H live here and nowhere else
        cx.open_window(window_options(), |_, cx| cx.new(|_| Root { on: false }))
            .unwrap();
        cx.activate(true);
    });
}
```

The value you pass a component **must be the state its `on_change` writes** — a literal wipes the
control a frame or two later. AGENTS.md § 4 explains why.

## `src/` map

| path | owns |
|---|---|
| `lib.rs` | the crate's rules, in doc form: two layers, one background, budget, long lists |
| `bridge/` | the machinery that lets AppKit views live in a GPUI window — *you should not need to open this* |
| `bridge/host.rs` | the per-window view pool (`NativeHost`), `place`/`commit`, event delivery into GPUI |
| `bridge/root.rs` | `native_root` (the frame boundary) and `window_options` |
| `bridge/appkit.rs` | the whole ObjC surface: reaching GPUI's view, and one flipped container class |
| `bridge/menus.rs` | the standard menu bar — `install_menus`, `add_menu`, `extend_menu` |
| `chrome/window_chrome.rs` | the window itself: corners, toolbar, flags, cursor, and the `native_rect` → `native_window_rect` → `native_screen_rect` coordinate recipe |
| `chrome/sidebar.rs` | a system-drawn `NSSplitViewItem.sidebar`. A **window shell**, not a rectangle: install once, update per frame |
| `controls/` | hosted controls — `button`, `menu`, `progress`, `slider`, `small`, `switch`, `text_field` |
| `materials/` | vibrancy and macOS 26 liquid glass; `glass_private`, `custom_glass`, `private_api` are `private-api`-only |
| `surfaces.rs` | transient surfaces in their own `NSWindow`s: popovers, sheets, the system share picker |
| `system/tokens.rs` | system colours, fonts, file icons, `WINDOW_BACKGROUND` |
| `system/color.rs` | sRGB conversion — it must be `colorWithSRGBRed:`, not the device-RGB `colorWithRed:` |

## Examples

Two are the ones that matter; the rest are single-component demos and measurement probes.

* **`examples/storybook/`** — the component browser: native macOS sidebar, GPUI content, 16 pages in
  `pages/`. Module-per-page: `PAGES` in `main.rs` is the **only** place a page is registered — the
  sidebar, the header and the router all read it. Each page is a `pub struct State` plus a
  `pub fn render`.
* **`examples/finder_demo/`** — a 1:1 Finder recreation and the proving ground for the chrome. Real
  macOS chrome, entirely fictional files: nothing reads the filesystem, icons come from a *type*
  lookup, and dates and sizes are formatted by `NSDateFormatter` / `NSByteCountFormatter` so the
  columns read in the user's own locale. One module per view — `toolbar`, `sidebar`, `header`,
  `list`, `column`, `icon`, `gallery`, `preview`, `palette`, `data`, `ui`. **Roughly a quarter of the
  anchor rows live in here**; treat its numbers as measured, not chosen.

Single-component demos: `button`, `glass`, `keys`, `material`, `menu`, `menu_bar`, `progress`,
`sidebar`, `slider`, `small`, `smoke`, `surfaces`, `text_field`, `tokens`, `window_chrome`,
`gallery` (everything in one window), plus `custom_glass` and `glass_variants` behind `private-api`.

Probes and gates, which read out of AppKit rather than showing anything: `glass_hit`, `z_order`,
`tabular_figures`, `menu_probe`, `sidebar_probe`.

## `scripts/`

| script | what it does |
|---|---|
| `check.sh` | **the gate.** Both feature configs, both anchor arms, formatting, unit tests, `z_order`, `glass_hit`, `tabular_figures`, rustdoc under `-D warnings`, and zero warnings in `src/`+`examples/`. Must print `== VERDICT: PASS`. Takes no keyboard focus and captures no pixels. ~4 min |
| `anchors.sh` | the comment guard: one row per load-bearing measured fact. `--self-test` proves both arms falsifiable. See AGENTS.md § 3 |
| `bundle.sh <example> [features]` | assembles a double-clickable, ad-hoc-signed `.app` in `dist/`. **Required before any keyboard or key-state measurement** — LaunchServices will not activate a bare binary. Defaults the storybook to `private-api` |
| `keys.sh` | keyboard suite driven by real System Events keystrokes, so events enter through the WindowServer as a physical key does. Arms: `KEYS_MENUS=0`, `KEYS_SABOTAGE=want`, `KEYS_SABOTAGE=nokeys` |
| `menubar.sh` | reads the menu bar back out of a **second** process via Accessibility, so the probe cannot agree with itself. `--sabotage`, `--proc <app>`, `--dump <file>` |
| `winlist.sh [pid]` | window-server census (`CGWindowListCopyWindowInfo`), frontmost first. **Six** tab-separated fields |
| `lockguard.sh` | aborts with **exit 90** if the display is locked or asleep. Run before *and* after anything that produces a number |
| `uidiff.py <a.png> [b.png]` | structure out of window captures: full-width boundaries, dominant row pitch by autocorrelation, column edges in a band |

`scripts/winlist.swift` is compiled on demand by `winlist.sh`; `scripts/menubar.applescript` is
driven by `menubar.sh`.

## Build and run

```sh
cargo build --release --examples                          # default features
cargo build --release --examples --features private-api   # required-features examples too

cargo run --release --example storybook --features private-api
cargo run --release --example finder_demo

./scripts/bundle.sh storybook && open -n dist/Storybook.app   # needed for key-window work
./scripts/check.sh                                            # the gate
```

`private-api` is off by default, and `custom_glass` / `glass_variants` carry `required-features`, so
a default `--examples` build **skips them by construction**. Both configs have to build clean, with
zero warnings.

Requires macOS 26 or later (`LSMinimumSystemVersion` 26.0). `/target`, `/dist` and `/shots` are
gitignored.
