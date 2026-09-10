use std::sync::Arc;

use gpui::{
    AnyElement, Entity, FontWeight, MouseButton, MouseDownEvent, RenderImage, SharedString, div,
    img, prelude::*, px,
};
use image::{Frame, RgbaImage};
use native_gpui::tokens::{
    self, Symbol, SymbolColor, SystemColor, sf_symbol_colored, system_color,
};

use super::INSET;
use super::stats::{CALCULATING, span};
use crate::app::FinderDemo;
use crate::art::{Poster, artwork, poster_art, row_art, square};
use finder_fs::meta::Meta;

use crate::data::{Face, Line, Row, byte_string, stamp, volume_at};
use crate::palette::Palette;
use crate::tags::{tag_dot, tag_pill};
use crate::ui::rename_field;
use crate::view::GALLERY_VIEW;

const NAME_SIZE: f32 = 15.;
const NAME_H: f32 = 18.;
const LINE_H: f32 = 15.;
const SMALL_H: f32 = 13.;
const GAP_NAME_KIND: f32 = 4.;
const GAP_KIND_HEAD: f32 = 9.;
const GAP_HEAD_ROWS: f32 = 8.5;
const ROW_GAP: f32 = 8.;
const GAP_ROWS_TAGS: f32 = 17.;
const GAP_TAGS_ADD: f32 = 6.5;
const ICON_H: f32 = 16.;
const GAP_ICON_LABEL: f32 = 10.5;
const BOTTOM_INSET: f32 = 16.;
const GAP_TOP: f32 = 10.;
const GAP_HERO_TEXT: f32 = 27.;
const HERO_TALL: f32 = 1.31;
const HERO_CAP: f32 = 0.5;
const HEADER_ICON: f32 = 48.;
const GAP_HEADER: f32 = 26.;
const GAP_HEADER_ICON: f32 = 10.;
const GAP_HEADER_HEAD: f32 = 14.;
const ACTION_SLOT_W: f32 = 75.5;
const ACTION_ICON_SIZE: f32 = 15.;
const STACK_ICON: f32 = 0.62;

pub(super) const STACK_LAYERS: [(f32, f32, f32, f32); 3] = [
    (-10., 0.03, -0.025, 0.9),
    (7., -0.025, 0., 0.95),
    (-3., 0., 0.02, 1.),
];

struct Card {
    hero: AnyElement,
    title: SharedString,
    subtitle: SharedString,
    rows: Vec<(&'static str, SharedString)>,
    show_more: bool,
    tags: Vec<usize>,
    many: bool,
    actions: &'static [(&'static str, &'static str)],
}

/// Straight-alpha BGRA in and out, as icons come.
fn stack_image(icons: &[Symbol], side: u32) -> Option<Arc<RenderImage>> {
    if icons.is_empty() {
        return None;
    }
    let layers = &STACK_LAYERS[3 - icons.len().min(3)..];
    let mut canvas = vec![0f32; (side * side * 4) as usize];
    let centre = side as f32 / 2.;
    for (icon, (degrees, dx, dy, zoom)) in icons.iter().take(3).rev().zip(layers) {
        let (w, h) = {
            let size = icon.image.size(0);
            (size.width.0 as usize, size.height.0 as usize)
        };
        let src = icon.image.as_bytes(0)?;
        let (sin, cos) = degrees.to_radians().sin_cos();
        let (cx, cy) = (centre + dx * side as f32, centre + dy * side as f32);
        for y in 0..side {
            for x in 0..side {
                let (px_, py) = (x as f32 - cx, y as f32 - cy);
                let u = (px_ * cos + py * sin) / zoom + w as f32 / 2.;
                let v = (-px_ * sin + py * cos) / zoom + h as f32 / 2.;
                if u < 0. || v < 0. || u >= (w - 1) as f32 || v >= (h - 1) as f32 {
                    continue;
                }
                let (x0, y0) = (u as usize, v as usize);
                let (fx, fy) = (u - x0 as f32, v - y0 as f32);
                let at = |x: usize, y: usize, c: usize| src[(y * w + x) * 4 + c] as f32;
                let mut s = [0f32; 4];
                for (c, slot) in s.iter_mut().enumerate() {
                    *slot = at(x0, y0, c) * (1. - fx) * (1. - fy)
                        + at(x0 + 1, y0, c) * fx * (1. - fy)
                        + at(x0, y0 + 1, c) * (1. - fx) * fy
                        + at(x0 + 1, y0 + 1, c) * fx * fy;
                }
                let a = s[3] / 255.;
                if a <= 0. {
                    continue;
                }
                let d = &mut canvas[((y * side + x) * 4) as usize..][..4];
                let da = d[3] / 255.;
                let out = a + da * (1. - a);
                for c in 0..3 {
                    d[c] = (s[c] * a + d[c] * da * (1. - a)) / out;
                }
                d[3] = out * 255.;
            }
        }
    }
    let bytes: Vec<u8> = canvas
        .iter()
        .map(|v| v.round().clamp(0., 255.) as u8)
        .collect();
    let raster = RgbaImage::from_raw(side, side, bytes)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(raster)])))
}

