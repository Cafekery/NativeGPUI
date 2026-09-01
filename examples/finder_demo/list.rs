//! The list view: rows, banding, the selection pill and the disclosure triangles.
//!
//! Owns `FinderDemo::list_body` and the row metrics — icon size, row height, the three x offsets
//! measured from the pane's leading edge, the pill radius.
//!
//! Deliberately not here: the column widths and cell padding, which `header` owns and this module
//! imports so the two cannot drift; the header band itself; and the other three view modes.

use std::collections::HashSet;

use gpui::{AnyElement, ClickEvent, FontWeight, SharedString, div, prelude::*, px, uniform_list};
use native_gpui::tokens::{self, SystemColor};

use crate::FinderDemo;
use crate::data::Column;
use crate::header::{CELL_PAD, CELL_TRAIL, PANE_INSET};
use crate::palette::Palette;
use crate::ui::{artwork, small_glyph, tag_dot};

pub(crate) const ICON_SIZE: f32 = 32.;
/// Finder's list row at the view options the target window uses — large icons, text size 14. The
/// same rule holds at 16pt icons and 21pt rows, so the row follows the icon rather than the other
/// way round.
pub(crate) const ROW_H: f32 = ICON_SIZE + 4.;
/// Measured from the list pane's leading edge, not from the Name column: the icon sits 26 in, the
/// label a 6pt gap after it, and the disclosure triangle's 15pt hit area ends where the icon
/// begins. The label offset moves with the icon size; nothing else here does.
pub(crate) const ICON_X: f32 = 26.;
pub(crate) const LABEL_X: f32 = ICON_X + ICON_SIZE + 6.;
pub(crate) const TWIST_W: f32 = 15.;

/// Read off a real window with the area→radius conversion calibrated against AppKit's own
/// `NSBezierPath(roundedRect:)`: the uncalibrated number is about 1pt low, because the formula
/// assumes an ideal quarter-disc where a rendered corner spreads its coverage over the arc.
pub(crate) const ROW_PILL_RADIUS: f32 = 8.;

/// The list's font size is Finder's per-folder `text size` view option and not a system metric: a
/// fresh folder defaults to 13, and the window this is matched against is set to 14.
pub(crate) const ROW_FONT: f32 = 14.;
/// Finder's table starts this far below the header hairline, so the first row's band does not touch
/// the line.
pub(crate) const LIST_TOP_GAP: f32 = 5.;

pub(crate) const INDENT: f32 = 15.;

