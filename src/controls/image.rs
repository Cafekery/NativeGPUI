//! A stock AppKit image hosted above GPUI, for the bands whose material sits over the Metal layer.

use gpui::{App, ElementId, FontWeight, Style, Window, px};
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSControl, NSImage, NSImageScaling, NSImageSymbolConfiguration, NSImageSymbolScale,
    NSImageView, NSResponder, NSView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSString};

use crate::bridge::host::{Layer, Slot, hosted_element};
use crate::system::tokens::{SystemColor, ns_font_weight};

/// An AppKit named image (`NSImage imageNamed:`) drawn at its own size, tinted like a template.
pub fn native_image(id: impl Into<ElementId>, name: &str) -> NativeImage {
    NativeImage {
        id: id.into(),
        source: Source::Named(name.to_string()),
        tint: SystemColor::Label,
    }
}

/// An SF Symbol at a point size and weight, hosted like [`native_image`].
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

enum Source {
    Named(String),
    Symbol {
        name: String,
        size: f32,
        weight: FontWeight,
    },
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
        // A 1 px stroke straddling a device pixel draws at half weight: keep the origin whole.
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
