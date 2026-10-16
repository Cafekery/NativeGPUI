use std::ffi::c_void;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use block2::RcBlock;
use objc2::encode::{Encode, Encoding};
use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{Message, msg_send};
use objc2_foundation::{
    NSArray, NSCopying, NSDictionary, NSError, NSFileManager, NSFileManagerItemReplacementOptions,
    NSNumber, NSSize, NSString,
};

use super::{Done, Op, free_path, io_error, name_of, split_name, url};
use crate::tags::Tag;

#[derive(Clone, Debug)]
pub enum Quick {
    Rotate {
        items: Vec<PathBuf>,
        left: bool,
    },
    CreatePdf(Vec<PathBuf>),
    Convert {
        items: Vec<PathBuf>,
        format: ImageFormat,
        size: ImageSize,
        metadata: bool,
    },
    RemoveBackground(Vec<PathBuf>),
    /// `item` from `start` to `end` seconds, written over `into`, which may be `item` itself.
    Trim {
        item: PathBuf,
        start: f64,
        end: f64,
        into: PathBuf,
        tags: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Jpeg,
    Png,
    Heif,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageSize {
    Actual,
    Large,
    Medium,
    Small,
}

impl ImageFormat {
    pub const ALL: [ImageFormat; 3] = [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Heif];

    pub fn title(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "JPEG",
            ImageFormat::Png => "PNG",
            ImageFormat::Heif => "HEIF",
        }
    }

    fn uti(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "public.jpeg",
            ImageFormat::Png => "public.png",
            ImageFormat::Heif => "public.heic",
        }
    }

    fn ext(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "jpeg",
            ImageFormat::Png => "png",
            ImageFormat::Heif => "heic",
        }
    }
}

impl ImageSize {
    pub const ALL: [ImageSize; 4] = [
        ImageSize::Actual,
        ImageSize::Large,
        ImageSize::Medium,
        ImageSize::Small,
    ];

    pub fn title(self) -> &'static str {
        match self {
            ImageSize::Actual => "Actual Size",
            ImageSize::Large => "Large",
            ImageSize::Medium => "Medium",
            ImageSize::Small => "Small",
        }
    }

    fn longest(self) -> Option<usize> {
        match self {
            ImageSize::Actual => None,
            ImageSize::Large => Some(1280),
            ImageSize::Medium => Some(640),
            ImageSize::Small => Some(320),
        }
    }
}

impl Quick {
    pub fn items(&self) -> &[PathBuf] {
        match self {
            Quick::Rotate { items, .. }
            | Quick::CreatePdf(items)
            | Quick::Convert { items, .. }
            | Quick::RemoveBackground(items) => items,
            Quick::Trim { item, .. } => std::slice::from_ref(item),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Quick::Rotate { left: true, .. } => "Rotate Left",
            Quick::Rotate { left: false, .. } => "Rotate Right",
            Quick::CreatePdf(_) => "Create PDF",
            Quick::Convert { .. } => "Convert Image",
            Quick::RemoveBackground(_) => "Remove Background",
            Quick::Trim { .. } => "Trim",
        }
    }
}

pub(super) fn perform(quick: Quick) -> io::Result<Done> {
    autoreleasepool(|_| match quick {
        Quick::Rotate { items, left } => rotate(items, left),
        Quick::CreatePdf(items) => create_pdf(items),
        Quick::Convert {
            items,
            format,
            size,
            metadata,
        } => convert(items, format, size, metadata),
        Quick::RemoveBackground(items) => remove_background(items),
        Quick::Trim {
            item,
            start,
            end,
            into,
            tags,
        } => trim(&item, start, end, &into, &tags),
    })
}

