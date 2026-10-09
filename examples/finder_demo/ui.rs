use gpui::{
    App, ClickEvent, Context, Div, Hsla, IntoElement, MouseDownEvent, Pixels, SharedString, Styled,
    WeakEntity, Window, div, prelude::*, px,
};

pub(crate) use native_gpui::{focus_gpui, native_text_field};

use crate::app::FinderDemo;
use crate::data::Line;
use crate::frame::ns_window;

pub(crate) fn text_width(window: &Window, text: impl Into<SharedString>, size: Pixels) -> f32 {
    crate::fit::line_width(window.text_system(), text.into(), size)
}

const RING_W: f32 = 2.;
const RING_INSET: f32 = 1.;

pub(crate) fn menu_ring(
    colour: Hsla,
    (inset_x, inset_y): (f32, f32),
    radius: f32,
    (above, below): (bool, bool),
    on_selection: bool,
) -> Div {
    let step = if on_selection { RING_INSET } else { 0. };
    let corner = px((radius - step).max(0.));
    div()
        .absolute()
        .left(px(inset_x + step))
        .right(px(inset_x + step))
        .top(px(if above { 0. } else { inset_y + step }))
        .bottom(px(if below { 0. } else { inset_y + step }))
        .border_l(px(RING_W))
        .border_r(px(RING_W))
        .border_color(colour)
        .when(!above, |ring| ring.border_t(px(RING_W)).rounded_t(corner))
        .when(!below, |ring| ring.border_b(px(RING_W)).rounded_b(corner))
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
        this.update(cx, |this, cx| {
            this.pick(&line, extend, toggle);
            match line
                .row
                .opens
                .filter(|_| twice && toggle && line.row.readable)
            {
                Some(folder) if crate::prefs::opens_folders_in_tabs() => {
                    this.open_tab(folder, window, cx)
                }
                Some(folder) => {
                    crate::open_finder_window(folder, cx);
                }
                None if twice => this.open(&line, window, cx),
                None => {}
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
        this.update(cx, |this, cx| {
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
            end_rename(&this, window, cx, |this, cx| this.rename(&key, text, cx))
        })
        .on_cancel(move |_, window, cx| end_rename(&leave, window, cx, |_, _| {}))
}

fn end_rename(
    this: &WeakEntity<FinderDemo>,
    window: &mut Window,
    cx: &mut App,
    then: impl FnOnce(&mut FinderDemo, &mut Context<FinderDemo>),
) {
    this.update(cx, |this, cx| {
        then(this, cx);
        this.renaming = None;
        focus_gpui(window);
        key_to_parent(window);
        cx.notify();
    })
    .ok();
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
