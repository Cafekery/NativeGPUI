//! The thumbnails Quick Look draws for a row: asked for once, swept when the row scrolls away.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{RenderImage, SharedString, px};
use native_gpui::thumbnails;

use crate::data::{Face, Row};

/// The kinds Quick Look draws a page or picture for; every other type keeps its icon.
const RENDERABLE: &[&str] = &[
    "pdf", "png", "jpg", "jpeg", "heic", "heif", "gif", "tiff", "tif", "bmp", "webp", "mov", "mp4",
    "m4v", "key", "pages", "numbers",
];

/// A file, its pixel side, and whether Quick Look drew it as an icon: what one thumbnail is.
type Thumb = (SharedString, u32, bool);

/// A file and whether Quick Look drew it as an icon, against the largest raster it has landed.
type Landed = HashMap<(SharedString, bool), (u32, Arc<RenderImage>)>;

thread_local! {
    static THUMBS: RefCell<HashMap<Thumb, Arc<RenderImage>>> = RefCell::new(HashMap::new());
    /// Every thumbnail asked for, so a row on screen for a hundred frames asks once.
    static ASKED: RefCell<HashSet<Thumb>> = RefCell::new(HashSet::new());
    /// The largest raster landed per file, drawn scaled while a new size is generated.
    static LARGEST: RefCell<Landed> = RefCell::new(HashMap::new());
    static DRAWN: RefCell<HashSet<SharedString>> = RefCell::new(HashSet::new());
    /// What came back empty, and when it may be asked for again.
    static RETRY: RefCell<HashMap<Thumb, (u32, Instant)>> = RefCell::new(HashMap::new());
    static FRAMES: Cell<u64> = const { Cell::new(0) };
    static SWEPT: Cell<(Option<Instant>, u64)> = const { Cell::new((None, 0)) };
}

/// Thumbnails land on a background queue; the window drains them on its next tick.
static DELIVERED: AtomicUsize = AtomicUsize::new(0);
static LANDED: Mutex<Vec<(Thumb, Option<Arc<RenderImage>>)>> = Mutex::new(Vec::new());

const SWEEP: Duration = Duration::from_millis(250);
const RETRY_AFTER: Duration = Duration::from_millis(250);
const RETRIES: u32 = 5;

pub(crate) fn thumbnails_landed() -> usize {
    DELIVERED.load(Ordering::Relaxed)
}

/// Move what the generator delivered into the map the rows draw from. Main thread only.
pub(crate) fn collect_thumbnails() {
    let landed = std::mem::take(&mut *LANDED.lock().unwrap());
    for (key, image) in landed {
        let Some(image) = image else {
            retry(&key);
            continue;
        };
        keep(&key, &image);
        THUMBS.with(|thumbs| thumbs.borrow_mut().insert(key, image));
    }
}

/// An empty delivery is a cancelled generation as often as a file with no picture: ask again,
/// backing off, rather than leaving a row on its type icon for as long as it is on screen.
fn retry(key: &Thumb) {
    let tries = RETRY.with(|retry| {
        let mut retry = retry.borrow_mut();
        let (tries, again) = retry.entry(key.clone()).or_insert((0, Instant::now()));
        *tries = (*tries + 1).min(RETRIES + 1);
        *again = Instant::now() + RETRY_AFTER * (1 << (*tries - 1));
        *tries
    });
    if tries <= RETRIES {
        ASKED.with(|asked| asked.borrow_mut().remove(key));
    }
}

/// Record a raster as the one a later size draws from until its own lands.
fn keep(key: &Thumb, image: &Arc<RenderImage>) {
    LARGEST.with(|largest| {
        let mut largest = largest.borrow_mut();
        let slot = largest.entry((key.0.clone(), key.2));
        slot.and_modify(|held| {
            if held.0 < key.1 {
                *held = (key.1, image.clone());
            }
        })
        .or_insert_with(|| (key.1, image.clone()));
    });
}

fn largest(path: &SharedString, icon: bool) -> Option<Arc<RenderImage>> {
    LARGEST.with(|largest| {
        largest
            .borrow()
            .get(&(path.clone(), icon))
            .map(|(_, image)| image.clone())
    })
}

pub(crate) fn frame_drawn() {
    FRAMES.with(|frames| frames.set(frames.get() + 1));
}

/// Drop the generations queued for rows that have scrolled away, once a frame has been drawn.
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
    if ASKED.with(|asked| asked.borrow().contains(&key)) {
        return largest(&row.path, icon);
    }
    let path = Path::new(row.path.as_ref());
    if let Some(image) = thumbnails::cached_thumbnail(path, px(size), scale, icon) {
        keep(&key, &image);
        THUMBS.with(|thumbs| thumbs.borrow_mut().insert(key, image.clone()));
        return Some(image);
    }
    ask(path, key, size, scale, icon);
    largest(&row.path, icon)
}

/// One request per file and size, so a frame does not pile waiters behind work in flight.
fn ask(path: &Path, key: Thumb, size: f32, scale: f32, icon: bool) {
    if RETRY.with(|retry| {
        retry
            .borrow()
            .get(&key)
            .is_some_and(|(_, again)| Instant::now() < *again)
    }) {
        return;
    }
    if !ASKED.with(|asked| asked.borrow_mut().insert(key.clone())) {
        return;
    }
    thumbnails::thumbnail(path, px(size), scale, icon, move |image| {
        LANDED.lock().unwrap().push((key, image));
        DELIVERED.fetch_add(1, Ordering::Relaxed);
    });
}
