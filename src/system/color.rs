use gpui::{Hsla, Rgba};
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSColorSpace};

/// It must be `colorWithSRGBRed:` — `Hsla` is sRGB-relative and `colorWithRed:` is *device* RGB,
/// which on a P3 display is a different colour from the same three numbers.
pub fn to_nscolor(color: Hsla) -> Retained<NSColor> {
    let rgba = Rgba::from(color);
    NSColor::colorWithSRGBRed_green_blue_alpha(
        rgba.r as f64,
        rgba.g as f64,
        rgba.b as f64,
        rgba.a as f64,
    )
}

/// Convert any `NSColor` to a gpui [`Hsla`], `None` if it refuses sRGB conversion. Reads the
/// ambient drawing appearance, which matches one appearance and silently contradicts the other,
/// so for a semantic colour use `SystemColor::try_resolve` instead.
///
/// That `try_resolve` genuinely answers per appearance rather than reading the ambient one twice
/// is checked by the storybook's tokens page, which lists the rows where
/// `try_resolve(Light) != try_resolve(Dark)`. It is the only live control on this, so a pass that
/// removes it leaves nothing behind that can fail.
///
/// The `Option` must not be collapsed to a default: `redComponent` and friends are undefined
/// outside an RGB colour space, so the conversion is mandatory, and it returns nil for pattern
/// colours — which would hand the caller a black indistinguishable from a deliberate one.
pub fn from_nscolor(color: &NSColor) -> Option<Hsla> {
    let srgb = color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    Some(Hsla::from(Rgba {
        r: srgb.redComponent() as f32,
        g: srgb.greenComponent() as f32,
        b: srgb.blueComponent() as f32,
        a: srgb.alphaComponent() as f32,
    }))
}
