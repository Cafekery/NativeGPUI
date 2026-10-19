use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{App, RenderImage, SharedString, px};
use native_gpui::thumbnails;
use parking_lot::Mutex;

use crate::data::{Face, Row};
use crate::signal::Signal;

type Thumb = (SharedString, u32, bool, u64);

type Ask = (PathBuf, Option<SystemTime>, Thumb, f32, f32);

type Landed = HashMap<(SharedString, bool), (u32, Arc<RenderImage>)>;

thread_local! {
    static THUMBS: RefCell<HashMap<Thumb, Arc<RenderImage>>> = RefCell::new(HashMap::new());
    static ASKED: RefCell<HashSet<Thumb>> = RefCell::new(HashSet::new());
    static LARGEST: RefCell<Landed> = RefCell::new(HashMap::new());
    static DRAWN: RefCell<HashSet<SharedString>> = RefCell::new(HashSet::new());
    static RETRY: RefCell<HashMap<Thumb, (u32, Instant)>> = RefCell::new(HashMap::new());
    static FRAMES: Cell<u64> = const { Cell::new(0) };
    static SWEPT: Cell<(Option<Instant>, u64)> = const { Cell::new((None, 0)) };
    static HELD: RefCell<Vec<Ask>> = const { RefCell::new(Vec::new()) };
}

static INFLIGHT: AtomicUsize = AtomicUsize::new(0);
static LANDED: Mutex<Vec<(Thumb, Option<Arc<RenderImage>>)>> = Mutex::new(Vec::new());
static LANDING: Signal = Signal::new();

const SWEEP: Duration = Duration::from_millis(250);
const RETRY_AFTER: Duration = Duration::from_millis(250);
const RETRIES: u32 = 5;
const KEPT: usize = 512;
const KEPT_BYTES: usize = 256 << 20;
/// The thumbnail service's own limit; past it, it queues requests and serves the newest first.
const LANES: usize = 8;

pub(crate) fn follow_thumbnails(cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut seen = 0;
        loop {
            seen = LANDING.after(seen).await;
            cx.update(|cx| {
                collect_thumbnails();
                crate::read::notify_windows(cx);
            });
        }
    })
    .detach();
}

pub(crate) fn collect_thumbnails() {
    // Bottom row first: the service serves its newest request first, so a frame fills top down.
    for ask in HELD.take().into_iter().rev() {
        send(ask);
    }
    let landed = std::mem::take(&mut *LANDED.lock());
    for (key, image) in landed {
        let Some(image) = image else {
            retry(&key);
            continue;
        };
        keep(key, image);
    }
    sweep_thumbnails();
}

/// An empty delivery is often a cancelled generation, so ask again, backing off.
fn retry(key: &Thumb) {
    let tries = RETRY.with_borrow_mut(|retry| {
        let (tries, again) = retry.entry(key.clone()).or_insert((0, Instant::now()));
        *tries = (*tries + 1).min(RETRIES + 1);
        *again = Instant::now() + RETRY_AFTER * (1 << (*tries - 1));
        *tries
    });
    if tries <= RETRIES {
        ASKED.with_borrow_mut(|asked| asked.remove(key));
    }
}

fn keep(key: Thumb, image: Arc<RenderImage>) {
    LARGEST.with_borrow_mut(|largest| {
        let held = largest
            .entry((key.0.clone(), key.2))
            .or_insert_with(|| (key.1, image.clone()));
        if held.0 < key.1 {
            *held = (key.1, image.clone());
        }
    });
    THUMBS.with_borrow_mut(|thumbs| thumbs.insert(key, image));
}

fn largest(path: &SharedString, icon: bool) -> Option<Arc<RenderImage>> {
    LARGEST.with_borrow(|largest| {
        largest
            .get(&(path.clone(), icon))
            .map(|(_, image)| image.clone())
    })
}

pub(crate) fn frame_drawn() {
    FRAMES.set(FRAMES.get() + 1);
}

