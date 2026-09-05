//! The gallery view: the selection large, every item in a strip along the bottom.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, ClickEvent, Path, SharedString, Window, canvas, div, point, prelude::*, px,
};
use native_gpui::tokens::{self, Appearance};

use crate::FinderDemo;
use crate::data::Line;
use crate::menus::row_menu;
use crate::palette::Palette;
use crate::ui::{click_kind, thumb_art};

const STRIP_INSET: f32 = 5.;
const STRIP_RADIUS: f32 = 8.;
const HERO_INSET: f32 = 61.;
const HERO_MAX: f32 = 512.;

const TIP_DELAY: Duration = Duration::from_millis(800);
const TIP_LINGER: Duration = Duration::from_millis(300);
const TIP_H: f32 = 25.;
const TIP_PAD: f32 = 12.;
const TIP_RADIUS: f32 = 10.;
const TIP_FONT: f32 = 13.;
const TIP_ARROW: f32 = 10.;
const TIP_GAP: f32 = 3.5;

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
        let chosen: Vec<Line> = lines
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .cloned()
            .collect();
        let strip = Strip::at(self.strip_icon);
        let room_w = self.list_room - self.pane_w();
        let room_h = f32::from(window.viewport_size().height)
            - f32::from(self.metrics().top)
            - strip.height();
        let size = (room_w.min(room_h) - 2. * HERO_INSET).clamp(64., HERO_MAX);
        let hero = match chosen.as_slice() {
            [] => self.cursor_line().map(|line| thumb_art(&line.row, size)),
            [one] => Some(thumb_art(&one.row, size)),
            many => Some(self.stack_hero(many, size)),
        };

        let this = cx.entity().downgrade();
        let tip = self.strip_tip.and_then(|index| {
            let line = lines.get(index)?;
            let centre = strip.start(index) + strip.cell / 2. + f32::from(self.strip.offset().x);
            Some(self.strip_tooltip(line.row.name.clone(), centre, strip, palette))
        });
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
            .children(lines.into_iter().enumerate().map(|(index, line)| {
                let is_selected = self.selected.contains(&line.key);
                let this = this.clone();
                let this_menu = this.clone();
                let this_hover = this.clone();
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
                    .child(thumb_art(&line.row, strip.icon))
                    .on_hover(move |hovered: &bool, _, cx| {
                        this_hover
                            .update(cx, |this: &mut FinderDemo, cx| {
                                this.hover_strip(*hovered, index, cx)
                            })
                            .ok();
                    })
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
                    cell,
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
            .children(tip)
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

    /// Finder names a strip thumbnail in a callout after the pointer rests on it; once one shows,
    /// it follows the pointer from cell to cell and lingers briefly after the pointer leaves.
    fn hover_strip(&mut self, hovered: bool, index: usize, cx: &mut Context<Self>) {
        if !hovered {
            if self.strip_hover.map(|(at, _)| at) == Some(index) {
                self.strip_hover = None;
            }
            if self.strip_tip.is_some() {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(TIP_LINGER).await;
                    this.update(cx, |this, cx| {
                        if this.strip_hover.is_none() {
                            this.strip_tip = None;
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
            return;
        }
        let since = Instant::now();
        self.strip_hover = Some((index, since));
        if self.strip_tip.is_some() {
            self.strip_tip = Some(index);
            cx.notify();
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TIP_DELAY).await;
            this.update(cx, |this, cx| {
                if this.strip_hover == Some((index, since)) {
                    this.strip_tip = Some(index);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn strip_tooltip(
        &self,
        name: SharedString,
        centre: f32,
        strip: Strip,
        palette: &Palette,
    ) -> AnyElement {
        let dark = tokens::current_appearance() == Appearance::Dark;
        let fill = if dark {
            gpui::rgb(0x242424)
        } else {
            gpui::rgb(0xf6f6f6)
        };
        let border = if dark {
            gpui::rgba(0xffffff4d)
        } else {
            gpui::rgba(0x0000002e)
        };
        let arrow = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let top = bounds.origin.y;
                let left = bounds.origin.x;
                let w = bounds.size.width;
                let h = bounds.size.height;
                let mut outer = Path::new(point(left, top));
                outer.line_to(point(left + w / 2., top + h));
                outer.line_to(point(left + w, top));
                window.paint_path(outer, border);
                // Inset by a point on each edge and overlapping the bubble by one, so the fill
                // covers the bubble's border under the arrow.
                let mut inner = Path::new(point(left + px(1.4), top - px(1.)));
                inner.line_to(point(left + w / 2., top + h - px(1.4)));
                inner.line_to(point(left + w - px(1.4), top - px(1.)));
                window.paint_path(inner, fill);
            },
        )
        .w(px(2. * TIP_ARROW))
        .h(px(TIP_ARROW))
        .flex_none();
        div()
            .absolute()
            .left(px(centre))
            .bottom(px(strip.height() - strip.margin + TIP_GAP))
            .w(px(0.))
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .flex_none()
                    .h(px(TIP_H))
                    .px(px(TIP_PAD))
                    .rounded(px(TIP_RADIUS))
                    .bg(fill)
                    .border_1()
                    .border_color(border)
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .font_family(tokens::system_font_family())
                    .text_size(px(TIP_FONT))
                    .text_color(palette.label)
                    .child(name),
            )
            .child(arrow)
            .into_any_element()
    }
}
