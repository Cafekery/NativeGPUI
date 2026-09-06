//! Drawing helpers every view shares, and the tag colours they draw with.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, ClickEvent, FontWeight, Hsla, ObjectFit, Pixels, RenderImage, SharedString,
    WeakEntity, Window, div, img, prelude::*, px,
};

use native_gpui::thumbnails;
use native_gpui::tokens::{SymbolColor, SystemColor, file_icon, sf_symbol_colored};
pub(crate) use native_gpui::{focus_gpui, native_text_field};

use crate::data::{Face, Line, Row};
use crate::{FinderDemo, ns_window};

/// The demo's tags, in Finder's order; the second field is Finder's own colour index for each
/// (1 grey … 7 orange, matching `finder_fs::tags`).
pub(crate) const TAGS: &[(&str, u8)] = &[
    ("Red", 6),
    ("Orange", 7),
    ("Yellow", 5),
    ("Green", 2),
    ("Blue", 4),
    ("Purple", 3),
    ("Gray", 1),
];

/// Finder's own sRGB for a tag colour index (1 grey … 7 orange), sampled from Finder's own tag
/// swatches (Get Info / the Tags context menu) rather than guessed from `NSColor.systemRed` etc.,
/// which run more saturated and don't match. `None` for 0 (no colour) or anything out of range.
pub(crate) fn finder_color_rgb(color: u8) -> Option<(u8, u8, u8)> {
    Some(match color {
        1 => (165, 165, 169), // Gray
        2 => (93, 210, 122),  // Green
        3 => (213, 89, 230),  // Purple
        4 => (51, 160, 255),  // Blue
        5 => (255, 214, 51),  // Yellow
        6 => (255, 96, 99),   // Red
        7 => (255, 164, 83),  // Orange
        _ => return None,
    })
}

/// [`finder_color_rgb`] as an [`Hsla`] gpui can paint with, white if the index is out of range.
pub(crate) fn finder_color(color: u8) -> Hsla {
    let (r, g, b) = finder_color_rgb(color).unwrap_or((255, 255, 255));
    gpui::rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32).into()
}

/// One of the demo's tags as Finder stores it.
pub(crate) fn finder_tag(index: usize) -> finder_fs::tags::Tag {
    finder_fs::tags::Tag {
        name: TAGS[index].0.to_string(),
        color: TAGS[index].1,
    }
}

/// The kinds Quick Look draws a real page or picture for; every other type keeps its icon.
const RENDERABLE: &[&str] = &[
    "pdf", "png", "jpg", "jpeg", "heic", "heif", "gif", "tiff", "tif", "bmp", "webp", "mov", "mp4",
    "m4v", "key", "pages", "numbers",
];

/// A file, its pixel side, and whether Quick Look drew it as an icon: what one thumbnail is.
type Thumb = (SharedString, u32, bool);

thread_local! {
    /// What the generator has delivered, so a row asks once and draws from here every frame after.
    static THUMBS: RefCell<HashMap<Thumb, Arc<RenderImage>>> = RefCell::new(HashMap::new());
    /// Every thumbnail asked for, so a row on screen for a hundred frames asks once.
    static ASKED: RefCell<HashSet<Thumb>> = RefCell::new(HashSet::new());
    /// The rows drawn since the last sweep: the ones whose generation is still worth running.
    static DRAWN: RefCell<HashSet<SharedString>> = RefCell::new(HashSet::new());
    /// Frames drawn, and when and after how many frames the last sweep ran.
    static FRAMES: Cell<u64> = const { Cell::new(0) };
    static SWEPT: Cell<(Option<Instant>, u64)> = const { Cell::new((None, 0)) };
}

/// Thumbnails land on a background queue; the window drains them on its next tick.
static DELIVERED: AtomicUsize = AtomicUsize::new(0);
static LANDED: Mutex<Vec<(Thumb, Option<Arc<RenderImage>>)>> = Mutex::new(Vec::new());

const SWEEP: Duration = Duration::from_millis(250);

pub(crate) fn thumbnails_landed() -> usize {
    DELIVERED.load(Ordering::Relaxed)
}

