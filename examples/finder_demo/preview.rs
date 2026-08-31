//! The preview pane: the info card beside the file list.
//!
//! Owns `FinderDemo::info_card` and the pane's width, which includes the divider on its leading
//! edge because the list's remaining width is measured from it.
//!
//! Deliberately not here: the gallery view's large preview, which is a different thing in a
//! different place.

use gpui::{AnyElement, FontWeight, div, prelude::*, px};
use native_gpui::tokens;

use crate::FinderDemo;
use crate::data::Line;
use crate::palette::Palette;
use crate::ui::{TAGS, glyph, tag_dot};

/// The right pane including the 1pt divider on its leading edge, which is what leaves Finder 594pt
/// of list in a 1026pt window. Get it wrong and every column lands somewhere else.
pub(crate) const PREVIEW_W: f32 = 240.;

impl FinderDemo {
    pub(crate) fn info_card(
        &self,
        line: Option<&Line>,
        palette: &Palette,
        size: f32,
    ) -> AnyElement {
        let mut card = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .text_color(palette.secondary);
        // Measured: with nothing selected Finder's preview pane is a uniform fill and carries no
        // label, so an empty div is the match rather than a "No selection" caption.
        let Some(line) = line else {
            return div().into_any_element();
        };

        card = card
            .child(glyph(line.row.face, size))
            .child(
                div()
                    .max_w(px(240.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.label)
                    .child(line.row.name.clone()),
            )
            .child(
                div()
                    .text_size(tokens::small_system_font_size())
                    .child(line.row.face.kind()),
            );

        for (caption, value) in [
            ("Modified", line.row.modified.clone()),
            ("Size", line.row.size.clone()),
        ] {
            card = card.child(
                div()
                    .flex()
                    .gap(px(6.))
                    .text_size(tokens::small_system_font_size())
                    .child(div().text_color(palette.tertiary).child(caption))
                    .child(div().text_color(palette.secondary).child(value)),
            );
        }
        if let Some(tag) = self.tags.get(&line.key) {
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_size(tokens::small_system_font_size())
                    .child(tag_dot(*tag))
                    .child(div().text_color(palette.secondary).child(TAGS[*tag].0)),
            );
        }
        card.into_any_element()
    }
}
