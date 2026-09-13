use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{App, ElementId, FontWeight, ImageId, Style, Window, px};
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{AllocAnyThread, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBitmapFormat, NSControl, NSImage, NSImageScaling, NSImageSymbolConfiguration,
    NSImageSymbolScale, NSImageView, NSResponder, NSView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};

use crate::bridge::host::{Layer, Slot, hosted_element};
use crate::system::tokens::{Symbol, SystemColor, bitmap, ns_font_weight};

const RASTERS_KEPT: usize = 256;

thread_local! {
    static RASTERS: RefCell<HashMap<ImageId, Retained<NSImage>>> = RefCell::new(HashMap::new());
}

pub fn native_image(id: impl Into<ElementId>, name: &str) -> NativeImage {
    NativeImage {
        id: id.into(),
        source: Source::Named(name.to_string()),
        tint: SystemColor::Label,
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
        source: Source::Symbol {
            name: name.to_string(),
            size,
            weight,
        },
        tint: SystemColor::Label,
    }
}

pub fn native_raster(id: impl Into<ElementId>, raster: Symbol) -> NativeImage {
    NativeImage {
        id: id.into(),
        source: Source::Raster(raster),
        tint: SystemColor::Label,
    }
}

enum Source {
    Named(String),
    Symbol {
        name: String,
        size: f32,
        weight: FontWeight,
    },
    Raster(Symbol),
}

pub struct NativeImage {
    id: ElementId,
    source: Source,
    tint: SystemColor,
}

impl NativeImage {
    pub fn tint(mut self, tint: SystemColor) -> Self {
        self.tint = tint;
        self
    }

    fn image(&self) -> Option<Retained<NSImage>> {
        match &self.source {
            Source::Named(name) => NSImage::imageNamed(&NSString::from_str(name)),
            Source::Symbol { name, size, weight } => {
                let base = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(name),
                    None,
                )?;
                let configuration =
                    NSImageSymbolConfiguration::configurationWithPointSize_weight_scale(
                        f64::from(*size),
                        ns_font_weight(*weight),
                        NSImageSymbolScale::Medium,
                    );
                base.imageWithSymbolConfiguration(&configuration)
            }
            Source::Raster(raster) => RASTERS.with(|kept| {
                let mut kept = kept.borrow_mut();
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
        let size = self.image().map(|image| image.size()).unwrap_or_default();
        let mut style = Style::default();
        style.size.width = px(size.width as f32).into();
        style.size.height = px(size.height as f32).into();
        style
    }

    fn paint(&mut self, mut slot: Slot, window: &mut Window, _: &mut App) {
        let scale = window.scale_factor();
        let snap = |v: gpui::Pixels| px((f32::from(v) * scale).round() / scale);
        slot.bounds.origin.x = snap(slot.bounds.origin.x);
        slot.bounds.origin.y = snap(slot.bounds.origin.y);
        let view = slot.place(Layer::Above, |mtm, _| Glyph::new(mtm));
        let image = self.image();
        let same = match (view.image(), &image) {
            (Some(have), Some(want)) => have.isEqual(Some(&**want)),
            (None, None) => true,
            _ => false,
        };
        if !same {
            view.setImage(image.as_deref());
        }
        let tint = self.tint.ns_color();
        if !view
            .contentTintColor()
            .is_some_and(|have| have.isEqual(Some(&**tint)))
        {
            view.setContentTintColor(Some(&tint));
        }
    }
}

hosted_element!(NativeImage);

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
