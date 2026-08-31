//! The colours one frame of the file list needs, resolved once.
//!
//! Owns [`Palette`] and the one semantic colour this crate's token set does not name.
//!
//! Deliberately not here: geometry, artwork, or anything that builds an element. A view takes a
//! `&Palette` and reads fields off it; nothing in this module knows what a row is.

use std::cell::RefCell;

use block2::StackBlock;
use gpui::Hsla;
use native_gpui::tokens::{self, SystemColor, system_color};
use objc2_app_kit::NSColor;

/// Every colour the list needs, resolved once per frame. `focused` picks between AppKit's two
/// selection colours, which is the rule a real table follows: the vivid one only while the view
/// that owns the selection has the keyboard.
pub(crate) struct Palette {
    pub(crate) label: Hsla,
    pub(crate) secondary: Hsla,
    pub(crate) tertiary: Hsla,
    pub(crate) surface: Hsla,
    pub(crate) stripe: Hsla,
    pub(crate) separator: Hsla,
    pub(crate) selection: Hsla,
    pub(crate) selection_text: Hsla,
    pub(crate) focused: bool,
}

impl Palette {
    pub(crate) fn new(focused: bool) -> Self {
        Self {
            label: system_color(SystemColor::Label),
            secondary: system_color(SystemColor::SecondaryLabel),
            tertiary: system_color(SystemColor::TertiaryLabel),
            surface: system_color(SystemColor::ControlBackground),
            stripe: alternating_row()
                .unwrap_or_else(|| system_color(SystemColor::ControlBackground)),
            separator: system_color(SystemColor::Separator),
            selection: if focused {
                system_color(SystemColor::SelectedContentBackground)
            } else {
                system_color(SystemColor::UnemphasizedSelectedContentBackground)
            },
            selection_text: if focused {
                system_color(SystemColor::AlternateSelectedControlText)
            } else {
                system_color(SystemColor::Label)
            },
            focused,
        }
    }
}

/// `alternatingContentBackgroundColors[1]`, the even row's fill, which this crate's palette does
/// not name.
///
/// Resolved inside an explicit drawing appearance rather than in whatever one happens to be
/// ambient: the dark value is white at 0.047 alpha over the table background while the light one is
/// an opaque (244,245,245), so an ambient read matches one appearance and silently contradicts the
/// other. Falls back to the plain surface, which shows up as banding simply gone rather than as
/// banding in the wrong colour.
pub(crate) fn alternating_row() -> Option<Hsla> {
    let slot: RefCell<Option<Hsla>> = RefCell::new(None);
    let block = StackBlock::new(|| {
        let colors = NSColor::alternatingContentBackgroundColors();
        *slot.borrow_mut() = colors.iter().nth(1).and_then(|color| tokens::srgb(&color));
    });
    tokens::current_appearance()
        .ns_appearance()
        .performAsCurrentDrawingAppearance(&block);
    slot.into_inner()
}
