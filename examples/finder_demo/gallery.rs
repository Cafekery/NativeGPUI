//! The gallery view: the selection large, every item in a strip along the bottom.

use gpui::{AnyElement, ClickEvent, Window, div, point, prelude::*, px};
use native_gpui::native_tooltip;

use crate::app::FinderDemo;
use crate::art::{Poster, art_at, poster_art, row_art};
use crate::data::Line;
use crate::menus::row_menu;
use crate::palette::Palette;
use crate::ui::click_kind;

const STRIP_INSET: f32 = 5.;
const STRIP_RADIUS: f32 = 8.;
const HERO_INSET: f32 = 61.;
const HERO_MAX: f32 = 512.;

/// The strip's cell, gap and margin at one of Finder's three thumbnail sizes (32, 48, 64).
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
}

impl FinderDemo {
    pub(crate) fn gallery_body(
        &mut self,
        palette: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lines = self.lines(false);
        // Only the hero needs the chosen rows, and it draws at most three of them.
        let chosen: Vec<Line> = lines
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .take(3)
            .cloned()
            .collect();
        let strip = Strip::at(self.strip_icon);
        let room_w = self.list_room - self.pane_w();
        let room_h = f32::from(window.viewport_size().height)
            - f32::from(self.metrics().top)
            - strip.height();
        let size = (room_w.min(room_h) - 2. * HERO_INSET).clamp(64., HERO_MAX);
        let scale = self.scale;
        let hero = match chosen.as_slice() {
            [] => self.cursor_line().map(|line| {
                art_at(
                    poster_art(&line.row, size, scale, Poster::Hero),
                    size,
                    Some(self.ql_icon.clone()),
                )
            }),
            [one] => Some(art_at(
                poster_art(&one.row, size, scale, Poster::Hero),
                size,
                Some(self.ql_icon.clone()),
            )),
            many => Some(self.stack_hero(many, size)),
        };

        let this = cx.entity().downgrade();
        let strip_row = div()
            .id("ng.finder.filmstrip")
            .h(px(strip.height()))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(strip.gap))
            .px(px(STRIP_INSET))
            .overflow_x_scroll()
            .track_scroll(&self.strip)
            .children(lines.iter().cloned().enumerate().map(|(index, line)| {
                let start = strip.start(index) + f32::from(self.strip.offset().x);
                let on_screen = start + 2. * strip.cell > 0. && start < room_w + strip.cell;
                let is_selected = self.selected.contains(&line.key);
                let this = this.clone();
                let this_menu = this.clone();
                let clicked = line.clone();
                let cell = div()
                    .id(("ng.finder.frame", index))
                    .w(px(strip.cell))
                    .h(px(strip.cell))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(STRIP_RADIUS))
                    .when(is_selected, |cell| cell.bg(palette.tertiary))
                    .child(row_art(&line.row, strip.icon, scale, on_screen))
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
                    });
                row_menu(
                    ("ng.finder.strip.menu", index),
                    &line,
                    &self.selected,
                    this_menu,
                    native_tooltip(("ng.finder.strip.tip", index), line.row.name.clone(), cell),
                )
            }));

        div()
            .flex_1()
            .relative()
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
            .child(strip_row)
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
