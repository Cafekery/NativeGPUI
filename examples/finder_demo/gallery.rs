//! The gallery view: one large preview above a strip of thumbnails.
//!
//! Owns `FinderDemo::gallery_body`.
//!
//! Deliberately not here: the info pane beside it, which is `preview`, and the tile metrics of the
//! icon view, which this arrangement does not use.

use gpui::{AnyElement, ClickEvent, div, prelude::*, px};

use crate::FinderDemo;
use crate::palette::Palette;
use crate::ui::artwork;

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
            // The hero is the item itself, not the inspector: `main` already renders the trailing
            // preview pane in gallery view, so showing `info_card` here put the same name, kind
            // and metadata on screen twice. Finder's gallery is a large preview centred on the
            // stage with the inspector beside it.
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(24.))
                    .children(cursor.as_ref().map(|line| artwork(&line.row, 256.))),
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
                        // 64pt, not the 42 that was here: the artwork ships representations at
                        // 16/32/64/128pt and a size between two of them is resampled, which shows
                        // up as a soft thumbnail long before anyone thinks to blame the size.
                        let mut cell = div()
                            .id(("ng.finder.frame", index))
                            .w(px(72.))
                            .h(px(72.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .child(artwork(&line.row, 64.));
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
