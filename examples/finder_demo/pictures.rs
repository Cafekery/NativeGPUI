use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;

use gpui::{App, RenderImage, SharedString};
use image::{Frame, RgbaImage};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_foundation::{NSDictionary, NSNumber, NSPoint, NSRect, NSSize, NSString, NSURL};
use parking_lot::Mutex;

use crate::data::{Face, Row};
use crate::signal::Signal;

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithURL(url: &NSURL, options: *const NSDictionary) -> *mut AnyObject;
    fn CGImageSourceCreateThumbnailAtIndex(
        source: &AnyObject,
        index: usize,
        options: &NSDictionary<NSString, NSNumber>,
    ) -> *mut AnyObject;
    static kCGImageSourceCreateThumbnailFromImageAlways: &'static NSString;
    static kCGImageSourceCreateThumbnailFromImageIfAbsent: &'static NSString;
    static kCGImageSourceCreateThumbnailWithTransform: &'static NSString;
    static kCGImageSourceThumbnailMaxPixelSize: &'static NSString;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGImageGetWidth(image: &AnyObject) -> usize;
    fn CGImageGetHeight(image: &AnyObject) -> usize;
    fn CGColorSpaceCreateDeviceRGB() -> *mut AnyObject;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: &AnyObject,
        bitmap_info: u32,
    ) -> *mut AnyObject;
    fn CGContextDrawImage(context: &AnyObject, rect: NSRect, image: &AnyObject);
}

/// `kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little`: BGRA in memory.
const BGRA: u32 = 2 | 2 << 12;
const WORKERS: usize = 2;
const KEPT: usize = 24;
const KEPT_BYTES: usize = 192 << 20;
/// A window being resized decodes again only once it outgrows its picture by this much.
const RESIZE_STEP: u32 = 256;

pub(crate) enum Hero {
    Picture(Arc<RenderImage>),
    Waiting,
    QuickLook,
}

#[derive(Clone, PartialEq)]
struct Key {
    path: SharedString,
    modified: u64,
}

struct Kept {
    key: Key,
    /// `u32::MAX` once nothing sharper can be had.
    pixels: u32,
    image: Option<Arc<RenderImage>>,
}

struct Pictures {
    /// Least recently wanted first.
    kept: Vec<Kept>,
    /// Most wanted first.
    wanted: Vec<(Key, u32)>,
    making: Vec<Key>,
    workers: usize,
}

static PICTURES: Mutex<Pictures> = Mutex::new(Pictures {
    kept: Vec::new(),
    wanted: Vec::new(),
    making: Vec::new(),
    workers: 0,
});
static RETIRED: Mutex<Vec<Arc<RenderImage>>> = Mutex::new(Vec::new());
static LANDED: Signal = Signal::new();

impl Pictures {
    fn kept(&self, key: &Key) -> Option<&Kept> {
        self.kept.iter().find(|kept| &kept.key == key)
    }

    fn jobs(&self) -> impl Iterator<Item = &(Key, u32)> {
        self.wanted.iter().filter(|(key, pixels)| {
            !self.making.contains(key) && self.kept(key).is_none_or(|kept| kept.pixels < *pixels)
        })
    }

    fn keep(&mut self, key: Key, pixels: u32, image: Option<Arc<RenderImage>>) {
        let mut retired = Vec::new();
        if let Some(at) = self.kept.iter().position(|kept| kept.key == key) {
            retired.extend(self.kept.remove(at).image);
        }
        self.kept.push(Kept { key, pixels, image });
        while self.kept.len() > KEPT || self.bytes() > KEPT_BYTES {
            let wanted = |kept: &Kept| self.wanted.iter().any(|(key, _)| key == &kept.key);
            let Some(at) = self.kept.iter().position(|kept| !wanted(kept)) else {
                break;
            };
            retired.extend(self.kept.remove(at).image);
        }
        RETIRED.lock().extend(retired);
    }

    fn bytes(&self) -> usize {
        self.kept
            .iter()
            .filter_map(|kept| kept.image.as_ref()?.as_bytes(0))
            .map(<[u8]>::len)
            .sum()
    }
}