type Ref = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Affine {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

unsafe impl Encode for Affine {
    const ENCODING: Encoding = Encoding::Struct("CGAffineTransform", &[f64::ENCODING; 6]);
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Time {
    value: i64,
    scale: i32,
    flags: u32,
    epoch: i64,
}

unsafe impl Encode for Time {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[i64::ENCODING, i32::ENCODING, u32::ENCODING, i64::ENCODING],
    );
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TimeRange {
    start: Time,
    duration: Time,
}

unsafe impl Encode for TimeRange {
    const ENCODING: Encoding = Encoding::Struct("?", &[Time::ENCODING, Time::ENCODING]);
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMTimeMakeWithSeconds(seconds: f64, preferred_timescale: i32) -> Time;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: Ref);
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithURL(url: Ref, options: Ref) -> Ref;
    fn CGImageSourceGetType(source: Ref) -> Ref;
    fn CGImageSourceGetCount(source: Ref) -> usize;
    fn CGImageSourceCopyPropertiesAtIndex(source: Ref, index: usize, options: Ref) -> Ref;
    fn CGImageSourceCreateThumbnailAtIndex(source: Ref, index: usize, options: Ref) -> Ref;
    fn CGImageDestinationCreateWithURL(url: Ref, kind: Ref, count: usize, options: Ref) -> Ref;
    fn CGImageDestinationCopyImageSource(
        dest: Ref,
        source: Ref,
        options: Ref,
        error: *mut Ref,
    ) -> bool;
    fn CGImageDestinationAddImageFromSource(dest: Ref, source: Ref, index: usize, properties: Ref);
    fn CGImageDestinationAddImage(dest: Ref, image: Ref, properties: Ref);
    fn CGImageDestinationFinalize(dest: Ref) -> bool;
    static kCGImageDestinationOrientation: Ref;
    static kCGImageDestinationLossyCompressionQuality: Ref;
    static kCGImagePropertyOrientation: Ref;
    static kCGImagePropertyPixelWidth: Ref;
    static kCGImagePropertyPixelHeight: Ref;
    static kCGImagePropertyTIFFDictionary: Ref;
    static kCGImagePropertyTIFFOrientation: Ref;
    static kCGImageSourceCreateThumbnailFromImageAlways: Ref;
    static kCGImageSourceCreateThumbnailWithTransform: Ref;
    static kCGImageSourceThumbnailMaxPixelSize: Ref;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPDFContextCreateWithURL(url: Ref, media_box: *const Rect, info: Ref) -> Ref;
    fn CGPDFContextClose(context: Ref);
    fn CGContextBeginPage(context: Ref, media_box: *const Rect);
    fn CGContextEndPage(context: Ref);
    fn CGContextDrawImage(context: Ref, rect: Rect, image: Ref);
    fn CGContextSaveGState(context: Ref);
    fn CGContextRestoreGState(context: Ref);
    fn CGContextConcatCTM(context: Ref, transform: Affine);
    fn CGPDFDocumentCreateWithURL(url: Ref) -> Ref;
    fn CGPDFDocumentGetNumberOfPages(document: Ref) -> usize;
    fn CGPDFDocumentGetPage(document: Ref, number: usize) -> Ref;
    fn CGPDFPageGetBoxRect(page: Ref, kind: i32) -> Rect;
    fn CGPDFPageGetRotationAngle(page: Ref) -> i32;
    fn CGPDFPageGetDrawingTransform(
        page: Ref,
        kind: i32,
        rect: Rect,
        rotate: i32,
        preserve_aspect: bool,
    ) -> Affine;
    fn CGContextDrawPDFPage(context: Ref, page: Ref);
}

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
    static AVMediaTypeVideo: &'static NSString;
    static AVFileTypeQuickTimeMovie: &'static NSString;
    static AVFileTypeMPEG4: &'static NSString;
    static AVFileTypeAppleM4V: &'static NSString;
    static AVAssetExportPresetPassthrough: &'static NSString;
}

#[link(name = "Vision", kind = "framework")]
unsafe extern "C" {
    static VNErrorDomain: &'static NSString;
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTCreateCGImageFromCVPixelBuffer(buffer: *mut c_void, options: Ref, image: *mut Ref) -> i32;
}

const CROP_BOX: i32 = 1;

struct Owned(Ref);

