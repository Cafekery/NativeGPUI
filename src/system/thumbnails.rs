use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use block2::RcBlock;
use gpui::{Pixels, RenderImage};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{NSBitmapFormat, NSCompositingOperation, NSImage};
use objc2_foundation::{NSError, NSSize, NSString, NSURL};
use objc2_quick_look_thumbnailing::{
    QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes,
    QLThumbnailGenerator, QLThumbnailRepresentation,
};

use crate::system::tokens;

const MAX_CACHE: usize = 4096;
const MAX_BYTES: usize = 512 << 20;
const MAX_INFLIGHT: usize = 8;

type Callback = Box<dyn FnOnce(Option<Arc<RenderImage>>) + Send>;

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    mtime: Option<SystemTime>,
    pixels: u32,
    icon: bool,
}

impl Key {
    /// The caller brings `modified`: a `stat` here would run on the UI thread and hang it on a
    /// volume that stopped answering.
    fn new(
        path: &Path,
        modified: Option<SystemTime>,
        size: Pixels,
        scale: f32,
        icon: bool,
    ) -> Self {
        Key {
            path: path.to_path_buf(),
            mtime: modified,
            pixels: (f32::from(size) * scale).round().max(1.0) as u32,
            icon,
        }
    }
}

#[derive(Default)]
struct State {
    cache: HashMap<Key, Option<Arc<RenderImage>>>,
    order: VecDeque<Key>,
    bytes: usize,
    waiters: HashMap<Key, Vec<Callback>>,
    queue: VecDeque<Key>,
    active: usize,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

pub fn cached_thumbnail(
    path: &Path,
    modified: Option<SystemTime>,
    size: Pixels,
    scale: f32,
    icon: bool,
) -> Option<Arc<RenderImage>> {
    let key = Key::new(path, modified, size, scale, icon);
    STATE.lock().unwrap().cache.get(&key).cloned().flatten()
}

pub(crate) fn cached_image(path: &Path) -> Option<Retained<NSImage>> {
    let raster = largest_cached(path)?;
    let size = raster.size(0);
    let (w, h) = (
        u32::from(size.width) as usize,
        u32::from(size.height) as usize,
    );
    let rep = tokens::bitmap(w, h, NSBitmapFormat::AlphaNonpremultiplied)?;
    let src = raster.as_bytes(0)?;
    let dst = rep.bitmapData();
    if dst.is_null() || src.len() < w * h * 4 {
        return None;
    }
    let stride = rep.bytesPerRow() as usize;
    for y in 0..h {
        for x in 0..w {
            let from = &src[(y * w + x) * 4..];
            let to = unsafe { dst.add(y * stride + x * 4) };
            for (at, channel) in [from[2], from[1], from[0], from[3]].into_iter().enumerate() {
                unsafe { *to.add(at) = channel };
            }
        }
    }
    let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(w as f64, h as f64));
    image.addRepresentation(&rep);
    Some(image)
}

fn largest_cached(path: &Path) -> Option<Arc<RenderImage>> {
    let state = STATE.lock().unwrap();
    state
        .cache
        .iter()
        .filter(|(key, _)| key.path == path)
        .filter_map(|(key, image)| Some((key.pixels, image.clone()?)))
        .max_by_key(|(pixels, _)| *pixels)
        .map(|(_, image)| image)
}

/// `icon` is Finder's icon rendering; a cache hit calls `done` at once, on this thread. Past
/// eight in flight the newest request goes first, so a scrolled list gets the rows it landed on.
/// A file Quick Look cannot preview gets `None`: its stand-in icon is not the one Finder shows.
pub fn thumbnail(
    path: &Path,
    modified: Option<SystemTime>,
    size: Pixels,
    scale: f32,
    icon: bool,
    done: impl FnOnce(Option<Arc<RenderImage>>) + Send + 'static,
) {
    let key = Key::new(path, modified, size, scale, icon);
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
    if state.active < MAX_INFLIGHT {
        state.active += 1;
        drop(state);
        generate(key);
    } else {
        state.queue.push_back(key);
    }
}

