use std::collections::HashSet;

use gpui::{
    AnyElement, ClickEvent, Div, DragMoveEvent, FontWeight, Hsla, SharedString, Window, div,
    prelude::*, px, uniform_list,
};
use native_gpui::tokens::{self, SystemColor};

use crate::data::Column;
use crate::header::{CELL_PAD, CELL_TRAIL, DragGhost, HEADER_H, PANE_INSET};
use crate::menus::row_menu;
use crate::palette::Palette;
use crate::ui::{artwork, axis_locked, click_kind, rename_field, small_glyph, tag_dot};
use crate::{FinderDemo, Slot};

pub(crate) const ICON_SIZE: f32 = 32.;
const BAND_INSET: f32 = 8.;
pub(crate) const ROW_H: f32 = ICON_SIZE + 4.;
pub(crate) const ICON_X: f32 = 26.;
pub(crate) const LABEL_X: f32 = ICON_X + ICON_SIZE + 6.;
pub(crate) const TWIST_W: f32 = 15.;

pub(crate) const ROW_PILL_RADIUS: f32 = 8.;

pub(crate) const ROW_FONT: f32 = 14.;
pub(crate) const LIST_TOP_GAP: f32 = 5.;

pub(crate) const INDENT: f32 = 15.;

impl FinderDemo {
    /// The list's top content inset: `(blank leading rows, how far the list frame is lifted)`.
    pub(crate) fn list_inset(&self) -> (usize, f32) {
        // Blank rows, not `uniform_list` padding, whose visible-range arithmetic under-counts it.
        let inset = f32::from(self.metrics().top) + HEADER_H + LIST_TOP_GAP;
        let rows = (inset / ROW_H).ceil() as usize;
        (rows, rows as f32 * ROW_H - inset)
    }

