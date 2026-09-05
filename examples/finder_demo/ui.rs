//! Drawing helpers every view shares, and the tag colours they draw with.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::{
    AnyElement, ClickEvent, FontWeight, Hsla, SharedString, WeakEntity, Window, div, img,
    prelude::*, px,
};

use native_gpui::thumbnails;
use native_gpui::tokens::{SymbolColor, SystemColor, file_icon, sf_symbol_colored, system_color};
pub(crate) use native_gpui::{focus_gpui, native_text_field};
use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;

use crate::data::{Face, Line, Row};
use crate::{FinderDemo, ns_window};

pub(crate) const TAGS: &[(&str, SystemColor)] = &[
    ("Red", SystemColor::SystemRed),
    ("Orange", SystemColor::SystemOrange),
    ("Yellow", SystemColor::SystemYellow),
    ("Green", SystemColor::SystemGreen),
    ("Blue", SystemColor::SystemBlue),
    ("Purple", SystemColor::SystemPurple),
    ("Gray", SystemColor::SystemGray),
];

/// The demo's tag colours are Finder's, in Finder's order; these are Finder's colour indices.
const TAG_COLORS: [u8; 7] = [6, 7, 5, 2, 4, 3, 1];

/// One of the demo's tags as Finder stores it.
pub(crate) fn finder_tag(index: usize) -> finder_fs::tags::Tag {
    finder_fs::tags::Tag {
        name: TAGS[index].0.to_string(),
        color: TAG_COLORS[index],
    }
}

/// Thumbnails land on a background queue; the count tells a window when to draw again.
static LANDED: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn thumbnails_landed() -> usize {
    LANDED.load(Ordering::Relaxed)
}

fn screen_scale() -> f32 {
    static SCALE: OnceLock<f32> = OnceLock::new();
    *SCALE.get_or_init(|| {
        let Some(mtm) = MainThreadMarker::new() else {
            return 2.;
        };
        NSScreen::screens(mtm)
            .iter()
            .map(|screen| screen.backingScaleFactor() as f32)
            .fold(1., f32::max)
    })
}

/// Finder's picture of a file where Quick Look has one, and its type's icon until that arrives.
pub(crate) fn thumb_art(row: &Row, size: f32) -> AnyElement {
    if !matches!(row.face, Face::Image | Face::Movie | Face::Pdf) {
        return artwork(row, size);
    }
    let path = Path::new(row.path.as_ref());
    let scale = screen_scale();
    let Some(image) = thumbnails::cached_thumbnail(path, px(size), scale) else {
        ask(row, size, scale);
        return artwork(row, size);
    };
    div()
        .w(px(size))
        .h(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(img(image).max_w(px(size)).max_h(px(size)))
        .into_any_element()
}

/// One request per file, size and moment: the module coalesces the work already in flight, but a
/// waiter a frame would pile up behind it.
fn ask(row: &Row, size: f32, scale: f32) {
    thread_local! {
        static ASKED: RefCell<HashSet<(SharedString, u32, i64)>> = RefCell::new(HashSet::new());
    }
    let key = (row.path.clone(), size as u32, row.modified_at as i64);
    if ASKED.with(|asked| !asked.borrow_mut().insert(key)) {
        return;
    }
    thumbnails::thumbnail(Path::new(row.path.as_ref()), px(size), scale, |_| {
        LANDED.fetch_add(1, Ordering::Relaxed);
    });
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

pub(crate) fn tag_pill(index: usize, text: Hsla) -> AnyElement {
    let (name, color) = TAGS[index];
    let mut wash = system_color(color);
    wash.a = 0.55;
    div()
        .px(px(6.))
        .rounded(px(4.))
        .bg(wash)
        .text_color(text)
        .child(name)
        .into_any_element()
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

/// `(extend, toggle, twice)`: Shift, Cmd and a double click.
pub(crate) fn click_kind(event: &ClickEvent) -> (bool, bool, bool) {
    match event {
        ClickEvent::Mouse(mouse) => (
            mouse.up.modifiers.shift,
            mouse.up.modifiers.platform,
            mouse.up.click_count >= 2,
        ),
        _ => (false, false, false),
    }
}

pub(crate) fn rename_field(
    line: &Line,
    at: usize,
    this: WeakEntity<FinderDemo>,
) -> impl IntoElement {
    let name = line.row.name.clone();
    let dot = name.rfind('.').filter(|&dot| dot > 0).unwrap_or(name.len());
    let stem = name[..dot].chars().count();
    let key = line.key.clone();
    let leave = this.clone();
    native_text_field(("ng.finder.rename", at), &name)
        .stretch()
        .editing(0..stem)
        .on_submit(move |text, window, cx| {
            let text = text.clone();
            this.update(cx, |this: &mut FinderDemo, cx| {
                this.rename(&key, &text, cx);
                this.renaming = None;
                focus_gpui(window);
                key_to_parent(window);
                cx.notify();
            })
            .ok();
        })
        .on_cancel(move |_, window, cx| {
            leave
                .update(cx, |this: &mut FinderDemo, cx| {
                    this.renaming = None;
                    focus_gpui(window);
                    key_to_parent(window);
                    cx.notify();
                })
                .ok();
        })
}

/// Hand key back to the main window after a field in the pane's panel took it.
pub(crate) fn key_to_parent(window: &Window) {
    if let Some(parent) = ns_window(window).and_then(|panel| panel.parentWindow()) {
        parent.makeKeyWindow();
    }
}

/// A scroll container that ignores the other axis; nested with a list, each axis then has one owner.
pub(crate) fn axis_locked<E: Styled>(mut element: E) -> E {
    element.style().restrict_scroll_to_axis = Some(true);
    element
}
