//! The window's two materials: the pane's glass over the rows, and the toolbar's band.

use gpui::{AnyElement, Hsla, Pixels, div, hsla, prelude::*, px};
use native_gpui::{Blending, Layer, Material, glass, material, native_rule};

/// Finder's bars are the window's material lifted by a tint no `NSColor` names: white at this
/// alpha over the dark material, and nothing at all over the light one.
pub(crate) const BAR_TINT: Hsla = hsla(0., 0., 1., 0.038);

/// The pane's glass: it starts a radius left of the pane, so only its window-side corners round.
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

/// One material over the toolbar, scope and header bands, running on under the floating sidebar.
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
        // Over the material: a within-window material all but swallows a tint painted under it.
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .child(native_rule("ng.finder.band.tint").tint(BAR_TINT)),
        )
        .into_any_element()
}
