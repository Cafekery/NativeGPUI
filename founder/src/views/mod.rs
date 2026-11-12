pub(crate) mod column;
pub(crate) mod gallery;
pub(crate) mod header;
pub(crate) mod icon;
pub(crate) mod list;

use gpui::{
    AnyElement, Div, ExternalPaths, FontWeight, Hsla, SharedString, Stateful, WeakEntity, div,
    prelude::*, px,
};
use native_gpui::tokens::SystemColor;

use crate::app::FinderDemo;
use crate::art::small_glyph;
use crate::dragdrop::{Lit, RowDrag, drop_target};

fn run_pill(fill: Hsla, inset: f32, radius: f32, (above, below): (bool, bool)) -> Div {
    let corner = |joined: bool| px(if joined { 0. } else { radius });
    div()
        .absolute()
        .left(px(inset))
        .right(px(inset))
        .top_0()
        .bottom_0()
        .rounded_t(corner(above))
        .rounded_b(corner(below))
        .bg(fill)
}

fn chevron(symbol: &str, size: f32, weight: FontWeight, lit: bool) -> Option<AnyElement> {
    let color = if lit {
        SystemColor::AlternateSelectedControlText
    } else {
        SystemColor::SecondaryLabel
    };
    small_glyph(symbol, size, weight, color)
}

fn folder_drop(
    row: Stateful<Div>,
    key: &SharedString,
    into: &'static str,
    band: Option<(f32, f32)>,
    this: WeakEntity<FinderDemo>,
) -> Stateful<Div> {
    let row = drop_target::<RowDrag>(row, key.clone(), into, band, Lit::Drawn, this.clone());
    drop_target::<ExternalPaths>(row, key.clone(), into, band, Lit::Drawn, this)
}
