//! The small element helpers every view shares.

use gpui::{
    App, ClickEvent, ElementId, FontWeight, IntoElement, MouseDownEvent, Pixels, ScrollHandle,
    SharedString, Styled, TextRun, WeakEntity, Window, hsla, point,
};
use native_gpui::tokens;
use native_gpui::{NativeScroller, native_scroller};

pub(crate) use native_gpui::{focus_gpui, native_text_field};

use crate::app::FinderDemo;
use crate::data::Line;
use crate::frame::ns_window;

/// How wide `text` shapes at `size` in the system font: what the column and crumb fits measure.
pub(crate) fn text_width(window: &Window, text: impl Into<SharedString>, size: Pixels) -> f32 {
    let text = text.into();
    let run = TextRun {
        len: text.len(),
        font: tokens::system_font(FontWeight::NORMAL),
        color: hsla(0., 0., 0., 1.),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f32::from(
        window
            .text_system()
            .shape_line(text, size, &[run], None)
            .width,
    )
}

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

/// Finder's row click: it picks the row, and the second one opens it.
pub(crate) fn pick_on_click(
    line: &Line,
    this: WeakEntity<FinderDemo>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let line = line.clone();
    move |event, window, cx| {
        let (extend, toggle, twice) = click_kind(event);
        let line = line.clone();
        this.update(cx, |this: &mut FinderDemo, cx| {
            this.pick(&line, extend, toggle);
            if twice {
                this.open(&line, window, cx);
            }
            cx.notify();
        })
        .ok();
    }
}

/// A press picks an unchosen row before its drag starts; Shift and Command wait for the click.
pub(crate) fn pick_on_press(
    line: &Line,
    this: WeakEntity<FinderDemo>,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    let line = line.clone();
    move |event, _, cx| {
        if event.modifiers.shift || event.modifiers.platform {
            return;
        }
        let line = line.clone();
        this.update(cx, |this: &mut FinderDemo, cx| {
            if !this.selected.contains(&line.key) {
                this.pick(&line, false, false);
                cx.notify();
            }
        })
        .ok();
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

/// The trailing-edge scroller, or nothing while it all fits; `inset` is content above the rows.
pub(crate) fn scroller(
    id: impl Into<ElementId>,
    handle: &ScrollHandle,
    inset: f32,
) -> Option<NativeScroller> {
    let max = f32::from(handle.max_offset().y);
    let room = f32::from(handle.bounds().size.height) - inset;
    if max < 1. || room <= 0. {
        return None;
    }
    let handle = handle.clone();
    let value = (-f32::from(handle.offset().y) / max).clamp(0., 1.);
    Some(
        native_scroller(id, value as f64, (room / (room + max)) as f64).on_change(
            move |value, window, _| {
                let offset = handle.offset();
                handle.set_offset(point(offset.x, -handle.max_offset().y * *value as f32));
                window.refresh();
            },
        ),
    )
}

/// A scroll container that ignores the other axis, so nested scrollers own one axis each.
pub(crate) fn axis_locked<E: Styled>(mut element: E) -> E {
    element.style().restrict_scroll_to_axis = Some(true);
    element
}
