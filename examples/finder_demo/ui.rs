use gpui::{
    App, ClickEvent, Div, FontWeight, Hsla, IntoElement, MouseDownEvent, Pixels, SharedString,
    Styled, TextRun, WeakEntity, Window, div, hsla, prelude::*, px,
};
use native_gpui::tokens;

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

const RING_W: f32 = 2.;
const RING_INSET: f32 = 1.;

pub(crate) fn menu_ring(
    colour: Hsla,
    pane_inset: f32,
    radius: f32,
    run: (bool, bool),
    on_selection: bool,
) -> Div {
    let step = if on_selection { RING_INSET } else { 0. };
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

pub(crate) fn axis_locked<E: Styled>(mut element: E) -> E {
    element.style().restrict_scroll_to_axis = Some(true);
    element
}
