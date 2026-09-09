use gpui::{
    App, ClickEvent, Div, ElementId, FontWeight, Hsla, IntoElement, MouseDownEvent, Pixels,
    ScrollHandle, SharedString, Styled, TextRun, WeakEntity, Window, div, hsla, point, prelude::*,
    px,
};
use native_gpui::tokens;
use native_gpui::{NativeScroller, native_scroller};

pub(crate) use native_gpui::{focus_gpui, native_text_field};

use crate::app::FinderDemo;
use crate::data::Line;
use crate::frame::ns_window;

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

/// Finder strokes two points of the row a contextual menu is open on.
const RING_W: f32 = 2.;
/// Over the selection's own fill the stroke turns white and steps a point in.
const RING_INSET: f32 = 1.;

/// `run` is `(joins above, joins below)`, so a block of selected rows is one ring; `inside` is
/// whether it lands on the emphasized fill.
pub(crate) fn menu_ring(
    colour: Hsla,
    pane_inset: f32,
    radius: f32,
    run: (bool, bool),
    inside: bool,
) -> Div {
    let step = if inside { RING_INSET } else { 0. };
    let (above, below) = run;
    div()
        .absolute()
        .left(px(pane_inset + step))
        .right(px(pane_inset + step))
        .top(px(if above { 0. } else { step }))
        .bottom(px(if below { 0. } else { step }))
        .border_l(px(RING_W))
        .border_r(px(RING_W))
        .border_color(colour)
        .when(!above, |ring| {
            ring.border_t(px(RING_W)).rounded_t(px(radius - step))
        })
        .when(!below, |ring| {
            ring.border_b(px(RING_W)).rounded_b(px(radius - step))
        })
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

pub(crate) fn key_to_parent(window: &Window) {
    if let Some(parent) = ns_window(window).and_then(|panel| panel.parentWindow()) {
        parent.makeKeyWindow();
    }
}

/// `inset` is content above the rows.
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

pub(crate) fn axis_locked<E: Styled>(mut element: E) -> E {
    element.style().restrict_scroll_to_axis = Some(true);
    element
}
