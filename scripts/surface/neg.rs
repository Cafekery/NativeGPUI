// NEGATIVE ARM — every path deliberately removed from the public surface must stay gone.
// Must FAIL, with one error per probe.
//
// This is the half nothing else in the tree can see. `chrome` and `system` are private grouping
// modules so that each feature has one obvious path instead of three; a future `pub mod` would
// hand back the extra paths and every other check would stay green, because no example imports
// `native_gpui::chrome::` to notice. Same for a deleted item reappearing.
//
// Each probe is marked `// probe:` and surface.sh requires the error count to equal the probe
// count, so one unrelated error cannot stand in for four.
#![allow(unused_imports)]

// probe: `chrome` is a private grouping module — reach the sidebar at the root or via `sidebar`.
use native_gpui::chrome::sidebar::sidebar_item;

// probe: `system` is a private grouping module — reach the tokens via `tokens`.
use native_gpui::system::tokens::WINDOW_BACKGROUND;

// probe: deleted. It had no caller and was the only place `objc2::runtime::Sel` reached the
// public surface. `supports_corner_radius` answers the same question for anyone with a `Window`.
use native_gpui::window_chrome::corner_radius_selector;

// probe: deleted. It was a one-line pass-through to a private `to_srgb` in the same file with no
// caller anywhere, and `from_nscolor` is now the crate's one NSColor-to-Hsla conversion.
use native_gpui::tokens::srgb;

fn main() {
    // probe: deleted. It was exactly `srgb(&self.ns_color())` with both halves public, and the
    // control it claimed to be lives in the storybook's tokens page instead.
    let _ = native_gpui::tokens::SystemColor::Label.resolve_ambient();
}