fn sweep_thumbnails() {
    let now = Instant::now();
    let frames = FRAMES.get();
    let (swept_at, swept_after) = SWEPT.get();
    if frames == swept_after || swept_at.is_some_and(|at| now.duration_since(at) < SWEEP) {
        return;
    }
    SWEPT.set((Some(now), frames));
    let drawn = DRAWN.take();
    ASKED.with_borrow_mut(|asked| asked.retain(|(path, _, _, _)| drawn.contains(path)));
    // Off-screen thumbnails come back from the library's own bounded cache when scrolled to.
    let (count, bytes) = THUMBS.with_borrow(|thumbs| {
        let bytes = thumbs
            .values()
            .map(|image| image.as_bytes(0).map_or(0, <[u8]>::len))
            .sum::<usize>();
        (thumbs.len(), bytes)
    });
    if count > KEPT || bytes > KEPT_BYTES {
        THUMBS.with_borrow_mut(|thumbs| thumbs.retain(|(path, _, _, _), _| drawn.contains(path)));
        LARGEST.with_borrow_mut(|largest| largest.retain(|(path, _), _| drawn.contains(path)));
        RETRY.with_borrow_mut(|retry| retry.retain(|(path, _, _, _), _| drawn.contains(path)));
    }
    crate::remote::prune_fetches(&drawn);
    let paths: Vec<PathBuf> = drawn.iter().filter_map(|path| local_path(path)).collect();
    thumbnails::cancel_thumbnails_outside(&paths);
}

pub(crate) fn wake() {
    LANDING.ring();
}

fn renderable(row: &Row) -> bool {
    row.opens.is_none() && row.face != Face::Program
}

fn local_path(id: &str) -> Option<PathBuf> {
    if crate::remote::is_remote(id) {
        crate::remote::cached(id)
    } else {
        Some(PathBuf::from(id))
    }
}

fn key_of(row: &Row, size: f32, scale: f32, icon: bool) -> Thumb {
    (
        row.path.clone(),
        (size * scale).round().max(1.) as u32,
        icon,
        row.modified_at.to_bits(),
    )
}

pub(crate) fn awaited(row: &Row, size: f32, scale: f32, icon: bool) -> bool {
    let key = key_of(row, size, scale, icon);
    renderable(row)
        && ASKED.with_borrow(|asked| asked.contains(&key))
        && !THUMBS.with_borrow(|thumbs| thumbs.contains_key(&key))
        && !RETRY.with_borrow(|retry| retry.contains_key(&key))
}

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
    let key = key_of(row, size, scale, icon);
    if visible {
        DRAWN.with_borrow_mut(|drawn| drawn.insert(row.path.clone()));
    }
    if let Some(image) = THUMBS.with_borrow(|thumbs| thumbs.get(&key).cloned()) {
        return Some(image);
    }
    if !visible {
        return None;
    }
    if ASKED.with_borrow(|asked| asked.contains(&key)) {
        return largest(&row.path, icon);
    }
    let path = if crate::remote::is_remote(&row.path) {
        crate::remote::cached_copy(row, key.1)?
    } else {
        PathBuf::from(row.path.as_ref())
    };
    let path = path.as_path();
    let modified = Duration::try_from_secs_f64(row.modified_at)
        .ok()
        .map(|since| UNIX_EPOCH + since);
    if let Some(image) = thumbnails::cached_thumbnail(path, modified, px(size), scale, icon) {
        keep(key, image.clone());
        return Some(image);
    }
    ask(path, modified, key, size, scale);
    largest(&row.path, icon)
}

fn ask(path: &Path, modified: Option<SystemTime>, key: Thumb, size: f32, scale: f32) {
    if RETRY.with_borrow(|retry| {
        retry
            .get(&key)
            .is_some_and(|(_, again)| Instant::now() < *again)
    }) {
        return;
    }
    if !ASKED.with_borrow_mut(|asked| asked.insert(key.clone())) {
        return;
    }
    let ask = (path.to_path_buf(), modified, key, size, scale);
    if INFLIGHT.load(Ordering::Relaxed) < LANES {
        send(ask);
    } else {
        HELD.with_borrow_mut(|held| held.push(ask));
    }
}

fn send((path, modified, key, size, scale): Ask) {
    INFLIGHT.fetch_add(1, Ordering::Relaxed);
    thumbnails::thumbnail(&path, modified, px(size), scale, key.2, move |image| {
        LANDED.lock().push((key, image));
        INFLIGHT.fetch_sub(1, Ordering::Relaxed);
        LANDING.ring();
    });
}