impl Owned {
    fn new(cf: Ref) -> Option<Owned> {
        (!cf.is_null()).then_some(Owned(cf))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

fn cf<T: ?Sized>(object: &T) -> Ref {
    object as *const T as Ref
}

fn key(name: Ref) -> &'static NSString {
    unsafe { &*(name as *const NSString) }
}

fn dict(pairs: &[(Ref, Retained<AnyObject>)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<&NSString> = pairs.iter().map(|(name, _)| key(*name)).collect();
    let values: Vec<&AnyObject> = pairs.iter().map(|(_, value)| &**value).collect();
    NSDictionary::from_slices(&keys, &values)
}

fn object(number: Retained<NSNumber>) -> Retained<AnyObject> {
    Retained::into_super(Retained::into_super(Retained::into_super(number)))
}

fn number(value: f64) -> Retained<AnyObject> {
    object(NSNumber::new_f64(value))
}

fn yes() -> Retained<AnyObject> {
    object(NSNumber::new_bool(true))
}

fn unreadable(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("The file “{}” couldn’t be opened.", name_of(path)),
    )
}

struct Picture {
    source: Owned,
    width: usize,
    height: usize,
    orientation: i64,
    properties: Option<Retained<NSDictionary<NSString, AnyObject>>>,
}

impl Picture {
    fn open(path: &Path) -> io::Result<Picture> {
        let source =
            Owned::new(unsafe { CGImageSourceCreateWithURL(cf(&*url(path)), std::ptr::null()) })
                .filter(|source| unsafe {
                    !CGImageSourceGetType(source.0).is_null() && CGImageSourceGetCount(source.0) > 0
                })
                .ok_or_else(|| unreadable(path))?;
        let properties = Owned::new(unsafe {
            CGImageSourceCopyPropertiesAtIndex(source.0, 0, std::ptr::null())
        })
        .map(|owned| {
            let dict: &NSDictionary<NSString, AnyObject> =
                unsafe { &*(owned.0 as *const NSDictionary<NSString, AnyObject>) };
            dict.copy()
        });
        let read = |name: Ref| -> Option<i64> {
            let value = properties.as_ref()?.objectForKey(key(name))?;
            Some(value.downcast_ref::<NSNumber>()?.as_i64())
        };
        let width =
            read(unsafe { kCGImagePropertyPixelWidth }).ok_or_else(|| unreadable(path))? as usize;
        let height =
            read(unsafe { kCGImagePropertyPixelHeight }).ok_or_else(|| unreadable(path))? as usize;
        let orientation = read(unsafe { kCGImagePropertyOrientation }).unwrap_or(1);
        Ok(Picture {
            source,
            width,
            height,
            orientation,
            properties,
        })
    }

    fn is_pdf(&self) -> bool {
        let kind = unsafe { CGImageSourceGetType(self.source.0) };
        !kind.is_null() && key(kind).to_string() == "com.adobe.pdf"
    }

    fn upright(&self) -> (usize, usize) {
        match self.orientation {
            5..=8 => (self.height, self.width),
            _ => (self.width, self.height),
        }
    }

