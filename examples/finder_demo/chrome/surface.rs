use gpui::{AnyElement, Hsla, Pixels, div, hsla, prelude::*};
use native_gpui::{Blending, Layer, Material, material, native_rule};
use objc2_foundation::NSProcessInfo;

/// macOS 27's Finder lifts its bars off the material by a tint no `NSColor` names; 26's do not.
pub(crate) fn bar_tint() -> Option<Hsla> {
    (NSProcessInfo::processInfo()
        .operatingSystemVersion()
        .majorVersion
        >= 27)
        .then(|| hsla(0., 0., 1., 0.038))
}

pub(crate) fn toolbar_rule() -> bool {
    NSProcessInfo::processInfo()
        .operatingSystemVersion()
        .majorVersion
        < 27
}

pub(crate) fn band_glass(height: Pixels) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(height)
        .block_mouse_except_scroll()
        .child(
            material("ng.finder.band", Material::Header)
                .layer(Layer::Above)
                // At the default BehindWindow this samples what is behind the window, not the rows.
                .blending(Blending::WithinWindow)
                .size_full(),
        )
        // A within-window material all but swallows a tint painted under it.
        .when_some(bar_tint(), |band, tint| {
            band.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .child(native_rule("ng.finder.band.tint").tint(tint)),
            )
        })
        .into_any_element()
}
