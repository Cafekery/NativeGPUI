//! What a row draws: its thumbnail where Quick Look has one, its type's icon otherwise.

use std::sync::Arc;

use gpui::{
    AnyElement, Bounds, Div, FontWeight, Hsla, ObjectFit, Pixels, RenderImage, div, img,
    prelude::*, px,
};
use native_gpui::tokens::{FileType, SymbolColor, SystemColor, sf_symbol_colored};

use crate::data::{Face, Row};
use crate::thumbs::thumb;

/// Finder's row icon: Quick Look's thumbnail where there is one, the type's icon until it lands.
pub(crate) fn row_art(row: &Row, size: f32, scale: f32, visible: bool) -> AnyElement {
    match thumb(row, size, scale, visible, true) {
        Some(image) => picture(image, size),
        None => artwork(row, size),
    }
}

/// Finder's large preview art: the picture at its own aspect, keylined on its rect, not the box.
pub(crate) fn poster_art(
    row: &Row,
    width: f32,
    height: f32,
    scale: f32,
    kind: Poster,
) -> AnyElement {
    match thumb(row, width.max(height), scale, true, false) {
        Some(image) => poster(image, width, height, scale, kind),
        None => slot(width, height)
            .child(type_art(row.artwork, row.face, width.min(height)))
            .into_any_element(),
    }
}

pub(crate) fn square(side: f32) -> Div {
    slot(side, side)
}

fn slot(width: f32, height: f32) -> Div {
    div()
        .w(px(width))
        .h(px(height))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
}

fn type_art(kind: Option<FileType<'static>>, face: Face, size: f32) -> AnyElement {
    kind.and_then(|kind| crate::icons::icon(kind, size))
        .map(|symbol| symbol.img().into_any_element())
        .unwrap_or_else(|| glyph(face, size))
}

/// Quick Look's icon mode bakes the keyline, corner radius and shadow into the raster.
fn picture(image: Arc<RenderImage>, size: f32) -> AnyElement {
    square(size)
        .child(
            img(image)
                .max_w(px(size))
                .max_h(px(size))
                .object_fit(ObjectFit::Contain),
        )
        .into_any_element()
}

/// Which preview a poster is: a hero is keylined light and rounded wider than a card.
#[derive(Clone, Copy)]
pub(crate) enum Poster {
    Hero,
    Card,
}

fn poster(
    image: Arc<RenderImage>,
    width: f32,
    height: f32,
    scale: f32,
    kind: Poster,
) -> AnyElement {
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
    let (w, h) = (
        u32::from(fit.width) as f32 / scale,
        u32::from(fit.height) as f32 / scale,
    );
    // The raster may have been drawn for another width: it wears this box's fit, not its own.
    let zoom = (width / w.max(1.)).min(height / h.max(1.));
    slot(width, height)
        .child(
            img(image)
                .w(px(w * zoom))
                .h(px(h * zoom))
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
    square(size)
        .child(type_art(row.artwork, row.face, size))
        .into_any_element()
}

/// The artwork a folder wears in the chrome: `public.volume` has none, so a volume draws `/`'s.
pub(crate) fn place_art(volume: bool, size: f32) -> AnyElement {
    let (kind, face) = match volume {
        true => (FileType::Path("/"), Face::Volume),
        false => (FileType::Folder, Face::Folder),
    };
    square(size)
        .child(type_art(Some(kind), face, size))
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