    fn render(&self, longest: usize) -> Option<Owned> {
        let options = dict(&[
            (
                unsafe { kCGImageSourceCreateThumbnailFromImageAlways },
                yes(),
            ),
            (unsafe { kCGImageSourceCreateThumbnailWithTransform }, yes()),
            (
                unsafe { kCGImageSourceThumbnailMaxPixelSize },
                number(longest as f64),
            ),
        ]);
        Owned::new(unsafe { CGImageSourceCreateThumbnailAtIndex(self.source.0, 0, cf(&*options)) })
    }
}

/// EXIF orientation as quarter turns clockwise after an optional mirror.
fn turned(orientation: i64, left: bool) -> i64 {
    const PLAIN: [i64; 4] = [1, 6, 3, 8];
    const MIRRORED: [i64; 4] = [2, 7, 4, 5];
    let (table, at) = match PLAIN.iter().position(|o| *o == orientation) {
        Some(at) => (PLAIN, at),
        None => (
            MIRRORED,
            MIRRORED.iter().position(|o| *o == orientation).unwrap_or(0),
        ),
    };
    table[(at + if left { 3 } else { 1 }) % 4]
}

fn staging(path: &Path) -> PathBuf {
    path.with_file_name(format!(".{}.staged", name_of(path)))
}

/// Unlike a rename, keeps `original`'s tags, labels and other attributes.
fn replace(original: &Path, staged: &Path) -> io::Result<()> {
    NSFileManager::defaultManager()
        .replaceItemAtURL_withItemAtURL_backupItemName_options_resultingItemURL_error(
            &url(original),
            &url(staged),
            None,
            NSFileManagerItemReplacementOptions::empty(),
            None,
        )
        .map_err(|err| {
            let _ = std::fs::remove_file(staged);
            io_error(err)
        })
}

fn rotate(items: Vec<PathBuf>, left: bool) -> io::Result<Done> {
    for item in &items {
        match Picture::open(item) {
            Ok(picture) => rotate_image(item, picture, left)?,
            Err(_) => rotate_movie(item, left)?,
        }
    }
    Ok(Done {
        results: items.clone(),
        undo: Some(Op::Quick(Quick::Rotate { items, left: !left })),
    })
}

fn rotate_image(path: &Path, picture: Picture, left: bool) -> io::Result<()> {
    let staged = staging(path);
    let kind = unsafe { CGImageSourceGetType(picture.source.0) };
    let dest = Owned::new(unsafe {
        CGImageDestinationCreateWithURL(cf(&*url(&staged)), kind, 1, std::ptr::null())
    })
    .ok_or_else(|| unreadable(path))?;
    let options = dict(&[(
        unsafe { kCGImageDestinationOrientation },
        number(turned(picture.orientation, left) as f64),
    )]);
    let mut error: Ref = std::ptr::null();
    if !unsafe {
        CGImageDestinationCopyImageSource(dest.0, picture.source.0, cf(&*options), &mut error)
    } {
        drop(Owned::new(error));
        let _ = std::fs::remove_file(&staged);
        return Err(unreadable(path));
    }
    drop(dest);
    replace(path, &staged)
}

fn movie_type(path: &Path) -> &'static NSString {
    let ext = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase());
    unsafe {
        match ext.as_deref() {
            Some("mp4") => AVFileTypeMPEG4,
            Some("m4v") => AVFileTypeAppleM4V,
            _ => AVFileTypeQuickTimeMovie,
        }
    }
}

fn rotate_movie(path: &Path, left: bool) -> io::Result<()> {
    let class = AnyClass::get(c"AVMutableMovie").ok_or_else(|| unreadable(path))?;
    let file = url(path);
    let mut error: *mut NSError = std::ptr::null_mut();
    let movie: Option<Retained<AnyObject>> = unsafe {
        msg_send![class, movieWithURL: &*file, options: std::ptr::null::<AnyObject>(), error: &mut error]
    };
    let movie = movie.ok_or_else(|| unreadable(path))?;
    let tracks: Retained<NSArray<AnyObject>> =
        unsafe { msg_send![&movie, tracksWithMediaType: AVMediaTypeVideo] };
    if tracks.is_empty() {
        return Err(unreadable(path));
    }
    let angle = if left {
        -std::f64::consts::FRAC_PI_2
    } else {
        std::f64::consts::FRAC_PI_2
    };
    let (sin, cos) = (angle.sin().round(), angle.cos().round());
    for track in tracks.iter() {
        let now: Affine = unsafe { msg_send![&track, preferredTransform] };
        let natural: NSSize = unsafe { msg_send![&track, naturalSize] };
        let mut turned = Affine {
            a: now.a * cos + now.b * -sin,
            b: now.a * sin + now.b * cos,
            c: now.c * cos + now.d * -sin,
            d: now.c * sin + now.d * cos,
            tx: now.tx * cos + now.ty * -sin,
            ty: now.tx * sin + now.ty * cos,
        };
        let corners = [
            (0., 0.),
            (natural.width, 0.),
            (0., natural.height),
            (natural.width, natural.height),
        ]
        .map(|(x, y)| {
            (
                turned.a * x + turned.c * y + turned.tx,
                turned.b * x + turned.d * y + turned.ty,
            )
        });
        turned.tx -= corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
        turned.ty -= corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
        let _: () = unsafe { msg_send![&track, setPreferredTransform: turned] };
    }
    let mut error: *mut NSError = std::ptr::null_mut();
    let wrote: Bool = unsafe {
        msg_send![&movie, writeMovieHeaderToURL: &*file, fileType: movie_type(path), options: 0usize, error: &mut error]
    };
    if wrote.as_bool() {
        Ok(())
    } else {
        Err(unsafe { error.as_ref() }
            .map_or_else(|| unreadable(path), |err| io_error(err.retain())))
    }
}

fn beside(first: &Path, name: String) -> PathBuf {
    free_path(&first.with_file_name(name))
}

