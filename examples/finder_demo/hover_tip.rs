//! The folder-size tooltip a list row shows on hover: the gallery strip's own name bubble,
//! anchored over a row instead of a filmstrip cell.

use std::path::Path;
use std::time::Duration;

use gpui::{
    AnyElement, Hsla, Path as VectorPath, SharedString, canvas, div, point, prelude::*, px,
};
use native_gpui::tokens::{self, Appearance};

pub(crate) const ROW_TIP_DELAY: Duration = Duration::from_millis(800);

const TIP_H: f32 = 25.;
const TIP_PAD: f32 = 12.;
const TIP_RADIUS: f32 = 10.;
const TIP_FONT: f32 = 13.;
const TIP_ARROW: f32 = 10.;

/// "3.2 GB — 1,204 items", once the index knows both; `None` while it is still finding out.
pub(crate) fn folder_summary(folder: &str) -> Option<SharedString> {
    let path = Path::new(folder);
    let index = crate::index();
    let size = index.folder_size(path)?;
    let count = index.children(path)?.len();
    Some(SharedString::from(format!(
        "{} — {} item{}",
        crate::data::byte_string(size),
        grouped(count),
        if count == 1 { "" } else { "s" },
    )))
}

fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Finder's own tooltip bezel: a rounded bubble on an arrow, pointing at `left` from `bottom`.
pub(crate) fn row_tooltip(text: SharedString, left: f32, bottom: f32, label: Hsla) -> AnyElement {
    let dark = tokens::current_appearance() == Appearance::Dark;
    let fill = if dark {
        gpui::rgb(0x242424)
    } else {
        gpui::rgb(0xf6f6f6)
    };
    let border = if dark {
        gpui::rgba(0xffffff4d)
    } else {
        gpui::rgba(0x0000002e)
    };
    let arrow = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let top = bounds.origin.y;
            let left = bounds.origin.x;
            let w = bounds.size.width;
            let h = bounds.size.height;
            let mut outer = VectorPath::new(point(left, top));
            outer.line_to(point(left + w / 2., top + h));
            outer.line_to(point(left + w, top));
            window.paint_path(outer, border);
            let mut inner = VectorPath::new(point(left + px(1.4), top - px(1.)));
            inner.line_to(point(left + w / 2., top + h - px(1.4)));
            inner.line_to(point(left + w - px(1.4), top - px(1.)));
            window.paint_path(inner, fill);
        },
    )
    .w(px(2. * TIP_ARROW))
    .h(px(TIP_ARROW))
    .flex_none();
    div()
        .absolute()
        .left(px(left))
        .bottom(px(bottom))
        .w(px(0.))
        .flex()
        .flex_col()
        .items_center()
        .child(
            div()
                .flex_none()
                .h(px(TIP_H))
                .px(px(TIP_PAD))
                .rounded(px(TIP_RADIUS))
                .bg(fill)
                .border_1()
                .border_color(border)
                .flex()
                .items_center()
                .whitespace_nowrap()
                .font_family(tokens::system_font_family())
                .text_size(px(TIP_FONT))
                .text_color(label)
                .child(text),
        )
        .child(arrow)
        .into_any_element()
}
