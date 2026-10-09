use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use gpui::{RenderImage, px};
use native_gpui::thumbnails;
use parking_lot::Mutex;

const WELL: f32 = 140.;
const POSTER: f32 = 256.;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Size {
    pixels: u32,
    is_icon: bool,
}

static LANDED: Mutex<Vec<(PathBuf, Size, Option<Arc<RenderImage>>)>> = Mutex::new(Vec::new());

thread_local! {
    static CACHE: RefCell<HashMap<PathBuf, HashMap<Size, Arc<RenderImage>>>> =
        RefCell::new(HashMap::new());
    static ASKED: RefCell<HashSet<(PathBuf, Size)>> = RefCell::new(HashSet::new());
}

pub fn collect() {
    let landed = std::mem::take(&mut *LANDED.lock());
    CACHE.with_borrow_mut(|cache| {
        for (path, size, image) in landed {
            if let Some(image) = image {
                cache.entry(path).or_default().insert(size, image);
            }
        }
    });
}

pub fn has_pending() -> bool {
    !LANDED.lock().is_empty()
}

fn cached(path: &Path, size: Size) -> Option<Arc<RenderImage>> {
    CACHE.with_borrow(|cache| cache.get(path)?.get(&size).cloned())
}

fn best(path: &Path) -> Option<Arc<RenderImage>> {
    CACHE.with_borrow(|cache| {
        cache
            .get(path)?
            .iter()
            .max_by_key(|(size, _)| (!size.is_icon, size.pixels))
            .map(|(_, image)| image.clone())
    })
}

fn store(path: &Path, size: Size, image: &Arc<RenderImage>) {
    CACHE.with_borrow_mut(|cache| {
        cache
            .entry(path.to_path_buf())
            .or_default()
            .insert(size, image.clone())
    });
}

fn pick(path: &Path, side: f32, scale: f32) -> Option<Arc<RenderImage>> {
    let step = if scale >= 1.5 { 2.0 } else { 1.0 };
    let pixels = (side * step).round().max(1.0) as u32;
    let size = |is_icon| Size { pixels, is_icon };
    let (poster, icon) = (size(false), size(true));
    if let Some(hit) = cached(path, poster) {
        return Some(hit);
    }
    if let Some(hit) = cached(path, icon).or_else(|| best(path)) {
        ask(path, poster, side, step);
        return Some(hit);
    }
    let modified = modified(path);
    if let Some(hit) = thumbnails::cached_thumbnail(path, modified, px(side), step, false) {
        store(path, poster, &hit);
        return Some(hit);
    }
    if let Some(hit) = thumbnails::cached_thumbnail(path, modified, px(side), step, true) {
        store(path, icon, &hit);
        ask(path, poster, side, step);
        return Some(hit);
    }
    ask(path, icon, side, step);
    ask(path, poster, side, step);
    None
}

pub fn poster(path: &Path, scale: f32) -> Option<Arc<RenderImage>> {
    pick(path, POSTER, scale)
}

pub fn well(path: &Path, scale: f32) -> Option<Arc<RenderImage>> {
    pick(path, WELL, scale)
}

pub fn prefetch(path: &Path) {
    well(path, 2.0);
}

fn ask(path: &Path, size: Size, side: f32, step: f32) {
    if !ASKED.with_borrow_mut(|asked| asked.insert((path.to_path_buf(), size))) {
        return;
    }
    let landed = path.to_path_buf();
    thumbnails::thumbnail(
        path,
        modified(path),
        px(side),
        step,
        size.is_icon,
        move |image| {
            LANDED.lock().push((landed, size, image));
        },
    );
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}
