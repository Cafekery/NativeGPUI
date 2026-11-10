// Control for the negative arm: neg.rs's probes aimed at paths that DO exist. Must COMPILE.
#![allow(unused_imports)]

use native_gpui::sidebar::sidebar_item;
use native_gpui::tokens::WINDOW_BACKGROUND;
use native_gpui::window_chrome::supports_corner_radius;
use native_gpui::native_scroll_bars;

fn main() {
    let _ = native_gpui::tokens::SystemColor::Label
        .try_resolve(native_gpui::tokens::Appearance::Dark);
}
