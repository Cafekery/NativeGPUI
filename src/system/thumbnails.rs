//! Real thumbnails and icons for files, via `QLThumbnailGenerator`. An LRU cache keyed by path,
//! mtime and requested pixel size makes a repeat request instant; in-flight requests for the
//! same key are coalesced into one generation; at most 8 generations run at once so scrolling a
//! large folder queues rather than floods the thumbnailing daemon.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use block2::RcBlock;
use gpui::{Pixels, RenderImage};
use image::{Frame, RgbaImage};
use objc2::AnyThread;
use objc2_app_kit::NSImage;
use objc2_foundation::{NSError, NSSize, NSString, NSURL};
use objc2_quick_look_thumbnailing::{
    QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes,
    QLThumbnailGenerator, QLThumbnailRepresentation,
};

use crate::system::tokens;

const MAX_CACHE: usize = 4096;
const MAX_INFLIGHT: usize = 8;

type Callback = Box<dyn FnOnce(Option<Arc<RenderImage>>) + Send>;

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    mtime: Option<SystemTime>,
    // The final pixel dimension, so two (size, scale) pairs that land on the same raster share
    // a cache entry.
    pixels: u32,
    icon: bool,
}

impl Key {
    fn new(path: &Path, size: Pixels, scale: f32, icon: bool) -> Self {
        Key {
            path: path.to_path_buf(),
            mtime: std::fs::metadata(path).and_then(|m| m.modified()).ok(),
            pixels: (f32::from(size) * scale).round().max(1.0) as u32,
            icon,
        }
    }
}

struct Job {
    key: Key,
    path: PathBuf,
    pixels: u32,
}

#[derive(Default)]
struct State {
    cache: HashMap<Key, Option<Arc<RenderImage>>>,
    order: VecDeque<Key>,
    waiters: HashMap<Key, Vec<Callback>>,
    queue: VecDeque<Job>,
    active: usize,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

/// A previously generated thumbnail, if this exact `(path, mtime, size, scale, icon)` is cached.
///
/// The brief describes the cache key as `(path, mtime, size)`; `scale` is folded into the pixel
/// size that key means, so it is taken here too, to compute the same key [`thumbnail`] would.
pub fn cached_thumbnail(
    path: &Path,
    size: Pixels,
    scale: f32,
    icon: bool,
) -> Option<Arc<RenderImage>> {
    let key = Key::new(path, size, scale, icon);
    STATE.lock().unwrap().cache.get(&key).cloned().flatten()
}

/// Request a thumbnail (or, failing that, an icon) for `path`, delivered to `done` once
/// generated. A cache hit calls `done` immediately, on the calling thread. Requests for the same
/// key already in flight share one generation. `icon` asks Quick Look for Finder's icon
/// rendering rather than the picture on its own.
pub fn thumbnail(
    path: &Path,
    size: Pixels,
    scale: f32,
    icon: bool,
    done: impl FnOnce(Option<Arc<RenderImage>>) + Send + 'static,
) {
    let key = Key::new(path, size, scale, icon);
    let mut state = STATE.lock().unwrap();
    if let Some(hit) = state.cache.get(&key) {
        let hit = hit.clone();
        drop(state);
        done(hit);
        return;
    }
    if let Some(waiters) = state.waiters.get_mut(&key) {
        waiters.push(Box::new(done));
        return;
    }
    state.waiters.insert(key.clone(), vec![Box::new(done)]);
    let job = Job {
        pixels: key.pixels,
        key,
        path: path.to_path_buf(),
    };
    if state.active < MAX_INFLIGHT {
        state.active += 1;
        drop(state);
        generate(job);
    } else {
        state.queue.push_back(job);
    }
}

/// Drop queued (not yet started) requests for paths outside `paths`, so scrolling past a file
/// does not leave its thumbnail generation queued behind everything still on screen.
pub fn cancel_thumbnails_outside(paths: &[PathBuf]) {
    let keep: HashSet<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let stranded: Vec<Callback> = {
        let mut state = STATE.lock().unwrap();
        let mut stale = Vec::new();
        state.queue.retain(|job| {
            if keep.contains(job.path.as_path()) {
                true
            } else {
                stale.push(job.key.clone());
                false
            }
        });
        stale
            .into_iter()
            .flat_map(|key| state.waiters.remove(&key).into_iter().flatten())
            .collect()
    };
    for callback in stranded {
        callback(None);
    }
}

fn generate(job: Job) {
    let points = NSSize::new(job.pixels as f64, job.pixels as f64);
    let url = NSURL::fileURLWithPath(&NSString::from_str(&job.path.to_string_lossy()));
    let request = unsafe {
        QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
            QLThumbnailGenerationRequest::alloc(),
            &url,
            points,
            1.0,
            QLThumbnailGenerationRequestRepresentationTypes::Thumbnail
                | QLThumbnailGenerationRequestRepresentationTypes::Icon,
        )
    };
    // Icon mode is Finder's own rendering: the fitted picture with its keyline, corner radius
    // and shadow baked into a raster the size of the box.
    unsafe { request.setIconMode(job.key.icon) };

