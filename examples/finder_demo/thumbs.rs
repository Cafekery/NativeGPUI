use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{App, Entity, RenderImage, SharedString, px};
use native_gpui::thumbnails;

use crate::app::FinderDemo;
use crate::data::{Face, Row};

type Thumb = (SharedString, u32, bool);

type Ask = (PathBuf, Option<SystemTime>, Thumb, f32, f32, bool);

type Landed = HashMap<(SharedString, bool), (u32, Arc<RenderImage>)>;

thread_local! {
    static THUMBS: RefCell<HashMap<Thumb, Arc<RenderImage>>> = RefCell::new(HashMap::new());
    static ASKED: RefCell<HashSet<Thumb>> = RefCell::new(HashSet::new());
    static LARGEST: RefCell<Landed> = RefCell::new(HashMap::new());
    static DRAWN: RefCell<HashSet<SharedString>> = RefCell::new(HashSet::new());
    static RETRY: RefCell<HashMap<Thumb, (u32, Instant)>> = RefCell::new(HashMap::new());
    static FRAMES: Cell<u64> = const { Cell::new(0) };
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    static SWEPT: Cell<(Option<Instant>, u64)> = const { Cell::new((None, 0)) };
    static HELD: RefCell<Vec<Ask>> = const { RefCell::new(Vec::new()) };
}

static DELIVERED: AtomicUsize = AtomicUsize::new(0);
static INFLIGHT: AtomicUsize = AtomicUsize::new(0);
static LANDED: Mutex<Vec<(Thumb, Option<Arc<RenderImage>>)>> = Mutex::new(Vec::new());

const SWEEP: Duration = Duration::from_millis(250);
const WATCH: Duration = Duration::from_millis(8);
const GIVE_UP: u32 = 250;
const RETRY_AFTER: Duration = Duration::from_millis(250);
const RETRIES: u32 = 5;
const KEPT: usize = 512;
const KEPT_BYTES: usize = 256 << 20;
/// The thumbnail service's own limit; past it, requests queue there and the newest goes first.
const LANES: usize = 8;

pub(crate) fn thumbnails_landed() -> usize {
    DELIVERED.load(Ordering::Relaxed)
}

pub(crate) fn pending() -> bool {
    INFLIGHT.load(Ordering::Relaxed) > 0
}

pub(crate) fn watch_thumbnails(demo: &Entity<FinderDemo>, cx: &mut App) {
    if !pending() || WATCHING.with(|watching| watching.replace(true)) {
        return;
    }
    let demo = demo.downgrade();
    cx.spawn(async move |cx| {
        let mut quiet = 0;
        loop {
            cx.background_executor().timer(WATCH).await;
            let landed = collect_thumbnails();
            // A generation whose handler is never called would leave this looking for ever.
            quiet = if landed { 0 } else { quiet + 1 };
            let done = !pending() || quiet > GIVE_UP;
            let alive = demo
                .update(cx, |_, cx| {
                    if landed {
                        cx.notify();
                    }
                })
                .is_ok();
            if done || !alive {
                break;
            }
        }
        WATCHING.with(|watching| watching.set(false));
    })
    .detach();
}

/// Main thread only.
pub(crate) fn collect_thumbnails() -> bool {
    // Bottom row first: the service serves its newest request first, so a frame fills top down.
    for ask in HELD.take().into_iter().rev() {
        send(ask);
    }
    let landed = std::mem::take(&mut *LANDED.lock().unwrap());
    let any = !landed.is_empty();
    for (key, image) in landed {
        let Some(image) = image else {
            retry(&key);
            continue;
        };
        keep(&key, &image);
        THUMBS.with(|thumbs| thumbs.borrow_mut().insert(key, image));
    }
    any
}

/// An empty delivery is often a cancelled generation, so ask again, backing off.
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
    // Off-screen thumbnails come back from the library's own bounded cache when scrolled to.
    let (count, bytes) = THUMBS.with(|thumbs| {
        let thumbs = thumbs.borrow();
        let bytes = thumbs
            .values()
            .map(|image| image.as_bytes(0).map_or(0, <[u8]>::len))
            .sum::<usize>();
        (thumbs.len(), bytes)
    });
    if count > KEPT || bytes > KEPT_BYTES {
        THUMBS.with(|thumbs| {
            thumbs
                .borrow_mut()
                .retain(|(path, _, _), _| drawn.contains(path))
        });
        LARGEST.with(|largest| {
            largest
                .borrow_mut()
                .retain(|(path, _), _| drawn.contains(path))
        });
        RETRY.with(|retry| {
            retry
                .borrow_mut()
                .retain(|(path, _, _), _| drawn.contains(path))
        });
    }
    let paths: Vec<PathBuf> = drawn
        .iter()
        .map(|path| PathBuf::from(path.as_ref()))
        .collect();
    thumbnails::cancel_thumbnails_outside(&paths);
}

fn renderable(row: &Row) -> bool {
    row.opens.is_none() && row.face != Face::Program
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
    let key = (
        row.path.clone(),
        (size * scale).round().max(1.) as u32,
        icon,
    );
    if visible {
        DRAWN.with(|drawn| drawn.borrow_mut().insert(row.path.clone()));
    }
    if let Some(image) = THUMBS.with(|thumbs| thumbs.borrow().get(&key).cloned()) {
        return Some(image);
    }
    if !visible {
        return None;
    }
    if ASKED.with(|asked| asked.borrow().contains(&key)) {
        return largest(&row.path, icon);
    }
    let path = Path::new(row.path.as_ref());
    let modified = Duration::try_from_secs_f64(row.modified_at)
        .ok()
        .map(|since| UNIX_EPOCH + since);
    if let Some(image) = thumbnails::cached_thumbnail(path, modified, px(size), scale, icon) {
        keep(&key, &image);
        THUMBS.with(|thumbs| thumbs.borrow_mut().insert(key, image.clone()));
        return Some(image);
    }
    ask(path, modified, key, size, scale, icon);
    largest(&row.path, icon)
}

fn ask(path: &Path, modified: Option<SystemTime>, key: Thumb, size: f32, scale: f32, icon: bool) {
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
    let ask = (path.to_path_buf(), modified, key, size, scale, icon);
    match INFLIGHT.load(Ordering::Relaxed) < LANES {
        true => send(ask),
        false => HELD.with_borrow_mut(|held| held.push(ask)),
    }
}

fn send((path, modified, key, size, scale, icon): Ask) {
    INFLIGHT.fetch_add(1, Ordering::Relaxed);
    thumbnails::thumbnail(&path, modified, px(size), scale, icon, move |image| {
        LANDED.lock().unwrap().push((key, image));
        DELIVERED.fetch_add(1, Ordering::Relaxed);
        INFLIGHT.fetch_sub(1, Ordering::Relaxed);
    });
}
