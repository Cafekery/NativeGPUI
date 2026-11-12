use std::sync::Arc;

use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, ObjectFit, RenderImage, SharedString, div, hsla,
    img, prelude::*, px, white,
};
use native_gpui::native_anchor;
use native_gpui::tokens::{
    Appearance, FileType, Symbol, SymbolColor, SystemColor, current_appearance, sf_symbol_colored,
};

use crate::app::FinderDemo;
use crate::data::{Face, Row};
use crate::prefs;
use crate::thumbs::thumb;
use crate::view::{COLUMN_VIEW, GALLERY_VIEW, ICON_VIEW, LIST_VIEW};
use crate::views::column::OPTIONS as COLUMN_OPTIONS;

const PREVIEWS: &str = "showIconPreview";
const COLUMN_PREVIEWS: &str = "ShowIconThumbnails";
const HIDDEN_ART: f32 = 0.51;
const HIDDEN_PILL: f32 = 0.65;

pub(crate) fn hidden_name(ink: Hsla) -> Hsla {
    ink.opacity(match current_appearance() {
        Appearance::Dark => 0.52,
        Appearance::Light => 0.36,
    })
}

// A translucent pill under translucent text composites wrongly on the light window.
pub(crate) fn hidden_pill(fill: Hsla) -> Hsla {
    let fill = fill.opacity(HIDDEN_PILL);
    match current_appearance() {
        Appearance::Dark => fill,
        Appearance::Light => white().blend(fill),
    }
}

pub(crate) struct Previews {
    pub(crate) list: bool,
    pub(crate) column: bool,
    pub(crate) gallery: bool,
}

impl Previews {
    pub(crate) fn of(folder: &str) -> Self {
        let look = |view, settings| {
            prefs::look_number(view, folder, PREVIEWS)
                .or_else(|| prefs::finder_look_number(settings, PREVIEWS))
                .is_none_or(|on| on != 0.)
        };
        Self {
            list: look(LIST_VIEW, "ExtendedListViewSettingsV2"),
            column: prefs::view_option_on(COLUMN_OPTIONS, COLUMN_PREVIEWS).unwrap_or(true),
            gallery: look(GALLERY_VIEW, "GalleryViewSettings"),
        }
    }
}

impl FinderDemo {
    pub(crate) fn icon_previews(&self) -> bool {
        match self.view {
            ICON_VIEW => self.icon_paint().previews,
            LIST_VIEW => self.previews.list,
            COLUMN_VIEW => self.previews.column,
            _ => self.previews.gallery,
        }
    }

    pub(crate) fn set_icon_previews(&mut self, on: bool, cx: &mut Context<Self>) {
        let here = self.here();
        match self.view {
            ICON_VIEW => {
                return self.change_icon_options(|icon| icon.show_icon_preview = Some(on), cx);
            }
            COLUMN_VIEW => prefs::set_view_option_on(COLUMN_OPTIONS, COLUMN_PREVIEWS, on),
            view => prefs::set_look_on(view, here, PREVIEWS, on),
        }
        self.previews = Previews::of(here);
        cx.notify();
    }
}

pub(crate) fn item_art(row: &Row, size: f32, scale: f32, previews: bool) -> AnyElement {
    if previews {
        row_art(row, size, scale, true)
    } else {
        artwork(row, size)
    }
}

pub(crate) fn row_art(row: &Row, size: f32, scale: f32, visible: bool) -> AnyElement {
    match thumb(row, size, scale, visible, true) {
        Some(image) => picture(image, size),
        None => artwork(row, size),
    }
}

#[derive(Clone)]
pub(crate) enum Art {
    Picture(Arc<RenderImage>),
    Icon(Symbol),
    Glyph(Face),
}

impl PartialEq for Art {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Art::Picture(a), Art::Picture(b)) => Arc::ptr_eq(a, b),
            (Art::Icon(a), Art::Icon(b)) => Arc::ptr_eq(&a.image, &b.image) && a.size == b.size,
            (Art::Glyph(a), Art::Glyph(b)) => a == b,
            _ => false,
        }
    }
}

impl Art {
    pub(crate) fn element(&self, size: f32) -> AnyElement {
        match self {
            Art::Picture(image) => picture(image.clone(), size),
            Art::Icon(symbol) => square(size).child(symbol.img()).into_any_element(),
            Art::Glyph(face) => square(size).child(glyph(*face, size)).into_any_element(),
        }
    }
}