pub fn cancel_thumbnails_outside(paths: &[PathBuf]) {
    let keep: HashSet<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let stranded: Vec<Callback> = {
        let mut state = STATE.lock().unwrap();
        let mut stale = Vec::new();
        state.queue.retain(|key| {
            if keep.contains(key.path.as_path()) {
                true
            } else {
                stale.push(key.clone());
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

fn generate(key: Key) {
    let points = NSSize::new(key.pixels as f64, key.pixels as f64);
    let url = NSURL::fileURLWithPath(&NSString::from_str(&key.path.to_string_lossy()));
    let request = unsafe {
        QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
            QLThumbnailGenerationRequest::alloc(),
            &url,
            points,
            1.0,
            QLThumbnailGenerationRequestRepresentationTypes::Thumbnail,
        )
    };
    unsafe { request.setIconMode(key.icon) };

    let pixels = key.pixels as usize;
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

fn render(image: &NSImage, max_pixels: usize) -> Option<Arc<RenderImage>> {
    let fit = image.size();
    let w = (fit.width.round() as usize).clamp(1, max_pixels.max(1));
    let h = (fit.height.round() as usize).clamp(1, max_pixels.max(1));
    tokens::rasterize(
        &[(image, NSCompositingOperation::SourceOver)],
        NSSize::new(w as f64, h as f64),
        w,
        h,
    )
}

fn raster_bytes(image: &RenderImage) -> usize {
    image.as_bytes(0).map_or(0, <[u8]>::len)
}

fn complete(key: Key, result: Option<Arc<RenderImage>>) {
    let (callbacks, next) = {
        let mut state = STATE.lock().unwrap();
        state.active -= 1;
        if let Some(Some(old)) = state.cache.insert(key.clone(), result.clone()) {
            state.bytes -= raster_bytes(&old);
        }
        state.bytes += result.as_deref().map_or(0, raster_bytes);
        state.order.push_back(key.clone());
        while state.order.len() > MAX_CACHE || state.bytes > MAX_BYTES {
            let Some(old) = state.order.pop_front() else {
                break;
            };
            if let Some(Some(image)) = state.cache.remove(&old) {
                state.bytes -= raster_bytes(&image);
            }
        }
        let callbacks = state.waiters.remove(&key).unwrap_or_default();
        let next = (state.active < MAX_INFLIGHT)
            .then(|| state.queue.pop_back())
            .flatten();
        if next.is_some() {
            state.active += 1;
        }
        (callbacks, next)
    };
    for callback in callbacks {
        callback(result.clone());
    }
    if let Some(key) = next {
        generate(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;
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

        let modified = std::fs::metadata(&path).unwrap().modified().ok();
        let (tx, rx) = mpsc::channel();
        thumbnail(
            &path,
            modified,
            Pixels::from(32.0),
            2.0,
            true,
            move |image| {
                tx.send(image).unwrap();
            },
        );
        let image = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("thumbnail within 2s")
            .expect("a png always thumbnails");
        assert_eq!(u32::from(image.size(0).width), 64);
        assert_eq!(u32::from(image.size(0).height), 64);

        let before = STATE.lock().unwrap().cache.len();
        let (tx2, rx2) = mpsc::channel();
        thumbnail(
            &path,
            modified,
            Pixels::from(32.0),
            2.0,
            true,
            move |image| {
                tx2.send(image).unwrap();
            },
        );
        let second = rx2
            .recv_timeout(Duration::from_secs(2))
            .expect("cache hit is instant")
            .expect("still there");
        assert!(Arc::ptr_eq(&image, &second), "same cached raster");
        assert_eq!(STATE.lock().unwrap().cache.len(), before, "no new entry");

        assert!(cached_thumbnail(&path, modified, Pixels::from(32.0), 2.0, true).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cached_raster_becomes_an_image_of_the_same_pixels() {
        let dir = std::env::temp_dir().join(format!("ng-thumb-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.png");
        write_png(&path, 64);
        let (tx, rx) = mpsc::channel();
        let modified = std::fs::metadata(&path).unwrap().modified().ok();
        thumbnail(
            &path,
            modified,
            Pixels::from(32.0),
            2.0,
            false,
            move |image| {
                tx.send(image).unwrap();
            },
        );
        let raster = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("thumbnail within 2s")
            .expect("a png always thumbnails");
        let image = cached_image(&path).expect("the cache has it");
        let size = image.size();
        assert_eq!(size.width as u32, u32::from(raster.size(0).width));
        assert_eq!(size.height as u32, u32::from(raster.size(0).height));

        let side = size.width as usize;
        let drawn = tokens::draw(
            &[(&*image, NSCompositingOperation::SourceOver)],
            NSSize::new(size.width, size.height),
            side,
            side,
        )
        .unwrap();
        let raster = raster.as_bytes(0).unwrap();
        let at = (side * side / 2 + side / 2) * 4;
        assert_eq!(drawn[at], raster[at + 2]);
        assert_eq!(drawn[at + 1], raster[at + 1]);
        assert_eq!(drawn[at + 2], raster[at]);
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