fn key(row: &Row) -> Key {
    Key {
        path: row.path.clone(),
        modified: row.modified_at.to_bits(),
    }
}

/// Reading a dataless file downloads it; Quick Look draws those from the provider's thumbnail.
pub(crate) fn decodes(row: &Row) -> bool {
    row.face == Face::Image && !row.dataless
}

/// Decodes `rows`, most wanted first, at `pixels` on their longer side.
pub(crate) fn want(rows: &[&Row], pixels: u32) {
    let pixels = pixels.div_ceil(RESIZE_STEP) * RESIZE_STEP;
    let wanted: Vec<(Key, u32)> = rows.iter().map(|row| (key(row), pixels)).collect();
    let mut pictures = PICTURES.lock();
    if pictures.wanted == wanted {
        return;
    }
    for (key, _) in wanted.iter().rev() {
        if let Some(at) = pictures.kept.iter().position(|kept| &kept.key == key) {
            let kept = pictures.kept.remove(at);
            pictures.kept.push(kept);
        }
    }
    pictures.wanted = wanted;
    let busy = (pictures.making.len() + pictures.jobs().count()).min(WORKERS);
    while pictures.workers < busy {
        pictures.workers += 1;
        std::thread::spawn(make);
    }
}

/// The best picture of `row` so far, standing in its filmstrip thumbnail until one is decoded.
pub(crate) fn hero(row: &Row, thumb: impl FnOnce() -> Option<Arc<RenderImage>>) -> Hero {
    let key = key(row);
    let mut pictures = PICTURES.lock();
    match pictures.kept(&key) {
        Some(Kept {
            image: Some(image), ..
        }) => return Hero::Picture(image.clone()),
        Some(Kept { image: None, .. }) => return Hero::QuickLook,
        None => {}
    }
    let Some(image) = thumb().and_then(|thumb| opaque(&thumb)) else {
        return Hero::Waiting;
    };
    pictures.keep(key, pixels_of(&image), Some(image.clone()));
    Hero::Picture(image)
}

pub(crate) fn follow_pictures(cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut seen = 0;
        loop {
            seen = LANDED.after(seen).await;
            let retired = std::mem::take(&mut *RETIRED.lock());
            cx.update(|cx| {
                for image in retired {
                    cx.drop_image(image, None);
                }
                crate::read::notify_windows(cx);
            });
        }
    })
    .detach();
}

fn make() {
    loop {
        let (key, pixels) = {
            let mut pictures = PICTURES.lock();
            let Some(job) = pictures.jobs().next().cloned() else {
                pictures.workers -= 1;
                return;
            };
            pictures.making.push(job.0.clone());
            job
        };
        let image = autoreleasepool(|_| decode(&key.path, pixels));
        let pixels = match &image {
            Some(image) if pixels_of(image) >= pixels => pixels,
            _ => u32::MAX,
        };
        let mut pictures = PICTURES.lock();
        pictures.making.retain(|making| making != &key);
        pictures.keep(key, pixels, image);
        drop(pictures);
        LANDED.ring();
    }
}

fn decode(path: &str, pixels: u32) -> Option<Arc<RenderImage>> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    let source = unsafe { Retained::from_raw(CGImageSourceCreateWithURL(&url, ptr::null())) }?;
    let thumbnail = |from: &NSString| {
        let (yes, most) = (NSNumber::new_bool(true), NSNumber::new_u32(pixels));
        let options = NSDictionary::from_slices(
            &[
                from,
                unsafe { kCGImageSourceCreateThumbnailWithTransform },
                unsafe { kCGImageSourceThumbnailMaxPixelSize },
            ],
            &[&*yes, &*yes, &*most],
        );
        unsafe { Retained::from_raw(CGImageSourceCreateThumbnailAtIndex(&source, 0, &options)) }
    };
    // A camera raw's embedded preview is near full size; a JPEG's is a postage stamp.
    let image = thumbnail(unsafe { kCGImageSourceCreateThumbnailFromImageIfAbsent })
        .filter(|image| 4 * long_side(image) >= 3 * pixels as usize)
        .or_else(|| thumbnail(unsafe { kCGImageSourceCreateThumbnailFromImageAlways }))?;
    bgra(&image)
}