pub(crate) fn list_art(row: &Row, size: f32, scale: f32, previews: bool) -> Art {
    (previews && matches!(row.face, Face::Image | Face::Movie))
        .then(|| thumb(row, size, scale, true, true))
        .flatten()
        .map(Art::Picture)
        .or_else(|| type_symbol(kind_of(row), row.face, size).map(Art::Icon))
        .unwrap_or(Art::Glyph(row.face))
}

pub(crate) fn poster_art(
    row: &Row,
    width: f32,
    height: f32,
    scale: f32,
    kind: Poster,
) -> AnyElement {
    let image = thumb(row, width.max(height), scale, true, false);
    picture_art(row, image, width, height, scale, kind)
}

pub(crate) fn picture_art(
    row: &Row,
    image: Option<Arc<RenderImage>>,
    width: f32,
    height: f32,
    scale: f32,
    kind: Poster,
) -> AnyElement {
    match image {
        Some(image) => poster(image, width, height, scale, kind),
        None => slot(width, height)
            .child(type_art(poster_kind(row), row.face, width.min(height)))
            .into_any_element(),
    }
}

/// Only an alias's icon wears the badge: its poster draws what it points at.
fn poster_kind(row: &Row) -> Option<FileType<'static>> {
    (row.uti == "public.symlink" && !crate::remote::is_remote(&row.path))
        .then(|| std::fs::canonicalize(row.path.as_ref()).ok())
        .flatten()
        .map(|target| FileType::Path(crate::data::intern_path(&target)))
        .or_else(|| kind_of(row))
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

fn type_symbol(kind: Option<FileType<'static>>, face: Face, size: f32) -> Option<Symbol> {
    kind.and_then(|kind| crate::icons::icon(kind, size))
        .or_else(|| crate::icons::icon(face.stand_in(), size))
}

fn type_art(kind: Option<FileType<'static>>, face: Face, size: f32) -> AnyElement {
    type_symbol(kind, face, size)
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
            .border_color(hsla(0., 0., 1., 0.1))
            .into_any_element(),
        // The rim sits outside the picture and darkens the glass behind it.
        Poster::Card => div()
            .w(w)
            .h(h)
            .rounded(px(4.5))
            .border(px(0.5))
            .border_color(hsla(
                0.,
                0.,
                0.,
                match current_appearance() {
                    Appearance::Dark => 0.1,
                    Appearance::Light => 0.02,
                },
            ))
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
        .size(px(size))
        .child(native_anchor(&anchor, art))
        .into_any_element()
}

pub(crate) fn marked(row: &Row, size: f32, art: AnyElement) -> AnyElement {
    if !row.hidden && !row.locked {
        return art;
    }
    square(size)
        .relative()
        .when(row.hidden, |art| art.opacity(HIDDEN_ART))
        .child(art)
        .when(row.locked, |art| {
            art.child(div().absolute().left_0().bottom_0().children(small_glyph(
                "lock.fill",
                (size / 3.).max(8.),
                FontWeight::NORMAL,
                SystemColor::Label,
            )))
        })
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

pub(crate) fn kind_now(row: &Row) -> Option<FileType<'static>> {
    folder_kind(row, crate::custom::custom_icon_now)
}

fn has_folder_art(row: &Row) -> bool {
    matches!(
        row.artwork,
        Some(FileType::Folder | FileType::FullFolder | FileType::FullPath(_))
    )
}

pub(crate) fn custom_folder(row: &Row) -> bool {
    has_folder_art(row) && crate::custom::custom_icon(&row.path)
}

fn folder_kind(row: &Row, custom: fn(&SharedString) -> bool) -> Option<FileType<'static>> {
    if has_folder_art(row) && custom(&row.path) {
        Some(FileType::Path(crate::data::intern(&row.path)))
    } else {
        row.artwork
    }
}

pub(crate) fn glyph(face: Face, size: f32) -> AnyElement {
    slot(size + 4., size)
        .children(
            sf_symbol_colored(face.symbol(), px(size), FontWeight::NORMAL, &face.tint())
                .map(|symbol| symbol.img()),
        )
        .into_any_element()
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