fn title_line(row: &Row) -> SharedString {
    match row.display.strip_suffix(".app") {
        Some(stem) if row.face == Face::Program => stem.to_string().into(),
        _ => row.display.clone(),
    }
}

fn kind_line(row: &Row, weighed: Option<u64>) -> SharedString {
    match (row.size.as_ref(), weighed) {
        ("--", None) => row.kind.clone(),
        ("--", Some(bytes)) => format!("{} - {}", row.kind, byte_string(bytes)).into(),
        (size, _) => format!("{} - {size}", row.kind).into(),
    }
}

fn info_rows(
    row: &Row,
    meta: Option<&Meta>,
    expanded: bool,
) -> (Vec<(&'static str, SharedString)>, bool) {
    let none = Meta::default();
    let meta = match row.face {
        Face::Folder | Face::Volume => &none,
        _ => meta.unwrap_or(&none),
    };
    let size = |(w, h): (u64, u64)| SharedString::from(format!("{w}\u{d7}{h}"));
    let mut all: Vec<(&'static str, SharedString, bool)> = Vec::new();
    if let Some(volume) = volume_at(&row.path) {
        all.push(("Capacity", byte_string(volume.capacity).into(), false));
        all.push(("Available", byte_string(volume.available).into(), false));
    }
    all.push(("Created", stamp(row.created_at), false));
    all.push(("Modified", stamp(row.modified_at), false));
    let mut add = |label, value: Option<SharedString>, under_more| {
        all.extend(value.map(|value| (label, value, under_more)));
    };
    add("Last opened", meta.last_used.map(stamp), false);
    let media = meta.pixels.is_some() || meta.duration.is_some();
    add(
        "Content created",
        meta.content_created.filter(|_| media).map(stamp),
        true,
    );
    add("Dimensions", meta.pixels.map(size), false);
    add("Duration", meta.duration.map(clock), false);
    add(
        "Version",
        meta.version.clone().map(SharedString::from),
        row.face != Face::Program,
    );
    add(
        "Pages",
        meta.pages.map(|pages| pages.to_string().into()),
        true,
    );
    add("Resolution", meta.page.map(size), true);
    add(
        "Security",
        meta.security.clone().map(SharedString::from),
        true,
    );
    add(
        "Color space",
        meta.color_space.clone().map(SharedString::from),
        true,
    );
    add(
        "Color profile",
        meta.color_profile.clone().map(SharedString::from),
        true,
    );
    add(
        "Encoding software",
        (!meta.encoders.is_empty()).then(|| meta.encoders.join(", ").into()),
        true,
    );
    add(
        "Category",
        meta.category.clone().map(SharedString::from),
        true,
    );
    let more = all.iter().any(|(_, _, under_more)| *under_more);
    let rows = all
        .into_iter()
        .filter(|(_, _, under_more)| expanded || !under_more)
        .map(|(label, value, _)| (label, value))
        .collect();
    (rows, more)
}

fn clock(seconds: f64) -> SharedString {
    let seconds = seconds.max(0.).round() as u64;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    match hours {
        0 => format!("{minutes}:{seconds:02}").into(),
        _ => format!("{hours}:{minutes:02}:{seconds:02}").into(),
    }
}

fn actions(face: Face) -> &'static [(&'static str, &'static str)] {
    match face {
        Face::Movie => &[
            ("rotate.left", "Rotate Left"),
            ("timeline.selection", "Trim"),
            ("ellipsis.circle", "More…"),
        ],
        Face::Image => &[("rotate.left", "Rotate Left"), ("ellipsis.circle", "More…")],
        _ => &[("ellipsis.circle", "More…")],
    }
}

impl FinderDemo {
    pub(crate) fn info_card(
        &self,
        line: Option<&Line>,
        palette: &Palette,
        content: f32,
        finder: &Entity<FinderDemo>,
    ) -> AnyElement {
        let card = if self.selected.len() > 1 {
            self.many_card(content)
        } else {
            let Some(line) = line else {
                return div().into_any_element();
            };
            let row = &line.row;
            let meta = crate::meta::meta(&row.path);
            let (rows, show_more) = info_rows(row, meta.as_deref(), self.preview_expanded);
            Card {
                hero: if self.view == GALLERY_VIEW {
                    row_art(row, HEADER_ICON, self.scale, true)
                } else {
                    poster_art(row, content, self.hero_h(content), self.scale, Poster::Card)
                },
                title: title_line(row),
                subtitle: kind_line(row, self.weighed()),
                rows,
                show_more,
                tags: self.tags.get(&line.key).cloned().unwrap_or_default(),
                many: false,
                actions: actions(row.face),
            }
        };
        self.card_element(card, palette, content, finder)
    }

    fn weighed(&self) -> Option<u64> {
        self.stats
            .as_ref()
            .filter(|stats| stats.key == self.stats_key())
            .map(|stats| stats.bytes)
            .filter(|bytes| *bytes > 0)
    }

    pub(crate) fn hero_h(&self, content: f32) -> f32 {
        (content * HERO_TALL).min(self.pane_h * HERO_CAP)
    }

    fn many_card(&self, content: f32) -> Card {
        let side = if self.view == GALLERY_VIEW {
            HEADER_ICON
        } else {
            content.min(self.hero_h(content))
        };
        let hero = match self.stats.as_ref() {
            Some(stats) => self.stack_hero(&stats.head, side),
            None => div().w(px(side)).h(px(side)).into_any_element(),
        };
        let actions: &'static [(&'static str, &'static str)] =
            &[("ellipsis.circle", "More\u{2026}")];
        let Some(stats) = self
            .stats
            .as_ref()
            .filter(|stats| stats.key == self.stats_key())
        else {
            return Card {
                hero,
                title: format!("{} items", self.selected.len()).into(),
                subtitle: CALCULATING.into(),
                rows: vec![
                    ("Created", CALCULATING.into()),
                    ("Modified", CALCULATING.into()),
                ],
                show_more: false,
                tags: Vec::new(),
                many: true,
                actions,
            };
        };
        let files = stats.items - stats.folders;
        let kinds = match (files, stats.folders) {
            (0, n) => format!("{n} folders"),
            (n, 0) => format!("{n} documents"),
            (f, d) => format!("{f} documents, {d} folders"),
        };
        Card {
            hero,
            title: format!("{} items", stats.items).into(),
            subtitle: format!("{kinds} - {}", byte_string(stats.bytes)).into(),
            rows: vec![
                ("Created", span(stats.created)),
                ("Modified", span(stats.modified)),
            ],
            show_more: false,
            tags: stats.tags.clone(),
            many: true,
            actions,
        }
    }

    pub(crate) fn stack_hero(&self, lines: &[Line], content: f32) -> AnyElement {
        let Some(first) = lines.first() else {
            return div().w(px(content)).h(px(content)).into_any_element();
        };
        let icons: Vec<Symbol> = lines
            .iter()
            .take(3)
            .filter_map(|line| line.row.artwork)
            .filter_map(|kind| crate::icons::icon(kind, content * STACK_ICON))
            .collect();
        let keys: Vec<SharedString> = lines.iter().take(3).map(|line| line.key.clone()).collect();
        let side = icons
            .first()
            .map(|icon| (content * icon.scale).round() as u32)
            .unwrap_or_default();
        let cached = self
            .stack
            .borrow()
            .as_ref()
            .filter(|(k, s, _)| *k == keys && *s == side)
            .map(|(_, _, image)| image.clone());
        let drawn = lines
            .iter()
            .take(3)
            .filter(|line| line.row.artwork.is_some())
            .count()
            == icons.len();
        let image = cached.or_else(|| {
            let image = stack_image(&icons, side)?;
            if drawn {
                *self.stack.borrow_mut() = Some((keys, side, image.clone()));
            }
            Some(image)
        });
        let art = match image {
            Some(image) => img(image).w(px(content)).h(px(content)).into_any_element(),
            None => artwork(&first.row, content),
        };
        square(content).child(art).into_any_element()
    }

    fn card_element(
        &self,
        card: Card,
        palette: &Palette,
        content: f32,
        finder: &Entity<FinderDemo>,
    ) -> AnyElement {
        let body = tokens::system_font_size();
        let small = tokens::small_system_font_size();
        let hairline = system_color(SystemColor::QuaternarySystemFill);
        let editing = (self.view == GALLERY_VIEW && !card.many)
            .then(|| self.cursor_line())
            .flatten()
            .filter(|line| self.renaming.as_ref() == Some(&line.key));
        let title: AnyElement = match editing {
            Some(line) => rename_field(&line, 0, finder.downgrade()).into_any_element(),
            None => card.title.into_any_element(),
        };

        let name = div()
            .line_height(px(NAME_H))
            .text_size(px(NAME_SIZE))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(palette.label)
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .child(title);
        let kind = div()
            .mt(px(GAP_NAME_KIND))
            .line_height(px(LINE_H))
            .text_size(body)
            .text_color(palette.secondary)
            .child(card.subtitle);

        let mut top = div()
            .size_full()
            .flex()
            .flex_col()
            .px(px(INSET))
            .pb(px(BOTTOM_INSET));
        let mut lower = div().flex_none().flex().flex_col();
        if self.view == GALLERY_VIEW {
            top = top.child(div().h(px(GAP_HEADER)).flex_none()).child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(GAP_HEADER_ICON))
                    .child(card.hero)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(name)
                            .child(kind),
                    ),
            );
            lower = lower.mt(px(GAP_HEADER_HEAD - GAP_KIND_HEAD));
        } else {
            top = top.child(div().h(px(GAP_TOP)).flex_none()).child(
                div()
                    .w(px(content))
                    .h(px(self.hero_h(content)))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .child(card.hero),
            );
            lower = lower.mt(px(GAP_HERO_TEXT)).child(name).child(kind);
        }
        lower = lower.child(
            div()
                .mt(px(GAP_KIND_HEAD))
                .flex()
                .items_baseline()
                .justify_between()
                .line_height(px(LINE_H))
                .text_size(body)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.label)
                .child("Information")
                .when(card.show_more, |heading| {
                    heading.child(
                        div()
                            .line_height(px(SMALL_H))
                            .text_size(small)
                            .font_weight(FontWeight::NORMAL)
                            .text_color(system_color(SystemColor::Link))
                            .on_mouse_down(MouseButton::Left, {
                                let finder = finder.clone();
                                move |_: &MouseDownEvent, _, cx| {
                                    finder.update(cx, |this, cx| {
                                        this.preview_expanded = !this.preview_expanded;
                                        cx.notify();
                                    })
                                }
                            })
                            .child(if self.preview_expanded {
                                "Show Less"
                            } else {
                                "Show More"
                            }),
                    )
                }),
        );

        let mut block = div().mt(px(GAP_HEAD_ROWS)).flex().flex_col();
        for (index, (label, value)) in card.rows.iter().enumerate() {
            if index > 0 {
                block = block.child(
                    div()
                        .h(px(ROW_GAP))
                        .flex()
                        .flex_col()
                        .justify_center()
                        .child(div().h(px(1.)).bg(hairline)),
                );
            }
            block = block.child(
                div()
                    .flex()
                    .justify_between()
                    .gap(px(8.))
                    .line_height(px(LINE_H))
                    .text_size(body)
                    .child(
                        div()
                            .flex_none()
                            .text_color(palette.secondary)
                            .child(*label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_right()
                            .text_color(palette.label)
                            .child(value.clone()),
                    ),
            );
        }
        lower = lower.child(block).child(
            div()
                .mt(px(GAP_ROWS_TAGS))
                .line_height(px(LINE_H))
                .text_size(body)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.label)
                .child("Tags"),
        );

        let tags = div()
            .mt(px(GAP_TAGS_ADD))
            .flex()
            .items_center()
            .gap(px(5.))
            .line_height(px(LINE_H))
            .text_size(body);
        lower = lower.child(match (card.many, card.tags.first()) {
            (false, Some(tag)) => tags.child(tag_dot(*tag)).child(
                div()
                    .text_color(palette.label)
                    .child(crate::tags::tags()[*tag].name.clone()),
            ),
            (false, None) => tags.text_color(palette.tertiary).child("Add Tags…"),
            (true, _) => tags
                .children(card.tags.iter().map(|tag| tag_pill(*tag, palette.label)))
                .child(
                    div()
                        .ml(px(3.))
                        .text_color(system_color(SystemColor::Link))
                        .child("Add Tag"),
                ),
        });

        let actions = card.actions;
        let slot = ACTION_SLOT_W.min(content / actions.len() as f32);
        top.child(lower)
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .flex()
                    .justify_center()
                    .children(actions.iter().map(|(symbol, label)| {
                        div()
                            .w(px(slot))
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(
                                div().h(px(ICON_H)).flex().items_center().children(
                                    sf_symbol_colored(
                                        symbol,
                                        px(ACTION_ICON_SIZE),
                                        FontWeight::NORMAL,
                                        &SymbolColor::Hierarchical(SystemColor::SecondaryLabel),
                                    )
                                    .map(|glyph| glyph.img()),
                                ),
                            )
                            .child(
                                div()
                                    .mt(px(GAP_ICON_LABEL))
                                    .line_height(px(SMALL_H))
                                    .text_size(small)
                                    .text_color(palette.secondary)
                                    .child(*label),
                            )
                    })),
            )
            .into_any_element()
    }
}
