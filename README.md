# native-gpui

Real AppKit controls, materials and macOS 26 liquid glass hosted inside a **GPUI** window.

GPUI owns layout; macOS owns the pixels of anything whose design Apple changes between releases. A hosted `NSSwitch` becomes the macOS 27 toggle without this crate being touched.

No Swift, no build script, no C shim: GPUI's window vends its `NSView` through `raw-window-handle`, and `objc2-app-kit` binds all of AppKit including `NSGlassEffectView`.

> Working in here? Read [AGENTS.md](AGENTS.md) first, then [docs/how-it-works.md](docs/how-it-works.md).

## Docs

| doc | for |
|---|---|
| [docs/how-it-works.md](docs/how-it-works.md) | the view tree, one frame start to finish, identity, events, focus, layers, and the gpui pin |
| [docs/adding-a-component.md](docs/adding-a-component.md) | hosting a new AppKit control, walked through with the switch |
| [docs/testing-and-tools.md](docs/testing-and-tools.md) | the gate, the scripts, capture rules, environment knobs, frame-rate measurement |
| [AGENTS.md](AGENTS.md) | the invariants and house rules |

## How it fits together

`native_root` wraps your root element once per window, creates that window's `NativeHost`, and commits every native view's geometry in one pass after your tree paints. Components (`native_switch`, `glass`, `material`, …) are ordinary GPUI elements that reserve layout space and place a pooled AppKit view into it, keyed by GPUI element identity.

`Layer::Above` is a subview of GPUI's Metal view and takes input; `Layer::Below` is a sibling beneath it, so GPUI composites over it and nothing there can receive input.

GPUI is a git dependency on its upstream repository at one pinned revision, and `[patch]` in `Cargo.toml` replaces the macOS platform crate `gpui_macos` with a fork built from that same revision. An app builds its `Application` through `gpui_platform::application()`, with `gpui_platform`'s `font-kit` feature on.

```rust
use gpui::{App, Context, Render, Window, div, prelude::*};
use native_gpui::{install_menus, native_root, native_switch, window_options};

struct Root {
    on: bool,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        native_root(div().size_full().child(
            native_switch("wifi", self.on).on_change(cx.listener(|this, on: &bool, _, cx| {
                this.on = *on;
                cx.notify();
            })),
        ))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        install_menus(cx);
        cx.open_window(window_options(), |_, cx| cx.new(|_| Root { on: false }))
            .unwrap();
        cx.activate(true);
    });
}
```

The value you pass a component must be the state its `on_change` writes.

## `src/` map

| path | owns |
|---|---|
| `lib.rs` | the crate's rules in doc form and the public surface |
| `bridge/host.rs` | the per-window view pool (`NativeHost`), `place`/`commit`, event delivery, the `hosted_element!` boilerplate |
| `bridge/root.rs` | `native_root` and `window_options` |
| `bridge/appkit.rs` | reaching GPUI's view, the flipped container class, the key path adoption |
| `bridge/menus.rs` | the standard menu bar: `install_menus`, `add_menu`, `extend_menu` |
| `chrome/window_chrome.rs` | corners, toolbar, flags, cursor, and the `native_rect` → `native_window_rect` → `native_screen_rect` recipe |
| `chrome/sidebar.rs` | a system-drawn `NSSplitViewItem.sidebar`: install once, update per frame |
| `controls/` | hosted controls: `button`, `menu`, `progress`, `slider`, `small`, `switch`, `text_field`, `label`, `rule` |
| `materials/` | vibrancy and liquid glass; `glass_private`, `custom_glass`, `private_api` are `private-api`-only |
| `surfaces.rs` | popovers, sheets and the share picker, each in its own `NSWindow` |
| `system/tokens.rs` | system colours, fonts, file icons, `WINDOW_BACKGROUND` |
| `system/color.rs` | sRGB conversion |

## Examples

* `examples/storybook/` — the component browser: native sidebar, GPUI content, one page per module in `pages/`. `PAGES` in `main.rs` is the only place a page is registered.
* `examples/finder_demo/` — a Finder recreation and the proving ground for the chrome: real sidebar and toolbar, a list that scrolls sideways under the sidebar, and a preview pane that frosts the rows beneath it (a glass in the window, with a child window for the card, see `preview/`).

Single-component demos: `button`, `glass`, `keys`, `material`, `menu`, `menu_bar`, `progress`, `sidebar`, `slider`, `small`, `smoke`, `surfaces`, `text_field`, `tokens`, `window_chrome`, `gallery`, plus `custom_glass` and `glass_variants` behind `private-api`. Probes and gates: `glass_hit`, `decoration_hit`, `z_order`, `tabular_figures`, `ngdispatch`, `menu_probe`, `sidebar_probe`.

## `scripts/`

| script | what it does |
|---|---|
| `check.sh` | the gate: both feature configs, formatting, unit tests, the probes, rustdoc under `-D warnings`, the public surface, zero warnings. Must print `== VERDICT: PASS` |
| `bundle.sh <example> [features]` | assembles a signed `.app` in `dist/`; required before key-window work |
| `keys.sh` | keyboard suite driven by real System Events keystrokes |
| `perf.sh` | on-screen frame cadence of the Finder demo under a scroll train and a resize drag, read from the window server |
| `menubar.sh` | reads the menu bar back out of a second process via Accessibility |
| `surface.sh` | the public surface, four type-check arms |
| `winlist.sh [pid]` | window-server census, six tab-separated fields |
| `lockguard.sh` | exits 90 if the display is locked or asleep |
| `uidiff.py <a.png> [b.png]` | structure out of window captures |

## Build and run

```sh
cargo build --release --examples
cargo build --release --examples --features private-api

cargo run --release --example storybook --features private-api
cargo run --release --example finder_demo

./scripts/bundle.sh storybook && open -n dist/Storybook.app
./scripts/check.sh
```

Requires macOS 26 or later and rustc 1.98 or later.
