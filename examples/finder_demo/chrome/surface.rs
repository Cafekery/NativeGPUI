//! The window's two materials: the pane's glass over the rows, and the toolbar's band.

use gpui::{AnyElement, Pixels, div, prelude::*, px};
use native_gpui::{Blending, Layer, Material, glass, material, native_rule};

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
        .into_any_element()
}
