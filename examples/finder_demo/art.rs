use std::sync::Arc;

use gpui::{
    AnyElement, Div, FontWeight, Hsla, ObjectFit, RenderImage, SharedString, div, img, prelude::*,
    px,
};
use native_gpui::native_anchor;
use native_gpui::tokens::{
    Appearance, FileType, SymbolColor, SystemColor, current_appearance, sf_symbol_colored,
};

use crate::data::{Face, Row};
use crate::thumbs::thumb;

pub(crate) fn row_art(row: &Row, size: f32, scale: f32, visible: bool) -> AnyElement {
    match thumb(row, size, scale, visible, true) {
        Some(image) => picture(image, size),
        None => artwork(row, size),
    }
}

/// A list row previews pictures and movies only; every other view previews documents too.
pub(crate) fn list_art(row: &Row, size: f32, scale: f32) -> AnyElement {
    match row.face {
        Face::Image | Face::Movie => row_art(row, size, scale, true),
        _ => artwork(row, size),
    }
}

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
            .child(type_art(poster_kind(row), row.face, width.min(height)))
            .into_any_element(),
    }
}

/// Finder badges an alias's icon, never its preview: a poster draws what the alias points at.
fn poster_kind(row: &Row) -> Option<FileType<'static>> {
    match (row.uti == "public.symlink")
        .then(|| std::fs::canonicalize(row.path.as_ref()).ok())
        .flatten()
    {
        Some(target) => Some(FileType::Path(crate::data::intern_path(&target))),
        None => kind_of(row),
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
        .or_else(|| crate::icons::icon(face.stand_in(), size))
        .map(|symbol| symbol.img().into_any_element())
        .unwrap_or_else(|| glyph(face, size))
}

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
    let (w, h) = (
        u32::from(fit.width) as f32 / scale,
        u32::from(fit.height) as f32 / scale,
    );
    let zoom = (width / w.max(1.)).min(height / h.max(1.));
    let (w, h) = (px(w * zoom), px(h * zoom));
    let framed = match kind {
        Poster::Hero => img(image)
            .w(w)
            .h(h)
            .rounded(px(6.5))
            .border(px(0.5))
            .border_color(Hsla {
                h: 0.,
                s: 0.,
                l: 1.,
                a: 0.1,
            })
            .into_any_element(),
        // The rim sits outside the picture and darkens the glass behind it.
        Poster::Card => div()
            .w(w)
            .h(h)
            .rounded(px(4.5))
            .border(px(0.5))
            .border_color(Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: match current_appearance() {
                    Appearance::Dark => 0.1,
                    Appearance::Light => 0.02,
                },
            })
            .child(img(image).size_full().rounded(px(4.)))
            .into_any_element(),
    };
    slot(width, height).child(framed).into_any_element()
}

pub(crate) type Anchor = native_gpui::SurfaceAnchor;

pub(crate) fn art_at(art: AnyElement, size: f32, anchor: Option<Anchor>) -> AnyElement {
    let Some(anchor) = anchor else {
        return art;
    };
    div()
        .flex_none()
        .w(px(size))
        .h(px(size))
        .child(native_anchor(&anchor, art))
        .into_any_element()
}

pub(crate) fn artwork(row: &Row, size: f32) -> AnyElement {
    square(size)
        .child(type_art(kind_of(row), row.face, size))
        .into_any_element()
}

fn kind_of(row: &Row) -> Option<FileType<'static>> {
    folder_kind(row, crate::custom::custom_icon)
}

/// Off the main thread, knowing a folder's own icon before its row is drawn.
pub(crate) fn kind_now(row: &Row) -> Option<FileType<'static>> {
    folder_kind(row, crate::custom::custom_icon_now)
}

fn folder_kind(row: &Row, custom: fn(&SharedString) -> bool) -> Option<FileType<'static>> {
    match row.artwork {
        Some(FileType::Folder | FileType::FullFolder | FileType::FullPath(_))
            if custom(&row.path) =>
        {
            Some(FileType::Path(crate::data::intern(&row.path)))
        }
        kind => kind,
    }
}

/// `public.volume` has no artwork of its own, so a volume draws `/`'s.
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