fn stem(path: &Path) -> String {
    split_name(&name_of(path)).0.to_string()
}

fn create_pdf(items: Vec<PathBuf>) -> io::Result<Done> {
    let first = items
        .first()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let dest = beside(first, format!("{}.pdf", stem(first)));
    let context = Owned::new(unsafe {
        CGPDFContextCreateWithURL(cf(&*url(&dest)), std::ptr::null(), std::ptr::null())
    })
    .ok_or_else(|| unreadable(&dest))?;
    let drawn = draw_pages(context.0, &items);
    unsafe { CGPDFContextClose(context.0) };
    drop(context);
    if let Err(err) = drawn {
        let _ = std::fs::remove_file(&dest);
        return Err(err);
    }
    // Finder's undo of Create PDF leaves the document where it is.
    Ok(Done {
        results: vec![dest],
        undo: Some(Op::Batch(Vec::new())),
    })
}

fn draw_pages(context: Ref, items: &[PathBuf]) -> io::Result<()> {
    for item in items {
        let picture = Picture::open(item).ok().filter(|picture| !picture.is_pdf());
        let Some(picture) = picture else {
            let document = Owned::new(unsafe { CGPDFDocumentCreateWithURL(cf(&*url(item))) })
                .ok_or_else(|| unreadable(item))?;
            for number in 1..=unsafe { CGPDFDocumentGetNumberOfPages(document.0) } {
                let page = unsafe { CGPDFDocumentGetPage(document.0, number) };
                let crop = unsafe { CGPDFPageGetBoxRect(page, CROP_BOX) };
                let (w, h) = match unsafe { CGPDFPageGetRotationAngle(page) }.rem_euclid(180) {
                    90 => (crop.h, crop.w),
                    _ => (crop.w, crop.h),
                };
                let media = Rect { x: 0., y: 0., w, h };
                unsafe {
                    CGContextBeginPage(context, &media);
                    CGContextSaveGState(context);
                    CGContextConcatCTM(
                        context,
                        CGPDFPageGetDrawingTransform(page, CROP_BOX, media, 0, true),
                    );
                    CGContextDrawPDFPage(context, page);
                    CGContextRestoreGState(context);
                    CGContextEndPage(context);
                }
            }
            continue;
        };
        let (w, h) = picture.upright();
        let image = picture.render(w.max(h)).ok_or_else(|| unreadable(item))?;
        let media = Rect {
            x: 0.,
            y: 0.,
            w: w as f64,
            h: h as f64,
        };
        unsafe {
            CGContextBeginPage(context, &media);
            CGContextDrawImage(context, media, image.0);
            CGContextEndPage(context);
        }
    }
    Ok(())
}

fn write_image(dest: &Path, format: ImageFormat, image: Ref, properties: Ref) -> io::Result<()> {
    let kind = NSString::from_str(format.uti());
    let out = Owned::new(unsafe {
        CGImageDestinationCreateWithURL(cf(&*url(dest)), cf(&*kind), 1, std::ptr::null())
    })
    .ok_or_else(|| unreadable(dest))?;
    unsafe { CGImageDestinationAddImage(out.0, image, properties) };
    if unsafe { CGImageDestinationFinalize(out.0) } {
        return Ok(());
    }
    let _ = std::fs::remove_file(dest);
    Err(unreadable(dest))
}

