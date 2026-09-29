use std::ops::Range;
use std::sync::LazyLock;

use gpui::{AnyElement, FontWeight, Pixels, Point, Window, div, point, prelude::*, px};
use native_gpui::tokens;
use native_gpui::{native_callout, native_scroll_bars};

use crate::app::FinderDemo;
use crate::art::{Poster, art_at, item_art, poster_art};
use crate::elastic::stretching;
use crate::menus::tiles_menu;
use crate::palette::Palette;
use crate::prefs;
use crate::ui::pick_on_click;
use crate::view::GALLERY_VIEW;

const STRIP_INSET: f32 = 5.;
const STRIP_RADIUS: f32 = 8.;
const HERO_INSET: f32 = 61.;
const OPTIONS: &str = "GalleryViewOptions";

static TITLE_H: LazyLock<f32> = LazyLock::new(|| {
    let font = tokens::system_font_metrics(tokens::small_system_font_size(), FontWeight::NORMAL);
    (font.ascent + font.descent.abs()).ceil()
});

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct GalleryLook {
    pub(crate) thumb: f32,
    pub(crate) filename: bool,
}

impl GalleryLook {
    pub(crate) fn load(folder: &str) -> Self {
        let thumb = prefs::look_number(GALLERY_VIEW, folder, "iconSize")
            .or_else(|| prefs::finder_look_number("GalleryViewSettings", "iconSize"));
        Self {
            thumb: thumb.map_or(48., |size| size as f32),
            filename: prefs::view_option(OPTIONS, "ShowTitles").is_some_and(|on| on != 0.),
        }
    }
}

#[derive(Clone, Copy)]
struct Strip {
    icon: f32,
    cell: f32,
    gap: f32,
    margin: f32,
    title: f32,
}

impl Strip {
    fn at(look: GalleryLook) -> Self {
        let icon = look.thumb;
        Self {
            icon,
            cell: icon + 6.,
            gap: icon / 8. - 1.,
            margin: icon / 16. + 3.,
            title: if look.filename { *TITLE_H } else { 0. },
        }
    }

    fn height(self) -> f32 {
        self.cell + 2. * self.margin + self.title
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
        let strip = Strip::at(self.gallery_look);
        let room_w = self.list_room;
        let room_h = f32::from(window.viewport_size().height)
            - f32::from(self.metrics().top)
            - strip.height();
        let size = (room_w.min(room_h) - 2. * HERO_INSET).max(64.);
        let scale = self.scale;
        let previews = self.previews.gallery;
        if chosen.len() <= 1 {
            self.ask_beside_cursor(size);
        }
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
        let shift = self.bands.strip.shown().x;
        let hit = move |demo: &FinderDemo, at: Point<Pixels>| {
            let x = f32::from(at.x - hit_strip.offset().x - shift) - STRIP_INSET;
            let index = x / (strip.cell + strip.gap);
            if x < 0. || index.fract() * (strip.cell + strip.gap) > strip.cell {
                return None;
            }
            demo.lines(false).get(index as usize).cloned()
        };
        let seen = strip.seen(f32::from(self.strip.offset().x), room_w, lines.len());
        let seen_titles = seen.clone();
        let strip_row = div()
            .id("ng.finder.filmstrip")
            .h(px(strip.height()))
            .flex_none()
            .relative()
            .overflow_x_scroll()
            .track_scroll(&self.strip)
            .on_scroll_wheel(stretching(
                |this| (&mut this.bands.strip, this.strip.clone()),
                cx,
            ))
            .child(div().w(px(strip.start(lines.len()))).h_full().flex_none())
            .children(seen.map(|index| {
                let line = &lines[index];
                let is_selected = self.selected.contains(&line.key);
                let cell = div()
                    .id(("ng.finder.frame", index))
                    .absolute()
                    .left(px(strip.start(index)) + shift)
                    .top(px(strip.margin))
                    .w(px(strip.cell))
                    .h(px(strip.cell))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(STRIP_RADIUS))
                    .when(is_selected, |cell| cell.bg(palette.tertiary))
                    .child(item_art(&line.row, strip.icon, scale, previews))
                    .on_click(pick_on_click(line, this.clone()));
                native_callout(
                    ("ng.finder.strip.tip", index),
                    line.row.display.clone(),
                    cell,
                )
            }))
            .when(strip.title > 0., |row| {
                row.children(seen_titles.map(|index| {
                    div()
                        .absolute()
                        .left(px(strip.start(index) - strip.gap / 2.) + shift)
                        .top(px(strip.margin + strip.cell))
                        .w(px(strip.cell + strip.gap))
                        .h(px(strip.title))
                        .font_family(tokens::system_font_family())
                        .text_size(tokens::small_system_font_size())
                        .text_color(palette.label)
                        .text_center()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis_middle()
                        .child(lines[index].row.display.clone())
                }))
            });

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
        let strip = Strip::at(self.gallery_look);
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

    pub(crate) fn gallery_look(&self) -> GalleryLook {
        self.gallery_look
    }

    pub(crate) fn change_gallery_look(
        &mut self,
        change: impl FnOnce(&mut GalleryLook),
        cx: &mut Context<Self>,
    ) {
        let was = self.gallery_look;
        change(&mut self.gallery_look);
        let look = self.gallery_look;
        if look.thumb != was.thumb {
            prefs::set_look_number(GALLERY_VIEW, self.here(), "iconSize", f64::from(look.thumb));
        }
        if look.filename != was.filename {
            prefs::set_view_option(OPTIONS, "ShowTitles", f64::from(u8::from(look.filename)));
        }
        cx.notify();
    }
}
