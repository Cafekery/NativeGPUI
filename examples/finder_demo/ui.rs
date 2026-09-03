//! Drawing helpers every view shares, and the tag colours they draw with.

use gpui::{AnyElement, FontWeight, Hsla, WeakEntity, Window, div, prelude::*, px};

use native_gpui::tokens::{SymbolColor, SystemColor, file_icon, sf_symbol_colored, system_color};
use native_gpui::{focus_gpui, native_text_field};

use crate::data::{Face, Line, Row};
use crate::{FinderDemo, ns_window};

pub(crate) const TAGS: &[(&str, SystemColor)] = &[
    ("Red", SystemColor::SystemRed),
    ("Orange", SystemColor::SystemOrange),
    ("Yellow", SystemColor::SystemYellow),
    ("Green", SystemColor::SystemGreen),
    ("Blue", SystemColor::SystemBlue),
    ("Purple", SystemColor::SystemPurple),
    ("Gray", SystemColor::SystemGray),
];

pub(crate) fn artwork(row: &Row, size: f32) -> AnyElement {
    let art = row
        .artwork
        .and_then(|kind| file_icon(kind, px(size)))
        .map(|symbol| symbol.img().into_any_element())
        .unwrap_or_else(|| glyph(row.face, size));
    div()
        .w(px(size))
        .h(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(art)
        .into_any_element()
}

pub(crate) fn glyph(face: Face, size: f32) -> AnyElement {
    let symbol = sf_symbol_colored(face.symbol(), px(size), FontWeight::NORMAL, &face.tint());
    let mut slot = div()
        .w(px(size + 4.))
        .h(px(size))
        .flex()
        .items_center()
        .justify_center()
        .flex_none();
    if let Some(symbol) = symbol {
        slot = slot.child(symbol.img());
    }
    slot.into_any_element()
}

pub(crate) fn small_glyph(name: &str, size: f32, color: SystemColor) -> Option<AnyElement> {
    sf_symbol_colored(
        name,
        px(size),
        FontWeight::SEMIBOLD,
        &SymbolColor::Hierarchical(color),
    )
    .map(|symbol| symbol.img().into_any_element())
}

/// Finder's tag pill over a selection: the tag's colour washed under its name.
pub(crate) fn tag_pill(index: usize, text: Hsla) -> AnyElement {
    let (name, color) = TAGS[index];
    let mut wash = system_color(color);
    wash.a = 0.55;
    div()
        .px(px(6.))
        .rounded(px(4.))
        .bg(wash)
        .text_color(text)
        .child(name)
        .into_any_element()
}

pub(crate) fn tag_dot(index: usize) -> AnyElement {
    div()
        .w(px(8.))
        .h(px(8.))
        .rounded_full()
        .flex_none()
        .bg(system_color(TAGS[index].1))
        .into_any_element()
}

/// The name edited in place: Finder selects the stem, Return commits, Escape leaves it, and
/// either way the view takes the keyboard back.
pub(crate) fn rename_field(
    line: &Line,
    at: usize,
    this: WeakEntity<FinderDemo>,
) -> impl IntoElement {
    let name = line.row.name.clone();
    let dot = name.rfind('.').filter(|&dot| dot > 0).unwrap_or(name.len());
    let stem = name[..dot].chars().count();
    let key = line.key.clone();
    let leave = this.clone();
    native_text_field(("ng.finder.rename", at), &name)
        .stretch()
        .editing(0..stem)
        .on_submit(move |text, window, cx| {
            let text = text.clone();
            this.update(cx, |this: &mut FinderDemo, cx| {
                this.rename(&key, &text);
                this.renaming = None;
                focus_gpui(window);
                key_to_parent(window);
                cx.notify();
            })
            .ok();
        })
        .on_cancel(move |_, window, cx| {
            leave
                .update(cx, |this: &mut FinderDemo, cx| {
                    this.renaming = None;
                    focus_gpui(window);
                    key_to_parent(window);
                    cx.notify();
                })
                .ok();
        })
}

/// A field in the pane's child panel takes key from the window it hangs off; hand it back once
/// the edit ends. A field in the main window itself has no parent to hand it to.
fn key_to_parent(window: &Window) {
    if let Some(parent) = ns_window(window).and_then(|panel| panel.parentWindow()) {
        parent.makeKeyWindow();
    }
}

/// A scroll container that ignores the other axis; nested with a list, each axis then has one owner.
pub(crate) fn axis_locked<E: Styled>(mut element: E) -> E {
    element.style().restrict_scroll_to_axis = Some(true);
    element
}
