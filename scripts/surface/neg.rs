// Negative arm: every path removed from the public surface must stay gone.
// Must FAIL, with one error per marked probe.
#![allow(unused_imports)]

// probe: `chrome` is a private grouping module.
use native_gpui::chrome::sidebar::sidebar_item;

// probe: `system` is a private grouping module.
use native_gpui::system::tokens::WINDOW_BACKGROUND;

// probe: deleted; `supports_corner_radius` replaces it.
use native_gpui::window_chrome::corner_radius_selector;

// probe: deleted; `native_scroll_bars` puts AppKit's scroll view over the content instead.
use native_gpui::native_scroller;

fn main() {
    // probe: deleted; it was `srgb(&self.ns_color())` with both halves public.
    let _ = native_gpui::tokens::SystemColor::Label.resolve_ambient();
}
