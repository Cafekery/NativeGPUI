use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui::{RenderImage, px};

use native_gpui::thumbnails;

// Well cards are ~140pt; browse uses a larger poster. Icon-mode lands first.
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
    static CACHE: std::cell::RefCell<HashMap<Key, Arc<RenderImage>>> =
        std::cell::RefCell::new(HashMap::new());
    static ASKED: std::cell::RefCell<HashSet<Key>> =
        std::cell::RefCell::new(HashSet::new());
}

fn scale_step(scale: f32) -> f32 {
    if scale >= 1.5 { 2.0 } else { 1.0 }
}

fn key(path: &Path, size: f32, scale: f32, is_icon: bool) -> Key {
    Key {
        path: path.to_path_buf(),
        pixels: (size * scale_step(scale)).round().max(1.0) as u32,
        is_icon,
    }
}

pub fn collect() -> bool {
    let landed = std::mem::take(&mut *LANDED.lock().unwrap());
    if landed.is_empty() {
        return false;
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        for (key, image) in landed {
            if let Some(image) = image {
                cache.insert(key, image);
            }
        }
    });
    true
}

pub fn has_pending() -> bool {
    !LANDED.lock().unwrap().is_empty()
}

fn best(path: &Path) -> Option<Arc<RenderImage>> {
    CACHE.with(|cache| {
        let cache = cache.borrow();
        let poster = cache
            .iter()
            .filter(|(held, _)| held.path == path && !held.is_icon)
            .max_by_key(|(held, _)| held.pixels)
            .map(|(_, image)| image.clone());
        poster.or_else(|| {
            cache
                .iter()
                .filter(|(held, _)| held.path == path)
                .max_by_key(|(held, _)| held.pixels)
                .map(|(_, image)| image.clone())
        })
    })
}

fn pick(path: &Path, size: f32, scale: f32) -> Option<Arc<RenderImage>> {
    let poster = key(path, size, scale, false);
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&poster).cloned()) {
        return Some(hit);
    }
    let icon = key(path, size, scale, true);
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&icon).cloned()) {
        ask(path, size, scale, false);
        return Some(hit);
    }
    if let Some(hit) = best(path) {
        ask(path, size, scale, false);
        return Some(hit);
    }
    if let Some(hit) = thumbnails::cached_thumbnail(path, px(size), scale_step(scale), false) {
        CACHE.with(|c| c.borrow_mut().insert(poster, hit.clone()));
        return Some(hit);
    }
    if let Some(hit) = thumbnails::cached_thumbnail(path, px(size), scale_step(scale), true) {
        CACHE.with(|c| c.borrow_mut().insert(icon, hit.clone()));
        ask(path, size, scale, false);
        return Some(hit);
    }
    ask(path, size, scale, true);
    ask(path, size, scale, false);
    None
}

pub fn get(path: &Path, scale: f32) -> Option<Arc<RenderImage>> {
    pick(path, POSTER, scale)
}

pub fn well(path: &Path, scale: f32) -> Option<Arc<RenderImage>> {
    pick(path, WELL, scale)
}

pub fn prefetch(path: &Path) {
    let _ = well(path, 2.0);
}

fn ask(path: &Path, size: f32, scale: f32, is_icon: bool) {
    let key = key(path, size, scale, is_icon);
    if ASKED.with(|asked| !asked.borrow_mut().insert(key.clone())) {
        return;
    }
    thumbnails::thumbnail(path, px(size), scale_step(scale), is_icon, move |image| {
        LANDED.lock().unwrap().push((key, image));
    });
}
