//! The gallery view: the selection large, every item in a strip along the bottom.

use gpui::{AnyElement, ClickEvent, Window, div, point, prelude::*, px};

use crate::FinderDemo;
use crate::data::Line;
use crate::palette::Palette;
use crate::ui::{artwork, click_kind};

const STRIP_CELL: f32 = 54.;
const STRIP_GAP: f32 = 5.;
const STRIP_INSET: f32 = 5.;
const STRIP_BOTTOM: f32 = 6.;
const STRIP_ICON: f32 = 48.;
const STRIP_RADIUS: f32 = 8.;
const STRIP_H: f32 = STRIP_CELL + 2. * STRIP_BOTTOM;
const HERO_INSET: f32 = 61.;
const HERO_MAX: f32 = 512.;

impl FinderDemo {
    pub(crate) fn gallery_body(
        &mut self,
        palette: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lines = self.lines(false);
        let chosen: Vec<Line> = lines
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .cloned()
            .collect();
        let room_w = self.list_room - self.pane_w();
        let room_h =
            f32::from(window.viewport_size().height) - f32::from(self.metrics().top) - STRIP_H;
        let size = (room_w.min(room_h) - 2. * HERO_INSET).clamp(64., HERO_MAX);
        let hero = match chosen.as_slice() {
            [] => self.cursor_line().map(|line| artwork(&line.row, size)),
            [one] => Some(artwork(&one.row, size)),
            many => Some(self.stack_hero(many, size)),
        };

        let this = cx.entity().downgrade();
        let strip = div()
            .id("ng.finder.filmstrip")
            .h(px(STRIP_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(STRIP_GAP))
            .px(px(STRIP_INSET))
            .overflow_x_scroll()
            .track_scroll(&self.strip)
            .children(lines.into_iter().enumerate().map(|(index, line)| {
                let is_selected = self.selected.contains(&line.key);
                let this = this.clone();
                let clicked = line.clone();
                div()
                    .id(("ng.finder.frame", index))
                    .w(px(STRIP_CELL))
                    .h(px(STRIP_CELL))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(STRIP_RADIUS))
                    .when(is_selected, |cell| cell.bg(palette.tertiary))
                    .child(artwork(&line.row, STRIP_ICON))
                    .on_click(move |event: &ClickEvent, window, cx| {
                        let (extend, toggle, twice) = click_kind(event);
                        let line = clicked.clone();
                        this.update(cx, |this: &mut FinderDemo, cx| {
                            this.pick(&line, extend, toggle);
                            if twice {
                                this.open(&line, window, cx);
                            }
                            cx.notify();
                        })
                        .ok();
                    })
            }));

        div()
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .pr(px(self.pane_w()))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(hero),
            )
            .child(strip)
            .into_any_element()
    }

    pub(crate) fn strip_reveal(&self, index: usize) {
        let start = STRIP_INSET + index as f32 * (STRIP_CELL + STRIP_GAP);
        let seen = -f32::from(self.strip.offset().x);
        let visible = f32::from(self.strip.bounds().size.width);
        let next = if start < seen {
            start - STRIP_INSET
        } else if start + STRIP_CELL > seen + visible {
            start + STRIP_CELL + STRIP_INSET - visible
        } else {
            return;
        };
        self.strip.set_offset(point(px(-next.max(0.)), px(0.)));
    }
}
