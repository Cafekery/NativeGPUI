//! The thumbnails Quick Look draws for a row: asked for once, drained on the window's tick and
//! swept when the row scrolls away.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{RenderImage, SharedString, px};
use native_gpui::thumbnails;

use crate::data::{Face, Row};

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
        // What came back empty stays asked: Quick Look draws nothing for this file, and asking
        // again is a stat and a lock for every row of every frame. The sweep re-arms whatever
        // scrolled away, so a row that comes back asks once more.
        if let Some(image) = image {
            THUMBS.with(|thumbs| thumbs.borrow_mut().insert(key, image));
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
    let drawn: HashSet<SharedString> = DRAWN.with(|drawn| drawn.borrow_mut().drain().collect());
    ASKED.with(|asked| {
        asked
            .borrow_mut()
            .retain(|(path, _, _)| drawn.contains(path))
    });
    let paths: Vec<PathBuf> = drawn
        .iter()
        .map(|path| PathBuf::from(path.as_ref()))
        .collect();
    thumbnails::cancel_thumbnails_outside(&paths);
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

/// The thumbnail a row draws from, once one has landed; a row on screen asks for one.
pub(crate) fn thumb(
    row: &Row,
    size: f32,
    scale: f32,
    visible: bool,
    icon: bool,
) -> Option<Arc<RenderImage>> {
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
    if !visible {
        return None;
    }
    DRAWN.with(|drawn| drawn.borrow_mut().insert(row.path.clone()));
    // Everything below costs a `stat` or a lock, so a row already waiting stops here.
    if ASKED.with(|asked| asked.borrow().contains(&key)) {
        return None;
    }
    let path = Path::new(row.path.as_ref());
    if let Some(image) = thumbnails::cached_thumbnail(path, px(size), scale, icon) {
        THUMBS.with(|thumbs| thumbs.borrow_mut().insert(key, image.clone()));
        return Some(image);
    }
    ask(path, key, size, scale, icon);
    None
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
