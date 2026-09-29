use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use gpui::{RenderImage, px};
use native_gpui::thumbnails;

const WELL: f32 = 140.;
const POSTER: f32 = 256.;

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    pixels: u32,
    is_icon: bool,
}

static LANDED: Mutex<Vec<(Key, Option<Arc<RenderImage>>)>> = Mutex::new(Vec::new());

thread_local! {
    static CACHE: RefCell<HashMap<Key, Arc<RenderImage>>> = RefCell::new(HashMap::new());
    static ASKED: RefCell<HashSet<Key>> = RefCell::new(HashSet::new());
}

fn scale_step(scale: f32) -> f32 {
    if scale >= 1.5 { 2.0 } else { 1.0 }
}

fn key(path: &Path, size: f32, step: f32, is_icon: bool) -> Key {
    Key {
        path: path.to_path_buf(),
        pixels: (size * step).round().max(1.0) as u32,
        is_icon,
    }
}

pub fn collect() {
    let landed = std::mem::take(&mut *LANDED.lock().unwrap());
    if landed.is_empty() {
        return;
    }
    CACHE.with_borrow_mut(|cache| {
        cache.extend(
            landed
                .into_iter()
                .filter_map(|(key, image)| Some((key, image?))),
        )
    });
}

pub fn has_pending() -> bool {
    !LANDED.lock().unwrap().is_empty()
}

fn cached(key: &Key) -> Option<Arc<RenderImage>> {
    CACHE.with_borrow(|cache| cache.get(key).cloned())
}

fn best(path: &Path) -> Option<Arc<RenderImage>> {
    CACHE.with_borrow(|cache| {
        cache
            .iter()
            .filter(|(held, _)| held.path == path)
            .max_by_key(|(held, _)| (!held.is_icon, held.pixels))
            .map(|(_, image)| image.clone())
    })
}

fn pick(path: &Path, size: f32, scale: f32) -> Option<Arc<RenderImage>> {
    let step = scale_step(scale);
    let poster = key(path, size, step, false);
    if let Some(hit) = cached(&poster) {
        return Some(hit);
    }
    let icon = key(path, size, step, true);
    if let Some(hit) = cached(&icon).or_else(|| best(path)) {
        ask(path, poster, size, step);
        return Some(hit);
    }
    let modified = modified(path);
    if let Some(hit) = thumbnails::cached_thumbnail(path, modified, px(size), step, false) {
        CACHE.with_borrow_mut(|cache| cache.insert(poster, hit.clone()));
        return Some(hit);
    }
    if let Some(hit) = thumbnails::cached_thumbnail(path, modified, px(size), step, true) {
        CACHE.with_borrow_mut(|cache| cache.insert(icon, hit.clone()));
        ask(path, poster, size, step);
        return Some(hit);
    }
    ask(path, icon, size, step);
    ask(path, poster, size, step);
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

fn ask(path: &Path, key: Key, size: f32, step: f32) {
    if !ASKED.with_borrow_mut(|asked| asked.insert(key.clone())) {
        return;
    }
    let is_icon = key.is_icon;
    thumbnails::thumbnail(
        path,
        modified(path),
        px(size),
        step,
        is_icon,
        move |image| {
            LANDED.lock().unwrap().push((key, image));
        },
    );
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}
