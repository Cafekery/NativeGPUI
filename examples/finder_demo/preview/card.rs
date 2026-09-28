use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui::{
    AnyElement, App, Entity, FontWeight, Hsla, MouseButton, MouseDownEvent, RenderImage,
    SharedString, div, hsla, img, prelude::*, px,
};
use image::{Frame, RgbaImage};
use native_gpui::native_context_menu;
use native_gpui::tokens::{
    self, Appearance, FileType, SymbolColor, SystemColor, sf_symbol_colored, system_color,
};
use objc2_app_kit::{NSEvent, NSEventModifierFlags};

use super::stats::{CALCULATING, span};
use super::{INSET, RESIZE_EDGE};
use crate::app::FinderDemo;
use crate::art::{Poster, artwork, poster_art, row_art, square};
use crate::signal::Signal;
use crate::thumbs::{awaited, thumb};
use finder_fs::meta::Meta;

use crate::data::{Face, Line, Row, byte_string, date_text, small_text_width, stamp, volume_at};
use crate::menus::Act;
use crate::palette::Palette;
use crate::quick_actions::{QuickAct, menu_rows, offered};
use crate::tags::{tag_dot, tag_pill};
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
/// Finder drops to a shorter date style once a value would crowd its row's title by this much.
const ROW_FIT_MARGIN: f32 = 14.;
const GAP_ROWS_TAGS: f32 = 17.25;
const GAP_TAGS_ADD: f32 = 5.9;
const TAGS_WELL_H: f32 = 33.;
const TAGS_TEXT_TOP: f32 = 1.;
/// The pane scrolls, with this much room under the tags, once its content reaches the band.
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
/// Finder's quick actions: a band of the content background along the pane's foot.
pub(super) const ACTIONS_H: f32 = 70.;
const ACTION_TOP: f32 = 15.5;
const ACTION_SLOT_W: f32 = 76.;
const ACTION_ICON_SIZE: f32 = 14.;
const COLUMN_BOTTOM: f32 = 16.;
const STACK_ICON: f32 = 0.62;

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
    tags: Vec<usize>,
    many: bool,
    actions: Vec<QuickAct>,
}

/// A card as its window's state had it, so a pane can draw it without reading that state: a
/// window draws again whenever anything it read while drawing changes.
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

/// Finder's text over the pane's glass is vibrant, which no `NSColor` names: these are the
/// colours it lands on. The tags well is the actions band's content material, opaque.
struct Ink {
    name: Hsla,
    heading: Hsla,
    value: Hsla,
    title: Hsla,
    kind: Hsla,
    rule: Hsla,
    action: Hsla,
    well: Hsla,
}

impl Ink {
    fn now(key: bool) -> Self {
        match tokens::current_appearance() {
            Appearance::Dark => {
                let white = |a| hsla(0., 0., 1., a);
                Self {
                    name: white(0.9),
                    heading: white(0.9),
                    value: white(0.9),
                    title: white(0.36),
                    kind: white(0.18),
                    rule: white(0.138),
                    action: white(0.5),
                    well: hsla(0., 0., 32. / 255., 1.),
                }
            }
            Appearance::Light => {
                let black = |a| hsla(0., 0., 0., a);
                // Light vibrant text darkens the glass by a fixed depth; the glass lightens off key.
                let glass = if key { 237. } else { 242. };
                let under = |depth: f32| hsla(0., 0., (glass - depth) / 255., 1.);
                Self {
                    name: black(0.7),
                    heading: under(178.5),
                    value: under(178.5),
                    title: under(102.),
                    kind: under(51.5),
                    rule: black(0.094),
                    action: black(0.5),
                    well: hsla(0., 0., 1., 1.),
                }
            }
        }
    }