impl FinderDemo {
    /// One row is 36pt with a 32pt icon 26pt in from the pane's leading edge, the label 6pt after
    /// the icon, and the disclosure triangle's 15pt hit area in the gap before it. Every offset is
    /// measured from the pane rather than from the Name column, which begins 10pt further in.
    pub(crate) fn list_body(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let lines = self.lines(true);
        let this = cx.entity().downgrade();
        let selected: HashSet<SharedString> = self.selected.clone();
        let tags = self.tags.clone();
        let expanded = self.expanded.clone();
        let label = palette.label;
        let secondary = palette.secondary;
        let stripe = palette.stripe;
        let selection = palette.selection;
        let focused = palette.focused;
        let selection_text = palette.selection_text;
        // The same order and widths the header is drawing, taken once: a row that read them from
        // anywhere else is exactly how the two stop lining up.
        let columns = self.list_columns.clone();
        let name_w = self.name_w;
        // Finder's date column is the same face, size and weight as the rest of the row with
        // tabular figures, so a column of times does not shift as its digits change. Resolved once
        // per frame: the feature list behind it allocates.
        let dates = tokens::system_font_tabular(FontWeight::NORMAL);

        let list = uniform_list("ng.finder.list", lines.len(), move |range, _, _| {
            range
                .map(|index| {
                    let line = lines[index].clone();
                    let is_selected = selected.contains(&line.key);
                    let text = if is_selected { selection_text } else { label };
                    let detail = if is_selected && focused {
                        selection_text
                    } else {
                        secondary
                    };
                    let this = this.clone();
                    let open = expanded.contains(&line.key);
                    let indent = line.depth as f32 * INDENT;

                    let mut row = div()
                        .id(("ng.finder.row", index))
                        // The row box spans the pane while its cells keep fixed widths and
                        // `flex_none`, so a pane too narrow for them overflows and the ancestor's
                        // clip cuts the last column mid-glyph. Sizing the box to the cells instead
                        // would pull the banding pill 17pt in from the pane's trailing edge, where
                        // a real window measures 10.
                        .w_full()
                        .h(px(ROW_H))
                        .relative()
                        .flex()
                        .items_center()
                        .font_family(tokens::system_font_family())
                        .text_size(px(ROW_FONT))
                        .text_color(text);
                    // The banding and the selection are one shape: a pill inset from both pane
                    // edges, the full row height. Finder's alternating band is *not* a full-bleed
                    // rectangle — measured off a real window it stops 10pt short of each edge and
                    // carries the same rounded corners, so a selected row needs no band beneath it.
                    let fill = if is_selected {
                        Some(selection)
                    } else if index % 2 == 1 {
                        Some(stripe)
                    } else {
                        None
                    };
                    if let Some(fill) = fill {
                        row = row.child(
                            div()
                                .absolute()
                                .left(px(PANE_INSET))
                                .right(px(PANE_INSET))
                                .top_0()
                                .bottom_0()
                                .rounded(px(ROW_PILL_RADIUS))
                                .bg(fill),
                        );
                    }

                    let twist = {
                        let this = this.clone();
                        let key = line.key.clone();
                        let mut slot = div()
                            .id(("ng.finder.twist", index))
                            .w(px(TWIST_W))
                            .h_full()
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center();
                        // Folders only. A bundle is a directory too, and Finder still gives it no
                        // triangle, so this follows what the row opens rather than what it is.
                        if line.row.opens.is_some() {
                            let arrow = if open {
                                "chevron.down"
                            } else {
                                "chevron.right"
                            };
                            let color = if is_selected && focused {
                                SystemColor::AlternateSelectedControlText
                            } else {
                                SystemColor::SecondaryLabel
                            };
                            if let Some(glyph) = small_glyph(arrow, 9., color) {
                                slot = slot.child(glyph);
                            }
                            slot = slot.on_click(move |_: &ClickEvent, _, cx| {
                                this.update(cx, |this: &mut FinderDemo, cx| {
                                    this.disclose(&key, !open);
                                    cx.notify();
                                })
                                .ok();
                            });
                        }
                        slot
                    };

                    // The gutter ends where the icon begins, so the triangle sitting at its
                    // trailing edge lands at ICON_X - TWIST_W without that number being written
                    // down twice.
                    let mut name = div()
                        .w(px(name_w))
                        .flex_none()
                        .h_full()
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(
                            div()
                                .w(px(ICON_X + indent))
                                .h_full()
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_end()
                                .child(twist),
                        )
                        .child(artwork(&line.row, ICON_SIZE))
                        .child(
                            div()
                                .ml(px(LABEL_X - ICON_X - ICON_SIZE))
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(line.row.name.clone()),
                        );
                    if let Some(tag) = tags.get(&line.key) {
                        name = name.child(div().ml(px(6.)).child(tag_dot(*tag)));
                    }

                    let clicked = line.clone();
                    let mut row = row.child(name);
                    for spec in &columns {
                        let cell = div().w(px(spec.width)).flex_none().text_color(detail);
                        row = row.child(match spec.column {
                            // Finder right-aligns Size and left-aligns the other two, and measures
                            // the inset from whichever edge the text is against.
                            Column::Size => cell
                                .pr(px(CELL_TRAIL))
                                .flex()
                                .justify_end()
                                .font(dates.clone())
                                .child(line.row.size.clone()),
                            Column::Modified => cell
                                .pl(px(CELL_PAD))
                                .font(dates.clone())
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .child(line.row.modified.clone()),
                            Column::Kind => cell
                                .pl(px(CELL_PAD))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(line.row.face.kind()),
                            // `list_columns` never holds Name — that is the flexible cell already
                            // rendered above — so this arm keeps the match exhaustive without
                            // putting some other column's text in a Name-shaped slot.
                            Column::Name => cell,
                        });
                    }
                    row.child(div().w(px(PANE_INSET)).flex_none()).on_click(
                        move |event: &ClickEvent, window, cx| {
                            let (extend, toggle, twice) = match event {
                                ClickEvent::Mouse(mouse) => (
                                    mouse.up.modifiers.shift,
                                    mouse.up.modifiers.platform,
                                    mouse.up.click_count >= 2,
                                ),
                                _ => (false, false, false),
                            };
                            let line = clicked.clone();
                            this.update(cx, |this: &mut FinderDemo, cx| {
                                this.pick(&line, extend, toggle);
                                if twice {
                                    this.open(&line, window, cx);
                                }
                                cx.notify();
                            })
                            .ok();
                        },
                    )
                })
                .collect::<Vec<_>>()
        })
        .flex_1()
        .track_scroll(self.scroll.clone());

        // Finder's table starts 5pt below the header hairline rather than flush against it, so the
        // first row's band does not touch the line.
        div()
            .flex_1()
            .flex()
            .flex_col()
            .pt(px(LIST_TOP_GAP))
            .child(list)
            .into_any_element()
    }
}
