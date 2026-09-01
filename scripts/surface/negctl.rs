// CONTROL for the negative arm — the same four probes aimed at paths that DO exist.
// Must COMPILE.
//
// A negative arm that cannot be made green is an error message someone liked the look of. Two
// versions of neg.rs were red for reasons that had nothing to do with the removals: one named a
// `SystemColor` variant that does not exist, and one wrote `let _ = generic_fn;`, which needs a
// type annotation whether or not the path resolves. Both would have been counted as proof. Path
// probes are therefore `use` items, which resolve without inference, and this file proves the four
// of them can come out green.
#![allow(unused_imports)]

use native_gpui::sidebar::sidebar_item;
use native_gpui::tokens::WINDOW_BACKGROUND;
use native_gpui::window_chrome::supports_corner_radius;

fn main() {
    let _ = native_gpui::tokens::SystemColor::Label
        .try_resolve(native_gpui::tokens::Appearance::Dark);
}
