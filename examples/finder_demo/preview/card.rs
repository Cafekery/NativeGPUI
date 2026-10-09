use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use finder_fs::meta::Meta;
use gpui::{
    AnyElement, App, Entity, FontWeight, Hsla, MouseButton, MouseDownEvent, RenderImage,
    SharedString, div, hsla, img, prelude::*, px,
};
use image::{Frame, RgbaImage};
use native_gpui::tokens::{
    self, Appearance, SymbolColor, SystemColor, sf_symbol_colored, system_color,
};
use native_gpui::{native_context_menu, native_tooltip};
use objc2::rc::Retained;
use objc2_app_kit::{NSEvent, NSEventModifierFlags};
use objc2_foundation::{
    NSCalendarUnit, NSDateComponentsFormatter, NSDateComponentsFormatterUnitsStyle,
    NSDateComponentsFormatterZeroFormattingBehavior, NSNumber, NSNumberFormatter,
};

use super::options::{self, Source};
use super::stats::{CALCULATING, span};
use super::{INSET, RESIZE_EDGE};
use crate::app::FinderDemo;
use crate::art::{Poster, artwork, poster_art, row_art, square};
use crate::data::{Face, Line, Row, byte_string, date_text, small_text_width, stamp, volume_at};
use crate::menus::Act;
use crate::palette::Palette;
use crate::quick_actions::{QuickAct, menu_rows, offered};
use crate::signal::Signal;
use crate::tags::{RowTag, row_tags, tag_dot, tag_pill};
use crate::thumbs::{awaited, thumb};
use crate::ui::rename_field;
use crate::view::{COLUMN_VIEW, GALLERY_VIEW};

const NAME_SIZE: f32 = 15.;
const NAME_H: f32 = 18.;
const LINE_H: f32 = 15.;
const SMALL_H: f32 = 13.;
const ROW_H: f32 = 16.;
const GAP_NAME_KIND: f32 = 4.75;
const GAP_KIND_HEAD: f32 = 9.;
const GAP_HEAD_ROWS: f32 = 7.25;
const ROW_GAP: f32 = 7.;
const ROW_TITLE_GAP: f32 = 10.;
const ROW_FIT_MARGIN: f32 = 14.;
const GAP_ROWS_TAGS: f32 = 17.25;
const GAP_TAGS_ADD: f32 = 5.9;
const TAGS_WELL_H: f32 = 33.;
const TAGS_TEXT_TOP: f32 = 1.;
const BOTTOM_PAD: f32 = 16.;
const ICON_H: f32 = 16.5;
const GAP_ICON_LABEL: f32 = 9.5;
const GAP_TOP: f32 = 10.;
const GAP_HERO_TEXT: f32 = 27.3;
const HERO_TALL: f32 = 1.25;
const HERO_EXTRA: f32 = 15.;
const HERO_CAP: f32 = 0.48;
const HERO_CAP_EXTRA: f32 = 49.2;
const HEADER_ICON: f32 = 50.;
const GAP_HEADER: f32 = 19.;
const GAP_HEADER_ICON: f32 = 9.;
const GAP_HEADER_NAME: f32 = 6.8;
const GAP_HEADER_HEAD: f32 = 19.6;
const ACTIONS_H: f32 = 70.;
const ACTION_TOP: f32 = 15.5;
const ACTION_SLOT_W: f32 = 76.;
const ACTION_ICON_SIZE: f32 = 14.;
const COLUMN_BOTTOM: f32 = 16.;
const STACK_ICON: f32 = 0.62;
const STACKS_KEPT: usize = 4;
const ACTION_BUTTONS: usize = 2;
const BY: &str = "\u{200a}\u{d7}\u{200a}";

pub(super) const STACK_LAYERS: [(f32, f32, f32, f32); 3] = [
    (-10., 0.03, -0.025, 0.9),
    (7., -0.025, 0., 0.95),
    (-3., 0., 0.02, 1.),
];

enum Hero {
    Row(Arc<Row>),
    Poster(Arc<Row>),
    Stack(Vec<Line>),
    Blank,
}

