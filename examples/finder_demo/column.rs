//! The column (browser) view.
//!
//! Owns `FinderDemo::column_body`, the single-column `FinderDemo::browser_column` it is built out
//! of, and the compact row height that view kept.
//!
//! Deliberately not here: the list view's row metrics. This view was never measured against a real
//! window the way the list was, and it does not share the list's constants.

use gpui::{AnyElement, ClickEvent, div, prelude::*, px};
use native_gpui::tokens::SystemColor;

use crate::FinderDemo;
use crate::data::Line;
use crate::palette::Palette;
use crate::ui::{glyph, small_glyph};

/// The browser (column) view keeps the compact row it always had: only the list view was measured.
pub(crate) const BROWSER_ROW_H: f32 = 21.;

impl FinderDemo {
    /// One column of the browser view: a flat listing that reports clicks back through `pick`.
    pub(crate) fn browser_column(
        &self,
        id: &'static str,
        lines: Vec<Line>,
        palette: &Palette,
        width: Option<f32>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity().downgrade();
        let selected = self.selected.clone();
        let label = palette.label;
        let selection = palette.selection;
        let selection_text = palette.selection_text;
        let mut column = div()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(palette.separator)
            .id(id)
            .overflow_y_scroll();
        column = match width {
            Some(width) => column.w(px(width)).flex_none(),
            None => column.flex_1(),
        };

        column
            .children(lines.into_iter().enumerate().map(|(index, line)| {
                let is_selected = selected.contains(&line.key);
                let this = this.clone();
                let clicked = line.clone();
                let mut row = div()
                    .id((id, index))
                    .h(px(BROWSER_ROW_H))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .px(px(8.))
                    .text_color(if is_selected { selection_text } else { label })
                    .child(glyph(line.row.face, 16.))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(line.row.name.clone()),
                    );
                if line.row.opens.is_some() {
                    if let Some(arrow) = small_glyph(
                        "chevron.right",
                        8.,
                        if is_selected {
                            SystemColor::AlternateSelectedControlText
                        } else {
                            SystemColor::TertiaryLabel
                        },
                    ) {
                        row = row.child(arrow);
                    }
                }
                if is_selected {
                    row = row.bg(selection);
                }
                row.on_click(move |event: &ClickEvent, window, cx| {
                    let twice =
                        matches!(event, ClickEvent::Mouse(mouse) if mouse.up.click_count >= 2);
                    let line = clicked.clone();
                    this.update(cx, |this: &mut FinderDemo, cx| {
                        this.pick(&line, false, false);
                        if twice {
                            this.open(&line, window, cx);
                        }
                        cx.notify();
                    })
                    .ok();
                })
            }))
            .into_any_element()
    }

    pub(crate) fn column_body(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let lines = self.lines(false);
        let cursor = self
            .cursor
            .as_ref()
            .and_then(|key| lines.iter().find(|line| &line.key == key))
            .cloned();
        let child = cursor.as_ref().and_then(|line| line.row.opens);
        let child_lines: Vec<Line> = match child {
            Some(id) => {
                let mut out = Vec::new();
                self.collect(id, 0, false, &mut out);
                out
            }
            None => Vec::new(),
        };

        let mut body = div()
            .flex_1()
            .flex()
            .overflow_hidden()
            .child(self.browser_column("ng.finder.col1", lines, palette, Some(260.), cx));
        if child.is_some() {
            body = body.child(self.browser_column(
                "ng.finder.col2",
                child_lines,
                palette,
                Some(260.),
                cx,
            ));
        }
        // Finder's preview is the column after the last listing, not a panel floating in the
        // leftover width, and the remainder stays empty.
        body.child(
            div()
                .w(px(300.))
                .flex_none()
                .flex()
                .justify_center()
                .pt(px(28.))
                .px(px(20.))
                .border_r_1()
                .border_color(palette.separator)
                .child(self.info_card(cursor.as_ref(), palette, 96.)),
        )
        .child(div().flex_1())
        .into_any_element()
    }
}
