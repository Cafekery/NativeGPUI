use std::sync::LazyLock;

use gpui::{AnyElement, Hsla, Pixels, div, hsla, prelude::*};
use native_gpui::{Blending, Layer, Material, material, native_rule};
use objc2_foundation::NSProcessInfo;

static MACOS_27: LazyLock<bool> = LazyLock::new(|| {
    NSProcessInfo::processInfo()
        .operatingSystemVersion()
        .majorVersion
        >= 27
});

/// macOS 27's Finder lifts its bars off the material by a tint no `NSColor` names; 26's do not.
const BAR_TINT: Hsla = hsla(0., 0., 1., 0.038);

pub(crate) fn toolbar_rule() -> bool {
    !*MACOS_27
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
        .when(*MACOS_27, |band| {
            band.child(
                div()
                    .absolute()
                    .inset_0()
                    .child(native_rule("ng.finder.band.tint").tint(BAR_TINT)),
            )
        })
        .into_any_element()
}
