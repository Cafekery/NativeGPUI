use std::cell::{Cell, RefCell};

use block2::StackBlock;
use gpui::{Hsla, hsla};
use native_gpui::tokens::{self, Appearance, SystemColor, system_color};
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

pub(crate) fn browser_rule() -> Hsla {
    match tokens::current_appearance() {
        Appearance::Dark => hsla(0., 0., 0., 1.),
        Appearance::Light => hsla(0., 0., 230. / 255., 1.),
    }
}

pub(crate) fn alternating_row() -> Option<Hsla> {
    thread_local! {
        static KEPT: Cell<Option<(u64, Appearance, Option<Hsla>)>> = const { Cell::new(None) };
    }
    let (generation, appearance) = (tokens::generation(), tokens::current_appearance());
    if let Some((_, _, color)) = KEPT
        .get()
        .filter(|(at, seen, _)| (*at, *seen) == (generation, appearance))
    {
        return color;
    }
    let slot: RefCell<Option<Hsla>> = RefCell::new(None);
    let block = StackBlock::new(|| {
        let colors = NSColor::alternatingContentBackgroundColors();
        *slot.borrow_mut() = colors.iter().nth(1).and_then(|color| tokens::srgb(&color));
    });
    // An ambient read is right in only one appearance, so read inside an explicit one.
    appearance
        .ns_appearance()
        .performAsCurrentDrawingAppearance(&block);
    let color = slot.into_inner();
    KEPT.set(Some((generation, appearance, color)));
    color
}
