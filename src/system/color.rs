//! Conversion between `NSColor` and gpui's `Hsla`.

use gpui::{Hsla, Rgba};
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSColorSpace};

// `colorWithSRGBRed:`, not `colorWithRed:`, which is device RGB.
pub fn to_nscolor(color: Hsla) -> Retained<NSColor> {
    let rgba = Rgba::from(color);
    NSColor::colorWithSRGBRed_green_blue_alpha(
        rgba.r as f64,
        rgba.g as f64,
        rgba.b as f64,
        rgba.a as f64,
    )
}

// `colorUsingColorSpace:` is nil for pattern colours, whose components are undefined.
pub fn from_nscolor(color: &NSColor) -> Option<Hsla> {
    let srgb = color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    Some(Hsla::from(Rgba {
        r: srgb.redComponent() as f32,
        g: srgb.greenComponent() as f32,
        b: srgb.blueComponent() as f32,
        a: srgb.alphaComponent() as f32,
    }))
}
