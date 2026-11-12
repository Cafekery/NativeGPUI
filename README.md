# NativeGPUI

Real AppKit controls, window chrome, sidebars, menus and macOS 26 liquid glass inside a GPUI window. GPUI owns layout; macOS owns the pixels.

macOS 26 or later, Apple silicon, Rust 1.97 or later. MIT licensed.

## The framework

Controls (`controls`), window chrome and the source-list sidebar (`window_chrome`, `sidebar`), menus (`install_menus`, `add_menu`), materials and liquid glass (`materials`), Quick Look (`quicklook`) and system tokens (`tokens`).

```rust
use gpui::{App, Context, Render, Window, div, prelude::*};
use native_gpui::{install_menus, native_button, native_root, window_options};

struct Hello;

impl Render for Hello {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        native_root(div().size_full().p_5().child(
            native_button("go", "Say hello").on_click(|_, _, _| println!("hello")),
        ))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        install_menus(cx);
        cx.open_window(window_options(), |_, cx| cx.new(|_| Hello)).unwrap();
        cx.activate(true);
    });
}
```

Run an example with `cargo run --release --example sidebar`. `custom_glass` and `glass_variants` also need `--features private-api` (undocumented material values and private AppKit selectors, off by default).

To depend on it, use the same GPUI revision the framework builds against, and take `gpui_macos` from this repository:

```toml
[dependencies]
native-gpui = { git = "https://github.com/Cafekery/NativeGPUI" }
gpui = { git = "https://github.com/zed-industries/zed", rev = "90d8514059561f0e44af0a32e84e1c329cef3232" }
gpui_platform = { git = "https://github.com/zed-industries/zed", rev = "90d8514059561f0e44af0a32e84e1c329cef3232", features = ["font-kit"] }

[patch."https://github.com/zed-industries/zed"]
gpui_macos = { git = "https://github.com/Cafekery/NativeGPUI" }
```

`crates/gpui-macos` is zed's `gpui_macos` at that revision plus seven patches, under Apache-2.0.

<div align="center">

<br>
<br>

<img src="founder/assets/icon.png" alt="" width="128" height="128" />

<h2>Found'er</h2>

The Finder, rebuilt on NativeGPUI.

**[Download Founder.zip](https://github.com/Cafekery/NativeGPUI/releases/latest/download/Founder.zip)**

</div>

The app is not notarized. Move it to Applications first (from Downloads the default file browser item stays disabled), open it, click Done, then click Open Anyway in System Settings → Privacy & Security.

### Build from source

```sh
git clone https://github.com/Cafekery/NativeGPUI.git
cd NativeGPUI
cargo install cargo-about --locked --features cli
./founder/scripts/bundle.sh
```

This makes `dist/Found'er.app`. `cargo run --release -p founder` also works, but a bare cargo binary never becomes the key window, so use the bundle for anything that depends on focus.

### First launch

macOS asks before Found'er touches Desktop, Documents, Downloads, external disks or iCloud Drive; it works without the ones you refuse. It indexes file names on your startup disk in the background, about 700–800 MB in `~/Library/Application Support/Founder/fsearch`.

### Default file browser

Found'er menu → Use Found'er as Default File Browser makes Show in Finder open Found'er. It adds a login item, `viewer-guard` (shown as "sh"), that restores your old viewer if Found'er is deleted. Uncheck the item to undo it, or run this by hand, only if `defaults read -g NSFileViewer` prints `dev.cafekery.founder`:

```sh
defaults delete -g NSFileViewer
launchctl bootout gui/$(id -u)/dev.cafekery.founder.viewer-guard
rm ~/Library/LaunchAgents/dev.cafekery.founder.viewer-guard.plist
```

### Uninstall

Uncheck the default file browser item, quit Found'er, move it to the Trash, then:

```sh
rm -rf ~/Library/Application\ Support/Founder
rm -rf ~/Library/Caches/dev.cafekery.founder
defaults delete dev.cafekery.founder
tccutil reset All dev.cafekery.founder
```

Saved searches in `~/Library/Saved Searches` stay, because Finder reads them too.

Saved server passwords stay in Keychain Access, under the server's address.

`founder-cli` is a developer tool, not in the app bundle. `cargo run --release -p founder-cli -- help` runs Found'er's file engine without a window.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) to build and send changes, and [SECURITY.md](SECURITY.md) to report a security problem.

## License

MIT; see [LICENSE](LICENSE). fsearch ([founder/crates/fsearch/LICENSE](founder/crates/fsearch/LICENSE)) is by Noah Dunnagan, also MIT. `crates/gpui-macos` is Apache-2.0. Not affiliated with or endorsed by Apple.