    let key = job.key;
    let pixels = job.pixels as usize;
    let block = RcBlock::new(
        move |rep: *mut QLThumbnailRepresentation, _err: *mut NSError| {
            let image = unsafe { rep.as_ref() }.map(|rep| unsafe { rep.NSImage() });
            let result = image.and_then(|image| render(&image, pixels));
            complete(key.clone(), result);
        },
    );
    unsafe {
        QLThumbnailGenerator::sharedGenerator()
            .generateBestRepresentationForRequest_completionHandler(&request, &block);
    }
}

/// Draws at the size Quick Look chose within the requested box, capped by `max_pixels` (the box
/// side we asked the generator for) so a source Quick Look upscales never grows past it.
fn render(image: &NSImage, max_pixels: usize) -> Option<Arc<RenderImage>> {
    let fit = image.size();
    let w = (fit.width.round() as usize).clamp(1, max_pixels.max(1));
    let h = (fit.height.round() as usize).clamp(1, max_pixels.max(1));
    let points = NSSize::new(w as f64, h as f64);
    let rgba = tokens::draw(image, points, w, h)?;
    let buf = RgbaImage::from_raw(w as u32, h as u32, tokens::to_bgra_straight(&rgba))?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(buf)])))
}

fn complete(key: Key, result: Option<Arc<RenderImage>>) {
    let (callbacks, next) = {
        let mut state = STATE.lock().unwrap();
        state.active -= 1;
        state.cache.insert(key.clone(), result.clone());
        state.order.push_back(key.clone());
        while state.order.len() > MAX_CACHE {
            if let Some(old) = state.order.pop_front() {
                state.cache.remove(&old);
            }
        }
        let callbacks = state.waiters.remove(&key).unwrap_or_default();
        let next = (state.active < MAX_INFLIGHT)
            .then(|| state.queue.pop_front())
            .flatten();
        if next.is_some() {
            state.active += 1;
        }
        (callbacks, next)
    };
    for callback in callbacks {
        callback(result.clone());
    }
    if let Some(job) = next {
        generate(job);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn write_png(path: &Path, side: u32) {
        let img = RgbaImage::from_pixel(side, side, image::Rgba([200, 40, 40, 255]));
        img.save(path).expect("write test png");
    }

    #[test]
    fn a_png_thumbnails_to_the_requested_size() {
        let dir = std::env::temp_dir().join(format!("ng-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.png");
        write_png(&path, 64);

        let (tx, rx) = mpsc::channel();
        thumbnail(&path, Pixels::from(32.0), 2.0, true, move |image| {
            tx.send(image).unwrap();
        });
        let image = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("thumbnail within 2s")
            .expect("a png always thumbnails");
        assert_eq!(u32::from(image.size(0).width), 64);
        assert_eq!(u32::from(image.size(0).height), 64);

        let before = STATE.lock().unwrap().cache.len();
        let (tx2, rx2) = mpsc::channel();
        thumbnail(&path, Pixels::from(32.0), 2.0, true, move |image| {
            tx2.send(image).unwrap();
        });
        let second = rx2
            .recv_timeout(Duration::from_secs(2))
            .expect("cache hit is instant")
            .expect("still there");
        assert!(Arc::ptr_eq(&image, &second), "same cached raster");
        assert_eq!(STATE.lock().unwrap().cache.len(), before, "no new entry");

        assert!(cached_thumbnail(&path, Pixels::from(32.0), 2.0, true).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plain_folder_has_no_custom_icon() {
        let dir = std::env::temp_dir().join(format!("ng-thumb-folder-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!tokens::has_custom_icon(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
