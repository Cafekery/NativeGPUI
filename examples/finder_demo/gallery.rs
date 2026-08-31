//! The gallery view: one large preview above a strip of thumbnails.
//!
//! Owns `FinderDemo::gallery_body`.
//!
//! Deliberately not here: the info pane beside it, which is `preview`, and the tile metrics of the
//! icon view, which this arrangement does not use.

use gpui::{AnyElement, ClickEvent, div, prelude::*, px};

use crate::FinderDemo;
use crate::palette::Palette;
use crate::ui::glyph;

impl FinderDemo {
    pub(crate) fn gallery_body(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let lines = self.lines(false);
        let cursor = self
            .cursor
            .as_ref()
            .and_then(|key| lines.iter().find(|line| &line.key == key))
            .cloned()
            .or_else(|| lines.first().cloned());
        let this = cx.entity().downgrade();
        let selected = cursor.as_ref().map(|line| line.key.clone());
        let selection = palette.selection;

        div()
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(24.))
                    .child(self.info_card(cursor.as_ref(), palette, 144.)),
            )
            .child(
                div()
                    .id("ng.finder.filmstrip")
                    .h(px(96.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(14.))
                    .border_t_1()
                    .border_color(palette.separator)
                    .overflow_x_scroll()
                    .children(lines.into_iter().enumerate().map(|(index, line)| {
                        let is_selected = selected.as_ref() == Some(&line.key);
                        let this = this.clone();
                        let clicked = line.clone();
                        let mut cell = div()
                            .id(("ng.finder.frame", index))
                            .w(px(64.))
                            .h(px(64.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .child(glyph(line.row.face, 42.));
                        if is_selected {
                            cell = cell.bg(selection);
                        }
                        cell.on_click(move |_: &ClickEvent, _, cx| {
                            let line = clicked.clone();
                            this.update(cx, |this: &mut FinderDemo, cx| {
                                this.pick(&line, false, false);
                                cx.notify();
                            })
                            .ok();
                        })
                    })),
            )
            .into_any_element()
    }
}