struct Card {
    hero: Hero,
    title: SharedString,
    subtitle: SharedString,
    rows: Vec<(&'static str, SharedString)>,
    show_more: bool,
    tags: Vec<RowTag>,
    show_tags: bool,
    many: bool,
    actions: Option<Vec<QuickAct>>,
}

/// A snapshot: a pane reading its window's state would make that window redraw.
pub(crate) struct Shown {
    card: Card,
    content: f32,
    view: usize,
    scale: f32,
    pane_h: f32,
    key: bool,
    expanded: bool,
    renaming: Option<Line>,
    pub(super) list_focused: bool,
    option: bool,
}

/// Vibrant text over the pane's glass, which no `NSColor` names.
struct Ink {
    name: Hsla,
    text: Hsla,
    title: Hsla,
    kind: Hsla,
    rule: Hsla,
    action: Hsla,
}

impl Ink {
    fn now(key: bool) -> Self {
        match tokens::current_appearance() {
            Appearance::Dark => {
                let white = |a| hsla(0., 0., 1., a);
                Self {
                    name: white(0.9),
                    text: white(0.9),
                    title: white(0.36),
                    kind: white(0.18),
                    rule: white(0.138),
                    action: white(0.5),
                }
            }
            Appearance::Light => {
                let black = |a| hsla(0., 0., 0., a);
                // Light vibrant text darkens the glass by a fixed depth; the glass lightens off key.
                let glass = if key { 237. } else { 242. };
                let under = |depth: f32| hsla(0., 0., (glass - depth) / 255., 1.);
                Self {
                    name: black(0.7),
                    text: under(178.5),
                    title: under(102.),
                    kind: under(51.5),
                    rule: black(0.094),
                    action: black(0.5),
                }
            }
        }
    }