/// Move what the generator delivered into the map the rows draw from. Main thread only.
pub(crate) fn collect_thumbnails() {
    let landed = std::mem::take(&mut *LANDED.lock().unwrap());
    for (key, image) in landed {
        match image {
            Some(image) => {
                THUMBS.with(|thumbs| thumbs.borrow_mut().insert(key, image));
            }
            // A cancelled generation asks again the next time the row is on screen.
            None => ASKED.with(|asked| {
                asked.borrow_mut().remove(&key);
            }),
        }
    }
}

/// A window finished a frame: what it drew is what the next sweep keeps.
pub(crate) fn frame_drawn() {
    FRAMES.with(|frames| frames.set(frames.get() + 1));
}

/// Drop the generations queued for rows that have scrolled away. A window that has not drawn
/// since the last sweep says nothing about what is on screen, so it does not sweep.
pub(crate) fn sweep_thumbnails() {
    let now = Instant::now();
    let frames = FRAMES.with(|frames| frames.get());
    let (swept_at, swept_after) = SWEPT.with(|swept| swept.get());
    if frames == swept_after || swept_at.is_some_and(|at| now.duration_since(at) < SWEEP) {
        return;
    }
    SWEPT.with(|swept| swept.set((Some(now), frames)));
    let drawn: Vec<PathBuf> = DRAWN.with(|drawn| {
        drawn
            .borrow_mut()
            .drain()
            .map(|path| PathBuf::from(path.as_ref()))
            .collect()
    });
    thumbnails::cancel_thumbnails_outside(&drawn);
}

/// Whether Quick Look has a rendered page or picture for this row, rather than a type icon.
fn renderable(row: &Row) -> bool {
    if row.opens.is_some() {
        return false;
    }
    if matches!(
        row.face,
        Face::Image | Face::Movie | Face::Pdf | Face::Slides | Face::Pages | Face::Sheet
    ) {
        return true;
    }
    row.name.rsplit_once('.').is_some_and(|(_, extension)| {
        RENDERABLE
            .iter()
            .any(|kind| extension.eq_ignore_ascii_case(kind))
    })
}

/// Finder's row icon: the file's own rendered thumbnail at this size where Quick Look draws one,
/// its type's icon until that lands. Only a row on screen asks for one.
pub(crate) fn row_art(row: &Row, size: f32, scale: f32, visible: bool) -> AnyElement {
    match thumb(row, size, scale, visible, true) {
        Some(image) => picture(image, size),
        None => artwork(row, size),
    }
}

/// Finder's large preview art: the picture at its own aspect, keylined and rounded on that rect
/// rather than on the box it is centred in, and without an icon's inset, radius or shadow.
pub(crate) fn poster_art(row: &Row, size: f32, scale: f32, kind: Poster) -> AnyElement {
    match thumb(row, size, scale, true, false) {
        Some(image) => poster(image, size, scale, kind),
        None => artwork(row, size),
    }
}

/// The thumbnail a row draws from, once one has landed; a row on screen asks for one.
fn thumb(row: &Row, size: f32, scale: f32, visible: bool, icon: bool) -> Option<Arc<RenderImage>> {
    if !renderable(row) {
        return None;
    }
    let key = (
        row.path.clone(),
        (size * scale).round().max(1.) as u32,
        icon,
    );
    if let Some(image) = THUMBS.with(|thumbs| thumbs.borrow().get(&key).cloned()) {
        return Some(image);
    }
    // Everything below costs a `stat` or a lock, so a row off screen or already waiting stops here.
    if !visible || ASKED.with(|asked| asked.borrow().contains(&key)) {
        return None;
    }
    DRAWN.with(|drawn| drawn.borrow_mut().insert(row.path.clone()));
    let path = Path::new(row.path.as_ref());
    if let Some(image) = thumbnails::cached_thumbnail(path, px(size), scale, icon) {
        THUMBS.with(|thumbs| thumbs.borrow_mut().insert(key, image.clone()));
        return Some(image);
    }
    ask(path, key, size, scale, icon);
    None
}

/// Finder's Quick Look icon: the whole picture fit inside the box, centred, with the keyline,
/// corner radius and shadow Quick Look's icon mode baked into the raster.
fn picture(image: Arc<RenderImage>, size: f32) -> AnyElement {
    div()
        .w(px(size))
        .h(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            img(image)
                .max_w(px(size))
                .max_h(px(size))
                .object_fit(ObjectFit::Contain),
        )
        .into_any_element()
}

/// Which preview a poster is: Finder strokes a gallery hero with a light keyline and a wider
/// radius, a preview card with a dark one.
#[derive(Clone, Copy)]
pub(crate) enum Poster {
    Hero,
    Card,
}