    /// Column view's preview has no glass under it, and Finder's text there is plain.
    fn plain() -> Self {
        let label = system_color(SystemColor::Label);
        let secondary = system_color(SystemColor::SecondaryLabel);
        Self {
            name: label,
            heading: label,
            value: label,
            title: secondary,
            kind: secondary,
            rule: system_color(SystemColor::Separator),
            action: secondary,
            well: hsla(0., 0., 0., 0.),
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

const STACKS_KEPT: usize = 4;

/// Whether a thread is making stacks, and the stacks, oldest first.
static STACKS: Mutex<(bool, Vec<Stacked>)> = Mutex::new((false, Vec::new()));
/// Stacks pushed out of `STACKS`, for the GPU atlas to let go of.
static RETIRED: Mutex<Vec<Arc<RenderImage>>> = Mutex::new(Vec::new());
static STACKED: Signal = Signal::new();

fn made_from(stack: &Stacked, from: &[Arc<RenderImage>], side: u32) -> bool {
    stack.side == side
        && stack.from.len() == from.len()
        && stack.from.iter().zip(from).all(|(a, b)| Arc::ptr_eq(a, b))
}

fn stacked(from: Vec<Arc<RenderImage>>, side: u32) -> Option<Arc<RenderImage>> {
    let mut stacks = STACKS.lock().unwrap();
    if let Some(stack) = stacks.1.iter().find(|stack| made_from(stack, &from, side)) {
        return stack.image.clone().flatten();
    }
    stacks.1.push(Stacked {
        from,
        side,
        image: None,
    });
    if stacks.1.len() > STACKS_KEPT {
        let gone = stacks.1.remove(0);
        RETIRED.lock().unwrap().extend(gone.image.flatten());
    }
    if !std::mem::replace(&mut stacks.0, true) {
        std::thread::spawn(make_stacks);
    }
    None
}

/// The newest stack asked for first: a pane being resized asks for one per frame.
fn make_stacks() {
    loop {
        let wanted = {
            let mut stacks = STACKS.lock().unwrap();
            let wanted = stacks
                .1
                .iter()
                .rev()
                .find(|stack| stack.image.is_none())
                .map(|stack| (stack.from.clone(), stack.side));
            stacks.0 = wanted.is_some();
            wanted
        };
        let Some((from, side)) = wanted else {
            return;
        };
        let image = stack_image(&from, side);
        let mut stacks = STACKS.lock().unwrap();
        if let Some(stack) = stacks
            .1
            .iter_mut()
            .find(|stack| made_from(stack, &from, side))
        {
            stack.image = Some(image);
        }
        drop(stacks);
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

/// Everything a card draws from: a pane draws again only when this changes.
#[derive(PartialEq)]
pub(super) struct CardKey {
    view: usize,
    stats: ((u64, usize), Option<(u64, usize)>),
    line: Option<(SharedString, usize)>,
    tags: Option<Vec<usize>>,
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
    content: f32,
) -> (Vec<(&'static str, SharedString)>, bool) {
    let none = Meta::default();
    let meta = match row.face {
        Face::Volume => &none,
        _ => meta.unwrap_or(&none),
    };
    let size = |(w, h): (u64, u64)| SharedString::from(format!("{w}\u{d7}{h}"));
    let mut all: Vec<(&'static str, SharedString, bool)> = Vec::new();
    if let Some(volume) = volume_at(&row.path) {
        all.push(("Capacity", byte_string(volume.capacity).into(), false));
        all.push(("Available", byte_string(volume.available).into(), false));
    }
    let date = |label, at| stamp(at, content - label_width(label) - ROW_FIT_MARGIN);
    all.push(("Created", date("Created", row.created_at), false));
    all.push(("Modified", date("Modified", row.modified_at), false));
    let mut add = |label, value: Option<SharedString>, under_more| {
        all.extend(value.map(|value| (label, value, under_more)));
    };
    add(
        "Last opened",
        meta.last_used.map(|at| date("Last opened", at)),
        false,
    );
    let media = matches!(row.face, Face::Image | Face::Movie | Face::Audio);
    add(
        "Content created",
        meta.content_created.filter(|_| media).map(date_text),
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

fn clock(seconds: f64) -> SharedString {
    let seconds = seconds.max(0.).round() as u64;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    match hours {
        0 => format!("{minutes}:{seconds:02}").into(),
        _ => format!("{hours}:{minutes:02}:{seconds:02}").into(),
    }
}

/// Finder shows the first two quick actions as buttons and the rest under More….
const ACTION_BUTTONS: usize = 2;

/// Each button's act, and what it shows: its Option alternate while Option is held.
fn action_slots(
    acts: &[QuickAct],
    option: bool,
) -> Vec<(Option<QuickAct>, &'static str, &'static str)> {
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
            let (rows, show_more) = info_rows(row, meta.as_deref(), self.preview_expanded, content);
            Card {
                hero: match self.view == GALLERY_VIEW {
                    true => Hero::Row(row.clone()),
                    false => Hero::Poster(row.clone()),
                },
                title: row.display.clone(),
                subtitle: kind_line(row, self.weighed()),
                rows,
                show_more,
                tags: self.tags.get(&line.key).cloned().unwrap_or_default(),
                many: false,
                actions: offered(&[row.face]),
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

    /// Asked each frame the pane is open, for the hero's pictures just as drawing it asks.
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
        let folder = |row: &Row| {
            matches!(
                row.artwork,
                Some(FileType::Folder | FileType::FullFolder | FileType::FullPath(_))
            )
        };
        CardKey {
            view: self.view,
            stats: (self.stats_key(), self.stats.as_ref().map(|stats| stats.key)),
            tags: line
                .as_ref()
                .and_then(|line| self.tags.get(&line.key).cloned()),
            meta: row
                .and_then(|row| crate::meta::meta(&row.path))
                .map_or(0, |meta| Arc::as_ptr(&meta) as usize),
            custom: row.is_some_and(|row| folder(row) && crate::custom::custom_icon(&row.path)),
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
        }
    }

    fn hero_side(&self, content: f32) -> f32 {
        content.max(hero_h(self.pane_h, content))
    }

    /// So an arrow key lands on a picture already made. Asked before the cursor's own, which the
    /// service then makes first: it serves its newest request first.
    pub(crate) fn ask_beside_cursor(&self, side: f32) {
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

    /// Whether the cursor's hero picture is still being made for the first time.
    pub(super) fn hero_awaited(&self, content: f32) -> bool {
        self.selected.len() <= 1
            && self.view != GALLERY_VIEW
            && self
                .cursor_line()
                .is_some_and(|line| awaited(&line.row, self.hero_side(content), self.scale, false))
    }

    fn weighed(&self) -> Option<u64> {
        self.stats
            .as_ref()
            .filter(|stats| stats.key == self.stats_key())
            .map(|stats| stats.bytes)
            .filter(|bytes| *bytes > 0)
    }

    fn many_card(&self) -> Card {
        let hero = match self.stats.as_ref() {
            Some(stats) => Hero::Stack(stats.head.clone()),
            None => Hero::Blank,
        };
        let actions = offered(&self.chosen_faces());
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
                actions: actions.clone(),
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
    lines[..lines.len().min(3)]
        .iter()
        .map(|line| thumb(&line.row, content * STACK_ICON, scale, true, true))
        .collect()
}

/// `last` is the stack this holder showed last, which stands in until this one is made.
fn stack_hero(
    lines: &[Line],
    content: f32,
    scale: f32,
    last: &RefCell<Option<Arc<RenderImage>>>,
) -> AnyElement {
    let Some(first) = lines.first() else {
        return div().w(px(content)).h(px(content)).into_any_element();
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
    let image = match stacked(images, (content * scale).round() as u32) {
        Some(image) => Some(last.borrow_mut().insert(image).clone()),
        None => last.borrow().clone(),
    };
    square(content)
        .children(image.map(|image| img(image).w(px(content)).h(px(content))))
        .into_any_element()
}

impl Shown {
    pub(super) fn width(&self) -> f32 {
        self.content + 2. * INSET
    }

    pub(super) fn element(
        &self,
        palette: &Palette,
        finder: &Entity<FinderDemo>,
        last: &RefCell<Option<Arc<RenderImage>>>,
    ) -> AnyElement {
        let (card, content) = (&self.card, self.content);
        let hero = match &card.hero {
            Hero::Row(row) => row_art(row, HEADER_ICON, self.scale, true),
            Hero::Poster(row) => poster_art(
                row,
                content,
                hero_h(self.pane_h, content),
                self.scale,
                Poster::Card,
            ),
            Hero::Stack(lines) => {
                let side = stack_side(self.view, self.pane_h, content);
                stack_hero(lines, side, self.scale, last)
            }
            Hero::Blank => {
                let side = stack_side(self.view, self.pane_h, content);
                div().w(px(side)).h(px(side)).into_any_element()
            }
        };
        self.card_element(hero, palette, finder)
    }

    fn card_element(
        &self,
        hero: AnyElement,
        palette: &Palette,
        finder: &Entity<FinderDemo>,
    ) -> AnyElement {
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

        let mut content_column = div().flex_none().flex().flex_col().px(px(INSET));
        let mut lower = div().flex_none().flex().flex_col();
        if self.view == GALLERY_VIEW {
            content_column = content_column
                .child(div().h(px(GAP_HEADER)).flex_none())
                .child(
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
            lower = lower.mt(px(GAP_HEADER_HEAD - GAP_KIND_HEAD));
        } else {
            content_column = content_column
                .child(div().h(px(GAP_TOP)).flex_none())
                .child(
                    div()
                        .w(px(content))
                        .h(px(hero_h(self.pane_h, content)))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .overflow_hidden()
                        .child(hero),
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
                .text_color(ink.heading)
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
                            .child(if self.expanded {
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
                            .text_color(ink.value)
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
                .text_color(ink.heading)
                .child(finder_fs::words::say("Tags").into_owned()),
        );

        let tags = div()
            .flex()
            .items_center()
            .gap(px(5.))
            .line_height(px(LINE_H))
            .text_size(body);
        let tags = match (card.many, card.tags.first()) {
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
        };
        lower = lower.child(
            div()
                .mt(px(GAP_TAGS_ADD))
                .flex_none()
                .h(px(TAGS_WELL_H))
                .w_full()
                .pt(px(TAGS_TEXT_TOP))
                .bg(ink.well)
                .child(tags),
        );

        let actions = div()
            .flex_none()
            .pt(px(ACTION_TOP))
            .flex()
            .justify_center()
            .children(
                action_slots(&card.actions, self.option)
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
                            Some(act) => slot
                                .on_click(move |event, _, cx| {
                                    let act = match event.modifiers().alt {
                                        true => act.alternate().unwrap_or(act),
                                        false => act,
                                    };
                                    finder.update(cx, |demo, cx| demo.run_quick(act, cx)).ok();
                                })
                                .into_any_element(),
                            None => {
                                let rows = menu_rows(
                                    &card.actions[card.actions.len().min(ACTION_BUTTONS)..],
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
        let head = match self.view == GALLERY_VIEW {
            true => GAP_HEADER + HEADER_ICON + GAP_HEADER_HEAD - GAP_KIND_HEAD,
            false => {
                GAP_TOP
                    + hero_h(self.pane_h, content)
                    + GAP_HERO_TEXT
                    + NAME_H
                    + GAP_NAME_KIND
                    + LINE_H
            }
        };
        let content = content_column.child(lower);
        let tall = head + GAP_KIND_HEAD + LINE_H + GAP_HEAD_ROWS + rows as f32 * (ROW_H + ROW_GAP)
            - ROW_GAP
            + GAP_ROWS_TAGS
            + LINE_H
            + GAP_TAGS_ADD
            + TAGS_WELL_H
            + BOTTOM_PAD;
        if self.view == COLUMN_VIEW {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(content)
                .child(div().flex_1())
                .child(actions.pb(px(COLUMN_BOTTOM)))
                .into_any_element();
        }
        // The pane scrolls above the actions band, which stays at its foot.
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
                    .bottom(px(ACTIONS_H - RESIZE_EDGE))
                    .overflow_y_scroll()
                    .child(content.pb(px(BOTTOM_PAD))),
            )
            // Finder rules the band off only while the content runs down to it.
            .when(tall > self.pane_h + RESIZE_EDGE - ACTIONS_H, |pane| {
                pane.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(ACTIONS_H - RESIZE_EDGE - 0.5))
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
                    .h(px(ACTIONS_H - RESIZE_EDGE)),
            )
            .into_any_element()
    }
}
