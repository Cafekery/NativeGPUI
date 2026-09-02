use std::cell::RefCell;

use block2::StackBlock;
use gpui::Hsla;
use native_gpui::tokens::{self, SystemColor, system_color};
use objc2_app_kit::NSColor;

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

/// `alternatingContentBackgroundColors[1]`, the even row's fill, which the token set does not name.
pub(crate) fn alternating_row() -> Option<Hsla> {
    let slot: RefCell<Option<Hsla>> = RefCell::new(None);
    let block = StackBlock::new(|| {
        let colors = NSColor::alternatingContentBackgroundColors();
        *slot.borrow_mut() = colors.iter().nth(1).and_then(|color| tokens::srgb(&color));
    });
    // Read inside an explicit drawing appearance; an ambient read is right in only one of them.
    tokens::current_appearance()
        .ns_appearance()
        .performAsCurrentDrawingAppearance(&block);
    slot.into_inner()
}
