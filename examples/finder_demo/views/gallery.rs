use std::ops::Range;

use gpui::{AnyElement, Pixels, Point, Window, div, point, prelude::*, px};
use native_gpui::{native_callout, native_scroll_bars};

use crate::app::FinderDemo;
use crate::art::{Poster, art_at, poster_art, row_art};
use crate::menus::tiles_menu;
use crate::palette::Palette;
use crate::ui::pick_on_click;

const STRIP_INSET: f32 = 5.;
const STRIP_RADIUS: f32 = 8.;
const HERO_INSET: f32 = 61.;

#[derive(Clone, Copy)]
struct Strip {
    icon: f32,
    cell: f32,
    gap: f32,
    margin: f32,
}

impl Strip {
    fn at(icon: f32) -> Self {
        Self {
            icon,
            cell: icon + 6.,
            gap: icon / 8. - 1.,
            margin: icon / 16. + 3.,
        }
    }

    fn height(self) -> f32 {
        self.cell + 2. * self.margin
    }

    fn start(self, index: usize) -> f32 {
        STRIP_INSET + index as f32 * (self.cell + self.gap)
    }

    fn seen(self, offset: f32, room: f32, count: usize) -> Range<usize> {
        let first = ((-offset - STRIP_INSET) / (self.cell + self.gap)).floor() - 1.;
        let first = (first.max(0.) as usize).min(count);
        let last = ((-offset + room - STRIP_INSET) / (self.cell + self.gap)).ceil() + 1.;
        first..(last.max(0.) as usize).clamp(first, count)
    }
}

impl FinderDemo {
    pub(crate) fn gallery_body(
        &mut self,
        palette: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lines = self.lines(false);
        let chosen = self.chosen_in(&lines, 3);
        let strip = Strip::at(self.strip_icon);
        let room_w = self.list_room;
        let room_h = f32::from(window.viewport_size().height)
            - f32::from(self.metrics().top)
            - strip.height();
        let size = (room_w.min(room_h) - 2. * HERO_INSET).max(64.);
        let scale = self.scale;
        let hero = match chosen.as_slice() {
            many if many.len() > 1 => Some(self.stack_hero(many, size)),
            _ => chosen
                .first()
                .cloned()
                .or_else(|| self.cursor_line())
                .map(|line| {
                    art_at(
                        poster_art(&line.row, size, size, scale, Poster::Hero),
                        size,
                        Some(self.ql_icon.clone()),
                    )
                }),
        };

        let this = cx.entity().downgrade();
        let hit_strip = self.strip.clone();
        let hit = move |demo: &FinderDemo, at: Point<Pixels>| {
            let x = f32::from(at.x) - f32::from(hit_strip.offset().x) - STRIP_INSET;
            let index = x / (strip.cell + strip.gap);
            if x < 0. || index.fract() * (strip.cell + strip.gap) > strip.cell {
                return None;
            }
            demo.lines(false).get(index as usize).cloned()
        };
        let seen = strip.seen(f32::from(self.strip.offset().x), room_w, lines.len());
        let strip_row = div()
            .id("ng.finder.filmstrip")
            .h(px(strip.height()))
            .flex_none()
            .relative()
            .overflow_x_scroll()
            .track_scroll(&self.strip)
            .child(div().w(px(strip.start(lines.len()))).h_full().flex_none())
            .children(seen.map(|index| {
                let line = lines[index].clone();
                let is_selected = self.selected.contains(&line.key);
                let cell = div()
                    .id(("ng.finder.frame", index))
                    .absolute()
                    .left(px(strip.start(index)))
                    .top(px(strip.margin))
                    .w(px(strip.cell))
                    .h(px(strip.cell))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(STRIP_RADIUS))
                    .when(is_selected, |cell| cell.bg(palette.tertiary))
                    .child(row_art(&line.row, strip.icon, scale, true))
                    .on_click(pick_on_click(&line, this.clone()));
                native_callout(
                    ("ng.finder.strip.tip", index),
                    line.row.display.clone(),
                    cell,
                )
            }));

        div()
            .flex_1()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(hero),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .child(tiles_menu("ng.finder.strip.menu", this, hit, strip_row))
                    .child(div().absolute().inset_0().child(
                        native_scroll_bars("ng.finder.strip.scroll_bars").horizontal(&self.strip),
                    )),
            )
            .into_any_element()
    }

    pub(crate) fn strip_reveal(&self, index: usize) {
        let strip = Strip::at(self.strip_icon);
        let start = strip.start(index);
        let seen = -f32::from(self.strip.offset().x);
        let visible = f32::from(self.strip.bounds().size.width);
        let next = if start < seen {
            start - STRIP_INSET
        } else if start + strip.cell > seen + visible {
            start + strip.cell + STRIP_INSET - visible
        } else {
            return;
        };
        self.strip.set_offset(point(px(-next.max(0.)), px(0.)));
    }
}