    fn plain() -> Self {
        let label = system_color(SystemColor::Label);
        let secondary = system_color(SystemColor::SecondaryLabel);
        Self {
            name: label,
            text: label,
            title: secondary,
            kind: secondary,
            rule: system_color(SystemColor::Separator),
            action: secondary,
        }
    }
}

/// Straight-alpha BGRA in and out.
fn stack_image(images: &[Arc<RenderImage>], side: u32) -> Option<Arc<RenderImage>> {
    if images.is_empty() {
        return None;
    }
    let layers = &STACK_LAYERS[3 - images.len().min(3)..];
    let mut canvas = vec![0f32; (side * side * 4) as usize];
    let centre = side as f32 / 2.;
    for (image, (degrees, dx, dy, zoom)) in images.iter().take(3).rev().zip(layers) {
        let (w, h) = {
            let size = image.size(0);
            (size.width.0 as usize, size.height.0 as usize)
        };
        let src = image.as_bytes(0)?;
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

/// `image` is `None` while the stack is being made.
struct Stacked {
    from: Vec<Arc<RenderImage>>,
    side: u32,
    image: Option<Option<Arc<RenderImage>>>,
}

impl Stacked {
    fn made_from(&self, from: &[Arc<RenderImage>], side: u32) -> bool {
        self.side == side
            && self.from.len() == from.len()
            && self.from.iter().zip(from).all(|(a, b)| Arc::ptr_eq(a, b))
    }
}

struct Stacks {
    making: bool,
    oldest_first: Vec<Stacked>,
}

static STACKS: Mutex<Stacks> = Mutex::new(Stacks {
    making: false,
    oldest_first: Vec::new(),
});
static RETIRED: Mutex<Vec<Arc<RenderImage>>> = Mutex::new(Vec::new());
static STACKED: Signal = Signal::new();

fn stacked(from: Vec<Arc<RenderImage>>, side: u32) -> Option<Arc<RenderImage>> {
    let mut stacks = STACKS.lock().unwrap();
    if let Some(stack) = stacks
        .oldest_first
        .iter()
        .find(|stack| stack.made_from(&from, side))
    {
        return stack.image.clone().flatten();
    }
    stacks.oldest_first.push(Stacked {
        from,
        side,
        image: None,
    });
    if stacks.oldest_first.len() > STACKS_KEPT {
        let gone = stacks.oldest_first.remove(0);
        RETIRED.lock().unwrap().extend(gone.image.flatten());
    }
    if !std::mem::replace(&mut stacks.making, true) {
        std::thread::spawn(make_stacks);
    }
    None
}

/// Newest first: a pane being resized asks for one per frame.
fn make_stacks() {
    loop {
        let wanted = {
            let mut stacks = STACKS.lock().unwrap();
            let wanted = stacks
                .oldest_first
                .iter()
                .rev()
                .find(|stack| stack.image.is_none())
                .map(|stack| (stack.from.clone(), stack.side));
            stacks.making = wanted.is_some();
            wanted
        };
        let Some((from, side)) = wanted else {
            return;
        };
        let image = stack_image(&from, side);
        if let Some(stack) = STACKS
            .lock()
            .unwrap()
            .oldest_first
            .iter_mut()
            .find(|stack| stack.made_from(&from, side))
        {
            stack.image = Some(image);
        }
        STACKED.ring();
    }
}

pub(crate) fn follow_stacks(cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut seen = 0;
        loop {
            seen = STACKED.after(seen).await;
            let retired = std::mem::take(&mut *RETIRED.lock().unwrap());
            cx.update(|cx| {
                for image in retired {
                    cx.drop_image(image, None);
                }
                crate::read::notify_windows(cx);
            });
        }
    })
    .detach();
}

#[derive(PartialEq)]
pub(super) struct CardKey {
    view: usize,
    stats: ((u64, usize), Option<(u64, usize)>),
    line: Option<(SharedString, usize)>,
    tags: Option<Vec<RowTag>>,
    meta: usize,
    custom: bool,
    thumbs: Vec<usize>,
    stacks: u64,
    volume: Option<(u64, u64)>,
    size: [u32; 3],
    look: (bool, bool, u64),
    expanded: bool,
    renaming: Option<SharedString>,
    option: bool,
    options: u64,
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
    kind: &'static options::Kind,
    meta: Option<&Meta>,
    expanded: bool,
    content: f32,
) -> (Vec<(&'static str, SharedString)>, bool) {
    let none = Meta::default();
    let meta = meta
        .filter(|_| !matches!(row.face, Face::Volume))
        .unwrap_or(&none);
    let size = |(w, h): (u64, u64)| SharedString::from(format!("{w}{BY}{h}"));
    let value = |name: &str| {
        meta.values
            .iter()
            .find(|(attribute, _)| attribute == name)
            .map(|(_, value)| value.clone())
    };
    let date = |label, at| stamp(at, content - label_width(label) - ROW_FIT_MARGIN);
    let mut all: Vec<(&'static str, SharedString, bool)> = Vec::new();
    if let Some(volume) = volume_at(&row.path) {
        all.push(("Capacity", byte_string(volume.capacity).into(), true));
        all.push(("Available", byte_string(volume.available).into(), true));
    }
    for field in kind.groups.iter().flat_map(|group| group.fields) {
        let shown = match field.source {
            Source::Created => Some(date(field.label, row.created_at)),
            Source::Modified => Some(date(field.label, row.modified_at)),
            Source::LastOpened => meta.last_used.map(|at| date(field.label, at)),
            Source::ContentCreated => meta.content_created.map(date_text),
            Source::Dimensions => meta.pixels.map(size),
            Source::PageSize => meta.page.map(size),
            Source::Dpi => value("kMDItemResolutionWidthDPI")
                .zip(value("kMDItemResolutionHeightDPI"))
                .map(|(w, h)| format!("{w}{BY}{h}").into()),
            Source::Duration => meta.duration.and_then(duration),
            Source::Attr(name) => value(name).map(|text| attr_text(name, text)),
        };
        all.extend(shown.map(|shown| (field.label, shown, options::shows(kind, field.key()))));
    }
    let more = all.iter().any(|(_, _, shown)| !shown);
    let rows = all
        .into_iter()
        .filter(|(_, _, shown)| expanded || *shown)
        .map(|(label, value, _)| (label, value))
        .collect();
    (rows, more)
}

fn label_width(label: &'static str) -> f32 {
    thread_local! {
        static WIDTHS: RefCell<HashMap<&'static str, f32>> = RefCell::new(HashMap::new());
    }
    WIDTHS.with_borrow_mut(|widths| {
        *widths
            .entry(label)
            .or_insert_with(|| small_text_width(label))
    })
}

pub(crate) fn duration(seconds: f64) -> Option<SharedString> {
    thread_local! {
        static FORMATTER: Retained<NSDateComponentsFormatter> = {
            let formatter = NSDateComponentsFormatter::new();
            formatter.setUnitsStyle(NSDateComponentsFormatterUnitsStyle::Positional);
            formatter.setZeroFormattingBehavior(NSDateComponentsFormatterZeroFormattingBehavior::Pad);
            formatter
        };
    }
    let seconds = seconds.max(0.).round();
    let units = match seconds >= 3600. {
        true => NSCalendarUnit::Hour | NSCalendarUnit::Minute | NSCalendarUnit::Second,
        false => NSCalendarUnit::Minute | NSCalendarUnit::Second,
    };
    FORMATTER.with(|formatter| {
        formatter.setAllowedUnits(units);
        formatter
            .stringFromTimeInterval(seconds)
            .map(|text| text.to_string().into())
    })
}

fn attr_text(name: &str, text: String) -> SharedString {
    match name {
        "kMDItemAudioSampleRate" => text
            .parse()
            .ok()
            .and_then(kilohertz)
            .unwrap_or_else(|| text.into()),
        "kMDItemAudioChannelCount" if text == "1" => "Mono".into(),
        "kMDItemAudioChannelCount" if text == "2" => "Stereo".into(),
        // Finder lists Spotlight's codecs last first.
        "kMDItemCodecs" => text.rsplit(", ").collect::<Vec<_>>().join(", ").into(),
        _ => text.into(),
    }
}

fn kilohertz(hertz: f64) -> Option<SharedString> {
    thread_local! {
        static FORMATTER: Retained<NSNumberFormatter> = {
            let formatter = NSNumberFormatter::new();
            formatter.setMaximumFractionDigits(1);
            formatter
        };
    }
    FORMATTER
        .with(|formatter| formatter.stringFromNumber(&NSNumber::new_f64(hertz / 1000.)))
        .map(|number| format!("{number} kHz").into())
}

fn action_slots(
    acts: Option<&[QuickAct]>,
    option: bool,
) -> Vec<(Option<QuickAct>, &'static str, &'static str)> {
    let Some(acts) = acts else {
        return Vec::new();
    };
    acts.iter()
        .take(ACTION_BUTTONS)
        .map(|act| {
            let shown = act.alternate().filter(|_| option).unwrap_or(*act);
            (Some(*act), shown.symbol(), shown.title())
        })
        .chain([(None, "ellipsis.circle", "More\u{2026}")])
        .collect()
}

fn option_held() -> bool {
    NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Option)
}

impl FinderDemo {
    pub(crate) fn info_card(
        &self,
        line: Option<&Line>,
        palette: &Palette,
        content: f32,
        finder: &Entity<FinderDemo>,
    ) -> AnyElement {
        match self.shown(line, content) {
            Some(shown) => shown.element(palette, finder, &self.stack),
            None => div().into_any_element(),
        }
    }

    pub(super) fn shown(&self, line: Option<&Line>, content: f32) -> Option<Shown> {
        let card = if self.selected.len() > 1 {
            self.many_card()
        } else {
            let line = line?;
            let row = &line.row;
            let meta = crate::meta::meta(&row.path);
            let kind = options::kind_of(row);
            let (rows, show_more) =
                info_rows(row, kind, meta.as_deref(), self.preview_expanded, content);
            Card {
                hero: match self.view == GALLERY_VIEW {
                    true => Hero::Row(row.clone()),
                    false => Hero::Poster(row.clone()),
                },
                title: row.display.clone(),
                subtitle: kind_line(row, self.fresh_stats().map(|stats| stats.bytes)),
                rows,
                show_more,
                tags: row_tags(&line.key),
                show_tags: options::shows(kind, options::TAGS),
                many: false,
                actions: options::shows(kind, options::QUICK_ACTIONS).then(|| offered(&[row.face])),
            }
        };
        let renaming = (self.view == GALLERY_VIEW && !card.many)
            .then(|| self.cursor_line())
            .flatten()
            .filter(|line| self.renaming.as_ref() == Some(&line.key));
        Some(Shown {
            card,
            content,
            view: self.view,
            scale: self.scale,
            pane_h: self.pane_h,
            key: self.host.as_ref().is_none_or(|host| host.isKeyWindow()),
            expanded: self.preview_expanded,
            renaming,
            list_focused: self.list_focused,
            option: option_held(),
        })
    }

    /// Asks for the hero's pictures just as drawing it would.
    pub(super) fn card_key(&self, content: f32, key: bool) -> CardKey {
        let many = self.selected.len() > 1;
        let line = self.cursor_line().filter(|_| !many);
        let row = line.as_ref().map(|line| &line.row);
        if row.is_some() && self.view != GALLERY_VIEW {
            self.ask_beside_cursor(self.hero_side(content));
        }
        let thumbs = match (&self.stats, row) {
            (Some(stats), _) if many => stack_thumbs(
                &stats.head,
                stack_side(self.view, self.pane_h, content),
                self.scale,
            ),
            (_, Some(row)) if self.view == GALLERY_VIEW => {
                vec![thumb(row, HEADER_ICON, self.scale, true, true)]
            }
            (_, Some(row)) => vec![thumb(row, self.hero_side(content), self.scale, true, false)],
            _ => Vec::new(),
        };
        CardKey {
            view: self.view,
            stats: (self.stats_key(), self.stats.as_ref().map(|stats| stats.key)),
            tags: line.as_ref().map(|line| row_tags(&line.key)),
            meta: row
                .and_then(|row| crate::meta::meta(&row.path))
                .map_or(0, |meta| Arc::as_ptr(&meta) as usize),
            custom: row.is_some_and(|row| crate::art::custom_folder(row)),
            line: line
                .as_ref()
                .map(|line| (line.key.clone(), Arc::as_ptr(&line.row) as usize)),
            thumbs: thumbs
                .iter()
                .map(|image| {
                    image
                        .as_ref()
                        .map_or(0, |image| Arc::as_ptr(image) as usize)
                })
                .collect(),
            stacks: STACKED.count(),
            volume: row
                .and_then(|row| volume_at(&row.path))
                .map(|volume| (volume.capacity, volume.available)),
            size: [
                content.to_bits(),
                self.pane_h.to_bits(),
                self.scale.to_bits(),
            ],
            look: (self.list_focused, key, tokens::generation()),
            expanded: self.preview_expanded,
            renaming: self.renaming.clone(),
            option: option_held(),
            options: options::changed(),
        }
    }

    fn hero_side(&self, content: f32) -> f32 {
        content.max(hero_h(self.pane_h, content))
    }

    /// Asked before the cursor's own, which the newest-first service then makes first.
    fn ask_beside_cursor(&self, side: f32) {
        let Some(cursor) = self.cursor.as_ref() else {
            return;
        };
        let lines = self.focused_lines();
        let Some(at) = self.line_at(&lines, cursor) else {
            return;
        };
        for beside in [at.checked_sub(1), Some(at + 1)].into_iter().flatten() {
            if let Some(line) = lines.get(beside) {
                thumb(&line.row, side, self.scale, true, false);
            }
        }
    }

    pub(super) fn hero_awaited(&self, content: f32) -> bool {
        self.selected.len() <= 1
            && self.view != GALLERY_VIEW
            && self
                .cursor_line()
                .is_some_and(|line| awaited(&line.row, self.hero_side(content), self.scale, false))
    }

    fn fresh_stats(&self) -> Option<&super::Stats> {
        self.stats
            .as_ref()
            .filter(|stats| stats.key == self.stats_key())
    }

    fn many_card(&self) -> Card {
        let (count, subtitle, [created, modified], tags) = match self.fresh_stats() {
            Some(stats) => {
                let kinds = match (stats.items - stats.folders, stats.folders) {
                    (0, n) => format!("{n} folders"),
                    (n, 0) => format!("{n} documents"),
                    (f, d) => format!("{f} documents, {d} folders"),
                };
                (
                    stats.items,
                    format!("{kinds} - {}", byte_string(stats.bytes)).into(),
                    [span(stats.created), span(stats.modified)],
                    stats.tags.clone(),
                )
            }
            None => (
                self.selected.len(),
                CALCULATING.into(),
                [CALCULATING.into(), CALCULATING.into()],
                Vec::new(),
            ),
        };
        Card {
            hero: match &self.stats {
                Some(stats) => Hero::Stack(stats.head.clone()),
                None => Hero::Blank,
            },
            title: format!("{count} items").into(),
            subtitle,
            rows: vec![("Created", created), ("Modified", modified)],
            show_more: false,
            tags,
            show_tags: true,
            many: true,
            actions: Some(offered(&self.chosen_faces())),
        }
    }

    pub(crate) fn stack_hero(&self, lines: &[Line], content: f32) -> AnyElement {
        stack_hero(lines, content, self.scale, &self.stack)
    }
}

fn hero_h(pane_h: f32, content: f32) -> f32 {
    let visible = pane_h + RESIZE_EDGE - ACTIONS_H;
    (content * HERO_TALL + HERO_EXTRA)
        .min(visible * HERO_CAP + HERO_CAP_EXTRA)
        .floor()
}

fn stack_side(view: usize, pane_h: f32, content: f32) -> f32 {
    match view == GALLERY_VIEW {
        true => HEADER_ICON,
        false => content.min(hero_h(pane_h, content)),
    }
}

fn stack_thumbs(lines: &[Line], content: f32, scale: f32) -> Vec<Option<Arc<RenderImage>>> {
    lines
        .iter()
        .take(3)
        .map(|line| thumb(&line.row, content * STACK_ICON, scale, true, true))
        .collect()
}

fn stack_hero(
    lines: &[Line],
    content: f32,
    scale: f32,
    last: &RefCell<Option<Arc<RenderImage>>>,
) -> AnyElement {
    let Some(first) = lines.first() else {
        return div().size(px(content)).into_any_element();
    };
    let size = content * STACK_ICON;
    let images: Vec<Arc<RenderImage>> = stack_thumbs(lines, content, scale)
        .into_iter()
        .zip(lines)
        .filter_map(|(thumb, line)| {
            thumb.or_else(|| Some(crate::icons::icon(line.row.artwork?, size)?.image))
        })
        .collect();
    if images.is_empty() {
        return square(content)
            .child(artwork(&first.row, content))
            .into_any_element();
    }
    if let Some(image) = stacked(images, (content * scale).round() as u32) {
        *last.borrow_mut() = Some(image);
    }
    square(content)
        .children(
            last.borrow()
                .clone()
                .map(|image| img(image).w(px(content)).h(px(content))),
        )
        .into_any_element()
}

impl Shown {
    pub(super) fn width(&self) -> f32 {
        self.content + 2. * INSET
    }

    fn hero(&self, last: &RefCell<Option<Arc<RenderImage>>>) -> AnyElement {
        let (content, side) = (
            self.content,
            stack_side(self.view, self.pane_h, self.content),
        );
        match &self.card.hero {
            Hero::Row(row) => row_art(row, HEADER_ICON, self.scale, true),
            Hero::Poster(row) => poster_art(
                row,
                content,
                hero_h(self.pane_h, content),
                self.scale,
                Poster::Card,
            ),
            Hero::Stack(lines) => stack_hero(lines, side, self.scale, last),
            Hero::Blank => div().size(px(side)).into_any_element(),
        }
    }

    pub(super) fn element(
        &self,
        palette: &Palette,
        finder: &Entity<FinderDemo>,
        last: &RefCell<Option<Arc<RenderImage>>>,
    ) -> AnyElement {
        let hero = self.hero(last);
        let (card, content) = (&self.card, self.content);
        let body = tokens::system_font_size();
        let small = tokens::small_system_font_size();
        let ink = match self.view == COLUMN_VIEW {
            true => Ink::plain(),
            false => Ink::now(self.key),
        };
        let rows = card.rows.len();
        let title: AnyElement = match &self.renaming {
            Some(line) => rename_field(line, 0, finder.downgrade()).into_any_element(),
            None => card.title.clone().into_any_element(),
        };

        let name = div()
            .line_height(px(NAME_H))
            .text_size(px(NAME_SIZE))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(ink.name)
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .child(title);
        let kind = div()
            .mt(px(GAP_NAME_KIND))
            .line_height(px(LINE_H))
            .text_size(body)
            .text_color(ink.kind)
            .child(card.subtitle.clone());

        let heading = |top: f32| {
            div()
                .mt(px(top))
                .line_height(px(LINE_H))
                .text_size(body)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ink.text)
        };
        let column = div().flex_none().flex().flex_col().px(px(INSET));
        let lower = div().flex_none().flex().flex_col();
        let (column, lower, head) = if self.view == GALLERY_VIEW {
            let column = column.child(div().h(px(GAP_HEADER)).flex_none()).child(
                div()
                    .flex_none()
                    .flex()
                    .items_start()
                    .gap(px(GAP_HEADER_ICON))
                    .child(hero)
                    .child(
                        div()
                            .pt(px(GAP_HEADER_NAME))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(name)
                            .child(kind),
                    ),
            );
            (
                column,
                lower.mt(px(GAP_HEADER_HEAD - GAP_KIND_HEAD)),
                GAP_HEADER + HEADER_ICON + GAP_HEADER_HEAD - GAP_KIND_HEAD,
            )
        } else {
            let hero_h = hero_h(self.pane_h, content);
            let column = column.child(div().h(px(GAP_TOP)).flex_none()).child(
                div()
                    .w(px(content))
                    .h(px(hero_h))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .child(hero),
            );
            (
                column,
                lower.mt(px(GAP_HERO_TEXT)).child(name).child(kind),
                GAP_TOP + hero_h + GAP_HERO_TEXT + NAME_H + GAP_NAME_KIND + LINE_H,
            )
        };
        let info = heading(GAP_KIND_HEAD)
            .flex()
            .items_baseline()
            .justify_between()
            .child("Information")
            .when(card.show_more, |info| {
                info.child(
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
                                    crate::prefs::set_preview_info_expanded(this.preview_expanded);
                                    cx.notify();
                                })
                            }
                        })
                        .child(if self.expanded {
                            "Show Less"
                        } else {
                            "Show More"
                        }),
                )
            });

        let mut block = div().mt(px(GAP_HEAD_ROWS)).flex().flex_col();
        for (index, (label, value)) in card.rows.iter().enumerate() {
            if index > 0 {
                block = block.child(
                    div()
                        .h(px(ROW_GAP))
                        .flex()
                        .flex_col()
                        .justify_center()
                        .child(div().h(px(1.)).bg(ink.rule)),
                );
            }
            block = block.child(
                div()
                    .flex()
                    .justify_between()
                    .gap(px(ROW_TITLE_GAP))
                    .line_height(px(ROW_H))
                    .text_size(small)
                    .whitespace_nowrap()
                    .child(div().flex_none().text_color(ink.title).child(*label))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_right()
                            .text_color(ink.text)
                            .child(value.clone()),
                    ),
            );
        }
        let lower = lower
            .child(info)
            .child(block)
            .when(card.show_tags, |lower| {
                let tags = div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .line_height(px(LINE_H))
                    .text_size(body);
                let tags = match (card.many, card.tags.first()) {
                    (false, Some(tag)) => tags
                        .child(tag_dot(tag.color))
                        .child(div().text_color(palette.label).child(tag.name.clone())),
                    (false, None) => tags.text_color(palette.tertiary).child("Add Tags…"),
                    (true, _) => tags
                        .children(card.tags.iter().map(|tag| tag_pill(tag, palette.label)))
                        .child(
                            div()
                                .ml(px(3.))
                                .text_color(system_color(SystemColor::Link))
                                .child("Add Tag"),
                        ),
                };
                lower
                    .child(heading(GAP_ROWS_TAGS).child(finder_fs::words::say("Tags").into_owned()))
                    .child(
                        div()
                            .mt(px(GAP_TAGS_ADD))
                            .flex_none()
                            .h(px(TAGS_WELL_H))
                            .w_full()
                            .pt(px(TAGS_TEXT_TOP))
                            .child(tags),
                    )
            });

        let actions = div()
            .flex_none()
            .pt(px(ACTION_TOP))
            .flex()
            .justify_center()
            .children(
                action_slots(card.actions.as_deref(), self.option)
                    .into_iter()
                    .enumerate()
                    .map(|(at, (act, symbol, label))| {
                        let slot = div()
                            .id(("action", at))
                            .flex_none()
                            .w(px(ACTION_SLOT_W))
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(
                                // Symbol images differ in height; AppKit's buttons sit them on one baseline.
                                div().h(px(ICON_H)).flex().items_end().children(
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
                                    .text_color(ink.action)
                                    .child(label),
                            );
                        let finder = finder.downgrade();
                        match act {
                            Some(act) => native_tooltip(
                                ("action.tip", at),
                                label,
                                slot.on_click(move |event, _, cx| {
                                    let act = match event.modifiers().alt {
                                        true => act.alternate().unwrap_or(act),
                                        false => act,
                                    };
                                    finder.update(cx, |demo, cx| demo.run_quick(act, cx)).ok();
                                }),
                            )
                            .into_any_element(),
                            None => {
                                let rows = menu_rows(
                                    card.actions
                                        .as_deref()
                                        .and_then(|acts| acts.get(ACTION_BUTTONS..))
                                        .unwrap_or_default(),
                                );
                                let (entries, acts): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
                                native_context_menu(label, slot)
                                    .entries(entries)
                                    .opens_on_click()
                                    .on_select(move |index, _, cx| {
                                        if let Some(Act::Quick(act)) = acts.get(*index) {
                                            finder
                                                .update(cx, |demo, cx| demo.run_quick(*act, cx))
                                                .ok();
                                        }
                                    })
                                    .into_any_element()
                            }
                        }
                    }),
            );
        let column = column.child(lower);
        let tall = head + GAP_KIND_HEAD + LINE_H + GAP_HEAD_ROWS + rows as f32 * (ROW_H + ROW_GAP)
            - ROW_GAP
            + if card.show_tags {
                GAP_ROWS_TAGS + LINE_H + GAP_TAGS_ADD + TAGS_WELL_H
            } else {
                0.
            }
            + BOTTOM_PAD;
        let band = match card.actions {
            Some(_) => ACTIONS_H,
            None => RESIZE_EDGE,
        };
        if self.view == COLUMN_VIEW {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(column)
                .child(div().flex_1())
                .child(actions.pb(px(COLUMN_BOTTOM)))
                .into_any_element();
        }
        div()
            .size_full()
            .relative()
            .child(
                div()
                    .id("ng.finder.preview.card")
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom(px(band - RESIZE_EDGE))
                    .overflow_y_scroll()
                    .child(column.pb(px(BOTTOM_PAD))),
            )
            .when(tall > self.pane_h + RESIZE_EDGE - band, |pane| {
                pane.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(band - RESIZE_EDGE - 0.5))
                        .h(px(1.))
                        .bg(ink.rule),
                )
            })
            .child(
                actions
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(band - RESIZE_EDGE)),
            )
            .into_any_element()
    }
}
