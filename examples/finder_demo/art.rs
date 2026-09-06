//! What a row draws: its thumbnail where Quick Look has one, its type's icon otherwise.

use std::sync::Arc;

use gpui::{
    AnyElement, Bounds, FontWeight, Hsla, ObjectFit, Pixels, RenderImage, div, img, prelude::*, px,
};
use native_gpui::tokens::{SymbolColor, SystemColor, file_icon, sf_symbol_colored};

use crate::data::{Face, Row};
use crate::thumbs::thumb;

/// Finder's row icon: the file's own rendered thumbnail at this size where Quick Look draws one,
/// its type's icon until that lands. Only a row on screen asks for one.
pub(crate) fn row_art(row: &Row, size: f32, scale: f32, visible: bool) -> AnyElement {
    match thumb(row, size, scale, visible, true) {
        Some(image) => picture(image, size),
        None => artwork(row, size),
    }
}

/// Finder's large preview art: the picture at its own aspect, keylined and rounded on that rect
/// rather than on the box it is centred in, and without an icon's inset, radius or shadow.
pub(crate) fn poster_art(row: &Row, size: f32, scale: f32, kind: Poster) -> AnyElement {
    match thumb(row, size, scale, true, false) {
        Some(image) => poster(image, size, scale, kind),
        None => artwork(row, size),
    }
}

/// Finder's Quick Look icon: the whole picture fit inside the box, centred, with the keyline,
/// corner radius and shadow Quick Look's icon mode baked into the raster.
fn picture(image: Arc<RenderImage>, size: f32) -> AnyElement {
    div()
        .w(px(size))
        .h(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            img(image)
                .max_w(px(size))
                .max_h(px(size))
                .object_fit(ObjectFit::Contain),
        )
        .into_any_element()
}

/// Which preview a poster is: Finder strokes a gallery hero with a light keyline and a wider
/// radius, a preview card with a dark one.
#[derive(Clone, Copy)]
pub(crate) enum Poster {
    Hero,
    Card,
}

fn poster(image: Arc<RenderImage>, size: f32, scale: f32, kind: Poster) -> AnyElement {
    let fit = image.size(0);
    let (radius, keyline) = match kind {
        Poster::Hero => (
            6.5,
            Hsla {
                h: 0.,
                s: 0.,
                l: 1.,
                a: 0.1,
            },
        ),
        Poster::Card => (
            2.5,
            Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.24,
            },
        ),
    };
    div()
        .w(px(size))
        .h(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            img(image)
                .w(px((u32::from(fit.width) as f32 / scale).min(size)))
                .h(px((u32::from(fit.height) as f32 / scale).min(size)))
                .rounded(px(radius))
                .border(px(0.5))
                .border_color(keyline),
        )
        .into_any_element()
}

/// Where an icon landed on screen, for the Quick Look panel to zoom out of and back into.
pub(crate) type Anchor = std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>;

/// `art` as drawn, recording where it lands when its row is the one Quick Look would open.
pub(crate) fn art_at(art: AnyElement, size: f32, anchor: Option<Anchor>) -> AnyElement {
    let Some(anchor) = anchor else {
        return art;
    };
    div()
        .relative()
        .flex_none()
        .w(px(size))
        .h(px(size))
        .child(art)
        .child(
            gpui::canvas(
                move |bounds, _, _| anchor.set(Some(bounds)),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .into_any_element()
}

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

pub(crate) fn small_glyph(
    name: &str,
    size: f32,
    weight: FontWeight,
    color: SystemColor,
) -> Option<AnyElement> {
    sf_symbol_colored(name, px(size), weight, &SymbolColor::Hierarchical(color))
        .map(|symbol| symbol.img().into_any_element())
}