fn convert(
    items: Vec<PathBuf>,
    format: ImageFormat,
    size: ImageSize,
    metadata: bool,
) -> io::Result<Done> {
    let mut results = Vec::with_capacity(items.len());
    for item in &items {
        let picture = Picture::open(item)?;
        let name = match size {
            ImageSize::Actual => format!("{}.{}", stem(item), format.ext()),
            _ => format!("{} {}.{}", stem(item), size.title(), format.ext()),
        };
        let dest = beside(item, name);
        match (size.longest(), metadata) {
            (None, true) => {
                let kind = NSString::from_str(format.uti());
                let out = Owned::new(unsafe {
                    CGImageDestinationCreateWithURL(
                        cf(&*url(&dest)),
                        cf(&*kind),
                        1,
                        std::ptr::null(),
                    )
                })
                .ok_or_else(|| unreadable(&dest))?;
                let quality = match format {
                    ImageFormat::Png => dict(&[]),
                    _ => dict(&[(
                        unsafe { kCGImageDestinationLossyCompressionQuality },
                        number(0.8),
                    )]),
                };
                unsafe {
                    CGImageDestinationAddImageFromSource(out.0, picture.source.0, 0, cf(&*quality))
                };
                if !unsafe { CGImageDestinationFinalize(out.0) } {
                    let _ = std::fs::remove_file(&dest);
                    return Err(unreadable(item));
                }
            }
            (longest, metadata) => {
                let (w, h) = picture.upright();
                let side = longest.map_or(w.max(h), |longest| longest.min(w.max(h)));
                let image = picture.render(side).ok_or_else(|| unreadable(item))?;
                let source = picture.properties.as_deref().filter(|_| metadata);
                let properties = upright_properties(source);
                write_image(&dest, format, image.0, cf(&*properties))?;
            }
        }
        results.push(dest);
    }
    Ok(Done {
        undo: Some(Op::DeleteImmediately(results.clone())),
        results,
    })
}

fn upright_properties(
    source: Option<&NSDictionary<NSString, AnyObject>>,
) -> Retained<NSDictionary<NSString, AnyObject>> {
    let (orientation, tiff, tiff_orientation) = unsafe {
        (
            kCGImagePropertyOrientation,
            kCGImagePropertyTIFFDictionary,
            kCGImagePropertyTIFFOrientation,
        )
    };
    let without = |dict: Option<&NSDictionary<NSString, AnyObject>>, skipped: &[Ref]| {
        let (names, values) = dict.map(|dict| dict.to_vecs()).unwrap_or_default();
        names
            .into_iter()
            .zip(values)
            .filter(|(name, _)| !skipped.iter().any(|skip| **name == *key(*skip)))
            .map(|(name, value)| (cf(&*name), value))
            .collect::<Vec<(Ref, Retained<AnyObject>)>>()
    };
    let source_tiff = source
        .and_then(|source| source.objectForKey(key(tiff)))
        .and_then(|tiff| tiff.downcast::<NSDictionary>().ok())
        .map(|tiff| unsafe { Retained::cast_unchecked::<NSDictionary<NSString, AnyObject>>(tiff) });
    let upright = number(1.);
    let mut tiff_pairs = without(source_tiff.as_deref(), &[tiff_orientation]);
    tiff_pairs.push((tiff_orientation, upright.clone()));
    let mut pairs = without(source, &[orientation, tiff]);
    pairs.push((
        tiff,
        Retained::into_super(Retained::into_super(dict(&tiff_pairs))),
    ));
    pairs.push((orientation, upright));
    dict(&pairs)
}

fn remove_background(items: Vec<PathBuf>) -> io::Result<Done> {
    let mut results = Vec::with_capacity(items.len());
    for item in &items {
        let picture = Picture::open(item)?;
        let image = cut_out(item, picture.orientation).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Failed to remove background")
        })?;
        let dest = beside(item, format!("{} Background Removed.png", stem(item)));
        write_image(&dest, ImageFormat::Png, image.0, std::ptr::null())?;
        results.push(dest);
    }
    Ok(Done {
        undo: Some(Op::DeleteImmediately(results.clone())),
        results,
    })
}

fn cut_out(path: &Path, orientation: i64) -> Option<Owned> {
    // Touching a Vision symbol links the framework, so its classes register.
    let _ = unsafe { VNErrorDomain };
    let handler_class = AnyClass::get(c"VNImageRequestHandler")?;
    let request_class = AnyClass::get(c"VNGenerateForegroundInstanceMaskRequest")?;
    let file = url(path);
    let empty = NSDictionary::<NSString, AnyObject>::new();
    let handler: Retained<AnyObject> = unsafe {
        let made: Allocated<AnyObject> = msg_send![handler_class, alloc];
        msg_send![made, initWithURL: &*file, orientation: orientation as u32, options: &*empty]
    };
    let request: Retained<AnyObject> = unsafe { msg_send![request_class, new] };
    let requests = NSArray::from_retained_slice(std::slice::from_ref(&request));
    let mut error: *mut NSError = std::ptr::null_mut();
    let ran: Bool = unsafe { msg_send![&handler, performRequests: &*requests, error: &mut error] };
    if !ran.as_bool() {
        return None;
    }
    let results: Option<Retained<NSArray<AnyObject>>> = unsafe { msg_send![&request, results] };
    let observation = results?.firstObject()?;
    let instances: Retained<AnyObject> = unsafe { msg_send![&observation, allInstances] };
    let count: usize = unsafe { msg_send![&instances, count] };
    if count == 0 {
        return None;
    }
    let buffer: *mut c_void = unsafe {
        msg_send![&observation, generateMaskedImageOfInstances: &*instances, fromRequestHandler: &*handler, croppedToInstancesExtent: false, error: &mut error]
    };
    if buffer.is_null() {
        return None;
    }
    let mut image: Ref = std::ptr::null();
    let status = unsafe { VTCreateCGImageFromCVPixelBuffer(buffer, std::ptr::null(), &mut image) };
    unsafe { CFRelease(buffer) };
    (status == 0).then(|| Owned::new(image)).flatten()
}