    pub(crate) fn list_body(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let lines = self.lines(true);
        let this = cx.entity().downgrade();
        let renaming = self.renaming.clone();
        let selected: HashSet<SharedString> = self.selected.clone();
        let tags = self.tags.clone();
        let expanded = self.expanded.clone();
        let label = palette.label;
        let secondary = palette.secondary;
        let stripe = palette.stripe;
        let selection = palette.selection;
        let focused = palette.focused;
        let selection_text = palette.selection_text;
        let columns = self.list_columns.clone();
        let name_w = self.name_w;
        let dates = tokens::system_font_tabular(FontWeight::NORMAL);

        let (pads, lift) = self.list_inset();
        let slots = self.slots(&lines);
        let drag_keys: HashSet<SharedString> = self.selected.clone();

        let list = uniform_list("ng.finder.list", pads + slots.len(), move |range, _, _| {
            range
                .map(|index| {
                    let Some(slot) = index.checked_sub(pads).map(|at| &slots[at]) else {
                        return div()
                            .id(("ng.finder.pad", index))
                            .h(px(ROW_H))
                            .w_full()
                            .into_any_element();
                    };
                    let at = match slot {
                        Slot::Header(title) => {
                            return band(title.clone(), secondary, name_w).into_any_element();
                        }
                        Slot::Row(at) => *at,
                    };
                    let line = lines[at].clone();
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
                        .id(("ng.finder.row", at))
                        .w_full()
                        .h(px(ROW_H))
                        .relative()
                        .flex()
                        .items_center()
                        .font_family(tokens::system_font_family())
                        .text_size(px(ROW_FONT))
                        .text_color(text);
                    let fill = if is_selected {
                        Some(selection)
                    } else if at % 2 == 1 {
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
                            .id(("ng.finder.twist", at))
                            .w(px(TWIST_W))
                            .h_full()
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center();
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
                            if let Some(glyph) = small_glyph(arrow, 9., FontWeight::BOLD, color) {
                                slot = slot.child(glyph);
                            }
                            let into = line.row.opens;
                            slot = slot.on_click(move |event: &ClickEvent, _, cx| {
                                this.update(cx, |this: &mut FinderDemo, cx| {
                                    // Finder opens the whole subtree when Option is down.
                                    match (event.modifiers().alt, into) {
                                        (true, Some(into)) => this.disclose_tree(&key, into, !open),
                                        _ => this.disclose(&key, !open),
                                    }
                                    cx.notify();
                                })
                                .ok();
                            });
                        }
                        slot
                    };

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
                        .child(if renaming.as_ref() == Some(&line.key) {
                            div()
                                .ml(px(LABEL_X - ICON_X - ICON_SIZE))
                                .flex_1()
                                .child(rename_field(&line, at, this.clone()))
                        } else {
                            div()
                                .ml(px(LABEL_X - ICON_X - ICON_SIZE))
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(line.row.name.clone())
                        });
                    if let Some(tag) = tags.get(&line.key) {
                        name = name.child(div().ml(px(6.)).child(tag_dot(*tag)));
                    }

                    let clicked = line.clone();
                    let mut row = row.child(name);
                    for spec in &columns {
                        let cell = div().w(px(spec.width)).flex_none().text_color(detail);
                        row = row.child(match spec.column {
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
                            // `list_columns` never holds Name; this only keeps the match exhaustive.
                            Column::Name => cell,
                        });
                    }
                    let row = row.child(div().w(px(PANE_INSET)).flex_none()).on_click({
                        let this = this.clone();
                        move |event: &ClickEvent, window, cx| {
                            let (extend, toggle, twice) = click_kind(event);
                            let line = clicked.clone();
                            this.update(cx, |this: &mut FinderDemo, cx| {
                                this.pick(&line, extend, toggle);
                                if twice {
                                    this.open(&line, window, cx);
                                }
                                cx.notify();
                            })
                            .ok();
                        }
                    });
                    let row = {
                        let keys = drag_keys.clone();
                        let key = line.key.clone();
                        let name = line.row.name.clone();
                        row.on_drag(RowDrag { key: key.clone() }, move |grab, _, _, cx| {
                            let _ = (grab, &keys, &name);
                            cx.new(|_| DragGhost)
                        })
                    };
                    let row = match line.row.opens {
                        Some(into) => row
                            .drag_over::<RowDrag>(move |style, _, _, _| style.bg(selection))
                            .on_drop({
                                let this = this.clone();
                                move |grab: &RowDrag, window: &mut Window, cx| {
                                    let key = grab.key.clone();
                                    // Finder copies instead of moving while Option is held.
                                    let copy = window.modifiers().alt;
                                    this.update(cx, |demo: &mut FinderDemo, cx| {
                                        let keys = demo.drag_keys(&key);
                                        demo.drop_into(into, &keys, copy);
                                        cx.notify();
                                    })
                                    .ok();
                                }
                            })
                            .on_drag_move({
                                let this = this.clone();
                                move |event: &DragMoveEvent<RowDrag>, window: &mut Window, cx| {
                                    if !event.bounds.contains(&event.event.position) {
                                        return;
                                    }
                                    this.update(cx, |demo: &mut FinderDemo, cx| {
                                        demo.hover_folder(into, window, cx);
                                    })
                                    .ok();
                                }
                            }),
                        None => row,
                    };
                    row_menu(("ng.finder.row.menu", at), &line, &selected, this, row)
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .flex_1()
        .track_scroll(&self.scroll);

        div()
            .flex_1()
            .flex()
            .flex_col()
            .mt(px(-lift))
            .child(axis_locked(list))
            .into_any_element()
    }
}

/// Finder's group header: the band's name over the rows it holds.
fn band(title: SharedString, colour: Hsla, name_w: f32) -> Div {
    div()
        .h(px(ROW_H))
        .w(px(name_w))
        .flex()
        .items_end()
        .pb(px(2.))
        .pl(px(BAND_INSET))
        .child(
            div()
                .text_color(colour)
                .text_size(tokens::small_system_font_size())
                .font_weight(FontWeight::BOLD)
                .child(title),
        )
}

/// What a row drag carries: the row the pointer grabbed, standing for the whole selection.
#[derive(Clone)]
pub(crate) struct RowDrag {
    pub(crate) key: SharedString,
}