fn long_side(image: &AnyObject) -> usize {
    unsafe { CGImageGetWidth(image).max(CGImageGetHeight(image)) }
}

fn pixels_of(image: &RenderImage) -> u32 {
    let size = image.size(0);
    u32::from(size.width).max(u32::from(size.height))
}

/// Straight-alpha BGRA, as `RenderImage` takes it.
fn bgra(image: &AnyObject) -> Option<Arc<RenderImage>> {
    let (w, h) = unsafe { (CGImageGetWidth(image), CGImageGetHeight(image)) };
    let mut bytes = vec![0u8; w * h * 4];
    let space = unsafe { Retained::from_raw(CGColorSpaceCreateDeviceRGB()) }?;
    let context = unsafe {
        Retained::from_raw(CGBitmapContextCreate(
            bytes.as_mut_ptr().cast(),
            w,
            h,
            8,
            w * 4,
            &space,
            BGRA,
        ))
    }?;
    let rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(w as f64, h as f64));
    unsafe { CGContextDrawImage(&context, rect, image) };
    drop(context);
    for pixel in bytes.chunks_exact_mut(4) {
        let a = u32::from(pixel[3]);
        if a > 0 && a < 255 {
            for c in &mut pixel[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let raster = RgbaImage::from_raw(w as u32, h as u32, bytes)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(raster)])))
}

/// Quick Look's icon mode pads a picture with a drop shadow: the opaque part is the picture.
fn opaque(thumb: &RenderImage) -> Option<Arc<RenderImage>> {
    let size = thumb.size(0);
    let (w, h) = (
        u32::from(size.width) as usize,
        u32::from(size.height) as usize,
    );
    let bytes = thumb.as_bytes(0)?;
    let solid = |x: usize, y: usize| bytes[(y * w + x) * 4 + 3] == 255;
    let rows: Vec<usize> = (0..h).filter(|&y| (0..w).any(|x| solid(x, y))).collect();
    let columns: Vec<usize> = (0..w).filter(|&x| (0..h).any(|y| solid(x, y))).collect();
    let (top, bottom) = (*rows.first()?, *rows.last()?);
    let (left, right) = (*columns.first()?, *columns.last()?);
    let picture: Vec<u8> = (top..=bottom)
        .flat_map(|y| &bytes[(y * w + left) * 4..(y * w + right + 1) * 4])
        .copied()
        .collect();
    let raster = RgbaImage::from_raw(
        (right - left + 1) as u32,
        (bottom - top + 1) as u32,
        picture,
    )?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(raster)])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageEncoder};

    /// Big-endian TIFF holding one tag: orientation 6, the way a camera stores a portrait.
    const PORTRAIT: &[u8] = b"MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0";

    #[test]
    fn a_photo_decodes_upright_at_the_asked_size() {
        let mut bytes = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut bytes, 90);
        encoder.set_exif_metadata(PORTRAIT.to_vec()).unwrap();
        let red = [255, 0, 0].repeat(400 * 200);
        encoder
            .write_image(&red, 400, 200, ExtendedColorType::Rgb8)
            .unwrap();
        let path = std::env::temp_dir().join(format!("ng-pictures-{}.jpg", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let picture = decode(path.to_str().unwrap(), 100);
        std::fs::remove_file(&path).ok();
        let picture = picture.expect("ImageIO decodes a JPEG");
        let size = picture.size(0);
        assert_eq!((u32::from(size.width), u32::from(size.height)), (50, 100));
        let centre = &picture.as_bytes(0).unwrap()[(50 * 50 + 25) * 4..][..4];
        assert!(
            centre[2] > 200 && centre[0] < 60,
            "BGRA, red third: {centre:?}"
        );
    }
}