const EXPORT_COMPLETED: isize = 3;

/// Finder keeps no undo for a trim.
fn trim(item: &Path, start: f64, end: f64, into: &Path, tags: &[String]) -> io::Result<Done> {
    let (Some(assets), Some(exports)) = (
        AnyClass::get(c"AVURLAsset"),
        AnyClass::get(c"AVAssetExportSession"),
    ) else {
        return Err(unreadable(item));
    };
    let asset: Option<Retained<AnyObject>> = unsafe {
        msg_send![assets, URLAssetWithURL: &*url(item), options: std::ptr::null::<AnyObject>()]
    };
    let asset = asset.ok_or_else(|| unreadable(item))?;
    let length: Time = unsafe { msg_send![&asset, duration] };
    let scale = length.scale.max(600);
    let session: Option<Retained<AnyObject>> = unsafe {
        let made: Allocated<AnyObject> = msg_send![exports, alloc];
        msg_send![made, initWithAsset: &*asset, presetName: AVAssetExportPresetPassthrough]
    };
    let session = session.ok_or_else(|| unreadable(item))?;
    let staged = staging(into);
    let _ = std::fs::remove_file(&staged);
    let range = unsafe {
        TimeRange {
            start: CMTimeMakeWithSeconds(start, scale),
            duration: CMTimeMakeWithSeconds(end - start, scale),
        }
    };
    let (finished, wait) = mpsc::channel();
    let handler = RcBlock::new(move || {
        let _ = finished.send(());
    });
    unsafe {
        let _: () = msg_send![&session, setOutputURL: &*url(&staged)];
        let _: () = msg_send![&session, setOutputFileType: movie_type(into)];
        let _: () = msg_send![&session, setTimeRange: range];
        let _: () = msg_send![&session, exportAsynchronouslyWithCompletionHandler: &*handler];
    }
    let _ = wait.recv();
    let status: isize = unsafe { msg_send![&session, status] };
    if status != EXPORT_COMPLETED {
        let _ = std::fs::remove_file(&staged);
        let error: Option<Retained<NSError>> = unsafe { msg_send![&session, error] };
        return Err(error.map_or_else(|| unreadable(item), io_error));
    }
    if into == item {
        replace(item, &staged)?;
    } else {
        if let Err(err) = std::fs::rename(&staged, into) {
            let _ = std::fs::remove_file(&staged);
            return Err(err);
        }
        if !tags.is_empty() {
            let tags: Vec<Tag> = tags
                .iter()
                .map(|name| Tag {
                    name: name.clone(),
                    color: 0,
                })
                .collect();
            crate::tags::set(into, &tags)?;
        }
    }
    Ok(Done {
        results: vec![into.to_path_buf()],
        undo: None,
    })
}

#[cfg(test)]
mod tests {
    use super::turned;

    #[test]
    fn a_full_turn_either_way_comes_back() {
        for start in 1..=8 {
            let mut left = start;
            let mut right = start;
            for _ in 0..4 {
                left = turned(left, true);
                right = turned(right, false);
            }
            assert_eq!((left, right), (start, start));
            assert_eq!(turned(turned(start, true), false), start);
        }
        assert_eq!(turned(1, true), 8);
        assert_eq!(turned(8, true), 3);
        assert_eq!(turned(1, false), 6);
    }
}