fn poster(image: Arc<RenderImage>, size: f32, scale: f32, kind: Poster) -> AnyElement {
    let fit = image.size(0);
    let (radius, keyline) = match kind {
        Poster::Hero => (
            6.5,
            Hsla {
                h: 0.,
                s: 0.,
                l: 1.,
                a: 0.1,
            },
        ),
        Poster::Card => (
            2.5,
            Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.24,
            },
        ),
    };
    div()
        .w(px(size))
        .h(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            img(image)
                .w(px((u32::from(fit.width) as f32 / scale).min(size)))
                .h(px((u32::from(fit.height) as f32 / scale).min(size)))
                .rounded(px(radius))
                .border(px(0.5))
                .border_color(keyline),
        )
        .into_any_element()
}

/// One request per file and size: the module coalesces the work already in flight, but a waiter
/// a frame would pile up behind it.
fn ask(path: &Path, key: Thumb, size: f32, scale: f32, icon: bool) {
    if !ASKED.with(|asked| asked.borrow_mut().insert(key.clone())) {
        return;
    }
    thumbnails::thumbnail(path, px(size), scale, icon, move |image| {
        LANDED.lock().unwrap().push((key, image));
        DELIVERED.fetch_add(1, Ordering::Relaxed);
    });
}

/// Where an icon landed on screen, for the Quick Look panel to zoom out of and back into.
pub(crate) type Anchor = std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>;

/// `art` as drawn, recording where it lands when its row is the one Quick Look would open.
pub(crate) fn art_at(art: AnyElement, size: f32, anchor: Option<Anchor>) -> AnyElement {
    let Some(anchor) = anchor else {
        return art;
    };
    div()
        .relative()
        .flex_none()
        .w(px(size))
        .h(px(size))
        .child(art)
        .child(
            gpui::canvas(
                move |bounds, _, _| anchor.set(Some(bounds)),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .into_any_element()
}

pub(crate) fn artwork(row: &Row, size: f32) -> AnyElement {
    let art = row
        .artwork
        .and_then(|kind| file_icon(kind, px(size)))
        .map(|symbol| symbol.img().into_any_element())
        .unwrap_or_else(|| glyph(row.face, size));
    div()
        .w(px(size))
        .h(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(art)
        .into_any_element()
}

pub(crate) fn glyph(face: Face, size: f32) -> AnyElement {
    let symbol = sf_symbol_colored(face.symbol(), px(size), FontWeight::NORMAL, &face.tint());
    let mut slot = div()
        .w(px(size + 4.))
        .h(px(size))
        .flex()
        .items_center()
        .justify_center()
        .flex_none();
    if let Some(symbol) = symbol {
        slot = slot.child(symbol.img());
    }
    slot.into_any_element()
}

pub(crate) fn small_glyph(
    name: &str,
    size: f32,
    weight: FontWeight,
    color: SystemColor,
) -> Option<AnyElement> {
    sf_symbol_colored(name, px(size), weight, &SymbolColor::Hierarchical(color))
        .map(|symbol| symbol.img().into_any_element())
}

pub(crate) fn tag_pill(index: usize, text: Hsla) -> AnyElement {
    let (name, color) = TAGS[index];
    let mut wash = finder_color(color);
    wash.a = 0.55;
    div()
        .px(px(6.))
        .rounded(px(4.))
        .bg(wash)
        .text_color(text)
        .child(name)
        .into_any_element()
}

pub(crate) fn tag_dot(index: usize) -> AnyElement {
    div()
        .w(px(8.))
        .h(px(8.))
        .rounded_full()
        .flex_none()
        .bg(finder_color(TAGS[index].1))
        .into_any_element()
}

/// `(extend, toggle, twice)`: Shift, Cmd and a double click.
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

/// Hand key back to the main window after a field in the pane's panel took it.
pub(crate) fn key_to_parent(window: &Window) {
    if let Some(parent) = ns_window(window).and_then(|panel| panel.parentWindow()) {
        parent.makeKeyWindow();
    }
}

/// A scroll container that ignores the other axis; nested with a list, each axis then has one owner.
pub(crate) fn axis_locked<E: Styled>(mut element: E) -> E {
    element.style().restrict_scroll_to_axis = Some(true);
    element
}
