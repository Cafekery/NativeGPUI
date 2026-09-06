//! The small element helpers every view shares.

use gpui::{ClickEvent, IntoElement, Styled, WeakEntity, Window};

pub(crate) use native_gpui::{focus_gpui, native_text_field};

use crate::app::FinderDemo;
use crate::data::Line;
use crate::frame::ns_window;

/// `(extend, toggle, twice)`: Shift, Cmd and a double click.
pub(crate) fn click_kind(event: &ClickEvent) -> (bool, bool, bool) {
    match event {
        ClickEvent::Mouse(mouse) => (
            mouse.up.modifiers.shift,
            mouse.up.modifiers.platform,
            mouse.up.click_count >= 2,
        ),
        _ => (false, false, false),
    }
}

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
                this.rename(&key, &text, cx);
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

/// Hand key back to the main window after a field in the pane's panel took it.
pub(crate) fn key_to_parent(window: &Window) {
    if let Some(parent) = ns_window(window).and_then(|panel| panel.parentWindow()) {
        parent.makeKeyWindow();
    }
}

/// A scroll container that ignores the other axis; nested with a list, each axis then has one owner.
pub(crate) fn axis_locked<E: Styled>(mut element: E) -> E {
    element.style().restrict_scroll_to_axis = Some(true);
    element
}
