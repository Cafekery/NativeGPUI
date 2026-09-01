//! Drawing helpers every view shares, and the tag colours they draw with.
//!
//! Owns [`artwork`] and [`glyph`] (a row's icon, real or symbol), [`small_glyph`] (the little
//! chevrons and sort arrows) and [`tag_dot`], plus the `TAGS` list all four of the views and the
//! Action menu index into.
//!
//! Deliberately not here: colour resolution (that is `palette`), layout constants (those live with
//! the view that measured them), and anything that touches `FinderDemo` state.

use gpui::{AnyElement, FontWeight, div, prelude::*, px};
use native_gpui::tokens::{SymbolColor, SystemColor, file_icon, sf_symbol_colored, system_color};

use crate::data::{Face, Row};

/// Finder's tag colours, in its own order. Seven, ending in Gray: it is the last dot in the strip
/// the Action menu shows, and a six-dot strip is visibly one short of Finder's.
pub(crate) const TAGS: &[(&str, SystemColor)] = &[
    ("Red", SystemColor::SystemRed),
    ("Orange", SystemColor::SystemOrange),
    ("Yellow", SystemColor::SystemYellow),
    ("Green", SystemColor::SystemGreen),
    ("Blue", SystemColor::SystemBlue),
    ("Purple", SystemColor::SystemPurple),
    ("Gray", SystemColor::SystemGray),
];

/// A row's icon: the system's own artwork for the row's type, falling back to the face's SF
/// symbol for the props that have no file type behind them. Both the list and the icon view draw
/// through here, so one row cannot show real artwork in one view and a symbol in the other.
///
/// 32pt at a 2x backing scale lands on the 64px representation the artwork actually ships, so the
/// icon is crisp rather than resampled — asking for a size off that ladder is what makes one look
/// soft. The size is the same for every row on purpose: the cache is keyed by it, and varying it
/// per row would turn a hit into a 42µs raster.
///
/// The box is square whichever path answers. A symbol needs the wider slot [`glyph`] gives it, and
/// handing that slot back to a caller that asked for `size` shifts the icon off the column the
/// artwork rows line up on.
pub(crate) fn artwork(row: &Row, size: f32) -> AnyElement {
    let art = row
        .artwork
        .and_then(|kind| file_icon(kind, px(size)))
        .map(|symbol| symbol.img().into_any_element())
        .unwrap_or_else(|| glyph(row.face, size));
    div()
        .w(px(size))
        .h(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(art)
        .into_any_element()
}

pub(crate) fn glyph(face: Face, size: f32) -> AnyElement {
    let symbol = sf_symbol_colored(face.symbol(), px(size), FontWeight::NORMAL, &face.tint());
    let mut slot = div()
        .w(px(size + 4.))
        .h(px(size))
        .flex()
        .items_center()
        .justify_center()
        .flex_none();
    if let Some(symbol) = symbol {
        slot = slot.child(symbol.img());
    }
    slot.into_any_element()
}

pub(crate) fn small_glyph(name: &str, size: f32, color: SystemColor) -> Option<AnyElement> {
    sf_symbol_colored(
        name,
        px(size),
        FontWeight::SEMIBOLD,
        &SymbolColor::Hierarchical(color),
    )
    .map(|symbol| symbol.img().into_any_element())
}

pub(crate) fn tag_dot(index: usize) -> AnyElement {
    div()
        .w(px(8.))
        .h(px(8.))
        .rounded_full()
        .flex_none()
        .bg(system_color(TAGS[index].1))
        .into_any_element()
}
