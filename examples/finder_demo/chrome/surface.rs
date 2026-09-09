use gpui::{AnyElement, Hsla, Pixels, div, hsla, prelude::*, px};
use native_gpui::{Blending, Layer, Material, glass, material, native_rule};
use objc2_foundation::NSProcessInfo;

/// macOS 27's Finder lifts its bars off the window material by a tint no `NSColor` names; macOS
/// 26's bars are the material itself.
pub(crate) fn bar_tint() -> Option<Hsla> {
    (NSProcessInfo::processInfo()
        .operatingSystemVersion()
        .majorVersion
        >= 27)
        .then(|| hsla(0., 0., 1., 0.038))
}

/// macOS 26's Finder rules a column or gallery view off the toolbar; macOS 27's does not.
pub(crate) fn toolbar_rule() -> bool {
    NSProcessInfo::processInfo()
        .operatingSystemVersion()
        .majorVersion
        < 27
}

pub(crate) fn pane_surface(width: Pixels, corner: Pixels) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .right_0()
        .bottom_0()
        .w(width)
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .left(-corner)
                .child(
                    glass("ng.finder.pane")
                        .frosting_gpui()
                        .corner_radius(corner),
                ),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .bottom_0()
                .w(px(1.))
                .child(native_rule("ng.finder.pane.edge")),
        )
        .into_any_element()
}

pub(crate) fn band_glass(right: Pixels, height: Pixels) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right(right)
        .h(height)
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
