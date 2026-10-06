use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{App, ElementId, FontWeight, Hsla, ImageId, Style, Window, px, size};
use objc2::rc::Retained;
use objc2::{AllocAnyThread, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBitmapFormat, NSControl, NSImage, NSImageScaling, NSImageSymbolConfiguration,
    NSImageSymbolScale, NSImageView, NSResponder, NSView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};

use crate::bridge::host::{Layer, Slot, hosted_element};
use crate::controls::{snap_origin, symbol_image};
use crate::system::color::to_nscolor;
use crate::system::tokens::{Symbol, SystemColor, bitmap, ns_font_weight};

const RASTERS_KEPT: usize = 256;

type SymbolKey = (String, u32, u32);

thread_local! {
    static RASTERS: RefCell<HashMap<ImageId, Retained<NSImage>>> = RefCell::new(HashMap::new());
    static SYMBOLS: RefCell<HashMap<SymbolKey, Option<Retained<NSImage>>>> =
        RefCell::new(HashMap::new());
}

pub fn native_image(id: impl Into<ElementId>, name: &str) -> NativeImage {
    NativeImage {
        id: id.into(),
        source: Source::Named(name.to_string()),
        tint: Tint::System(SystemColor::Label),
    }
}

pub fn native_symbol(
    id: impl Into<ElementId>,
    name: &str,
    size: f32,
    weight: FontWeight,
) -> NativeImage {
    NativeImage {
        id: id.into(),
        source: Source::Symbol((name.to_string(), size.to_bits(), weight.0.to_bits())),
        tint: Tint::System(SystemColor::Label),
    }
}

pub fn native_raster(id: impl Into<ElementId>, raster: Symbol) -> NativeImage {
    NativeImage {
        id: id.into(),
        source: Source::Raster(raster),
        tint: Tint::System(SystemColor::Label),
    }
}

enum Source {
    Named(String),
    Symbol(SymbolKey),
    Raster(Symbol),
}

#[derive(Clone, Copy)]
enum Tint {
    System(SystemColor),
    Fixed(Hsla),
    Untinted,
}

pub struct NativeImage {
    id: ElementId,
    source: Source,
    tint: Tint,
}

impl NativeImage {
    pub fn tint(mut self, tint: SystemColor) -> Self {
        self.tint = Tint::System(tint);
        self
    }

    /// A fixed colour in place of a system tint.
    pub fn ink(mut self, ink: impl Into<Hsla>) -> Self {
        self.tint = Tint::Fixed(ink.into());
        self
    }

    /// AppKit inks the template itself, fading it while the window is not key.
    pub fn untinted(mut self) -> Self {
        self.tint = Tint::Untinted;
        self
    }

    fn image(&self) -> Option<Retained<NSImage>> {
        match &self.source {
            Source::Named(name) => NSImage::imageNamed(&NSString::from_str(name)),
            // A configured symbol is a new NSImage each time, and NSImage equality is identity.
            Source::Symbol(key) => SYMBOLS.with_borrow_mut(|kept| {
                if let Some(hit) = kept.get(key) {
                    return hit.clone();
                }
                let image = configured_symbol(key);
                kept.insert(key.clone(), image.clone());
                image
            }),
            Source::Raster(raster) => RASTERS.with_borrow_mut(|kept| {
                if let Some(hit) = kept.get(&raster.image.id) {
                    return Some(hit.clone());
                }
                let image = ns_image(raster)?;
                if kept.len() >= RASTERS_KEPT {
                    kept.clear();
                }
                kept.insert(raster.image.id, image.clone());
                Some(image)
            }),
        }
    }

    fn layout(&self) -> Style {
        let fitting = self.image().map(|image| image.size()).unwrap_or_default();
        Style {
            size: size(
                px(fitting.width as f32).into(),
                px(fitting.height as f32).into(),
            ),
            ..Default::default()
        }
    }

    fn paint(&mut self, mut slot: Slot, window: &mut Window, _: &mut App) {
        snap_origin(&mut slot, window.scale_factor());
        let view = slot.place(Layer::Above, |mtm, _| Glyph::new(mtm));
        let image = self.image();
        if view.image() != image {
            view.setImage(image.as_deref());
        }
        let tint = match self.tint {
            Tint::System(color) => Some(color.ns_color()),
            Tint::Fixed(color) => Some(to_nscolor(color)),
            Tint::Untinted => None,
        };
        if view.contentTintColor() != tint {
            view.setContentTintColor(tint.as_deref());
        }
    }
}

hosted_element!(NativeImage);

fn configured_symbol((name, size, weight): &SymbolKey) -> Option<Retained<NSImage>> {
    let configuration = NSImageSymbolConfiguration::configurationWithPointSize_weight_scale(
        f64::from(f32::from_bits(*size)),
        ns_font_weight(FontWeight(f32::from_bits(*weight))),
        NSImageSymbolScale::Medium,
    );
    symbol_image(name, None)?.imageWithSymbolConfiguration(&configuration)
}

fn ns_image(raster: &Symbol) -> Option<Retained<NSImage>> {
    let pixels = raster.image.size(0);
    let (w, h) = (pixels.width.0 as usize, pixels.height.0 as usize);
    let bgra = raster.image.as_bytes(0)?;
    let rep = bitmap(w, h, NSBitmapFormat::AlphaNonpremultiplied)?;
    let data = rep.bitmapData();
    if data.is_null() || bgra.len() < w * h * 4 {
        return None;
    }
    let rgba = unsafe { std::slice::from_raw_parts_mut(data, w * h * 4) };
    for (to, from) in rgba.chunks_exact_mut(4).zip(bgra.chunks_exact(4)) {
        to.copy_from_slice(&[from[2], from[1], from[0], from[3]]);
    }
    let points = NSSize::new(
        f64::from(f32::from(raster.size.width)),
        f64::from(f32::from(raster.size.height)),
    );
    rep.setSize(points);
    let image = NSImage::initWithSize(NSImage::alloc(), points);
    image.addRepresentation(&rep);
    Some(image)
}

define_class!(
    #[unsafe(super(NSImageView, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGGlyph"]
    struct Glyph;

    impl Glyph {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }
    }
);

impl Glyph {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this: Retained<Self> =
            unsafe { msg_send![Self::alloc(mtm), initWithFrame: NSRect::ZERO] };
        this.setImageScaling(NSImageScaling::ScaleNone);
        this
    }
}
