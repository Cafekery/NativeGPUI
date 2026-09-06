use gpui::{
    AnyElement, ClickEvent, Div, DragMoveEvent, FontWeight, Hsla, MouseButton, MouseDownEvent,
    SharedString, Window, div, prelude::*, px, uniform_list,
};
use native_gpui::tokens::{self, SystemColor};

use crate::app::FinderDemo;
use crate::art::{art_at, row_art, small_glyph};
use crate::chrome::scope_bar::SCOPE_H;
use crate::data::{Column, Line, date_text};
use crate::dragdrop::{DragGhost, RowDrag, carries};
use crate::listing::Slot;
use crate::menus::row_menu;
use crate::palette::Palette;
use crate::tags::tag_dots;
use crate::ui::{axis_locked, click_kind, rename_field};
use crate::views::header::{CELL_PAD, CELL_TRAIL, HEADER_H, PANE_INSET};

/// A column's text for a row, to draw or to measure for the divider's double-click-to-fit.
pub(crate) fn column_text(column: Column, line: &Line) -> SharedString {
    match column {
        Column::Name => line.row.name.clone(),
        Column::Modified => date_text(line.row.modified_at),
        Column::Size => line.row.size.clone(),
        Column::Kind => line.row.kind.clone(),
    }
}

/// The Name column's leading room before its text: the twist, the indent, the icon, the gap.
pub(crate) fn name_prefix(line: &Line, icon: f32) -> f32 {
    ICON_X + line.depth as f32 * INDENT + icon + LABEL_GAP
}

/// Finder's two list icon sizes; ⌘= and ⌘- toggle between them.
pub(crate) const ICON_SIZE: f32 = 32.;
pub(crate) const ICON_SMALL: f32 = 16.;
const BAND_INSET: f32 = 8.;
pub(crate) const ICON_X: f32 = 26.;
const LABEL_GAP: f32 = 6.;
pub(crate) const TWIST_W: f32 = 15.;

pub(crate) fn row_h(icon: f32) -> f32 {
    icon + 4.
}

pub(crate) const ROW_PILL_RADIUS: f32 = 8.;

pub(crate) const ROW_FONT: f32 = 14.;
pub(crate) const LIST_TOP_GAP: f32 = 5.;

pub(crate) const INDENT: f32 = 15.;

impl FinderDemo {
    /// The list's top content inset: `(blank leading rows, how far the list frame is lifted)`.
    pub(crate) fn list_inset(&self) -> (usize, f32) {
        // Blank rows, not `uniform_list` padding, whose visible-range arithmetic under-counts it.
        let scope_h = if self.searching() { SCOPE_H } else { 0. };
        let inset = f32::from(self.metrics().top) + scope_h + HEADER_H + LIST_TOP_GAP;
        let row_h = self.row_h();
        let rows = (inset / row_h).ceil() as usize;
        (rows, rows as f32 * row_h - inset)
    }

    pub(crate) fn list_body(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let lines = self.lines(true);
        let this = cx.entity().downgrade();
        let renaming = self.renaming.clone();
        let cursor = self.cursor.clone();
        let ql_icon = self.ql_icon.clone();
        let selected = self.selected.snapshot();
        let cut = self.cut.snapshot();
        let tags = self.tags.clone();
        let expanded = self.expanded.snapshot();
        let label = palette.label;
        let secondary = palette.secondary;
        let stripe = palette.stripe;
        let selection = palette.selection;
        let focused = palette.focused;
        let selection_text = palette.selection_text;
        let columns = self.list_columns.clone();
        let name_w = self.name_w;
        let dates = tokens::system_font_tabular(FontWeight::NORMAL);
        // Finder's search results show a path where the Kind column sits.
        let searching = self.results();

        let (pads, lift) = self.list_inset();
        let scale = self.scale;
        let icon = self.list_icon_now();
        let row_h = row_h(icon);
        let slots = self.slots(&lines);

        let list = uniform_list("ng.finder.list", pads + slots.len(), move |range, _, _| {
            range
                .map(|index| {
                    let Some(slot) = index.checked_sub(pads).map(|at| &slots[at]) else {
                        return div()
                            .id(("ng.finder.pad", index))
                            .h(px(row_h))
                            .w_full()
                            .into_any_element();
                    };
                    let at = match slot {
                        Slot::Header(title) => {
                            return band(title.clone(), secondary, name_w, row_h)
                                .into_any_element();
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
                        .h(px(row_h))
                        .relative()
                        .flex()
                        .items_center()
                        .font_family(tokens::system_font_family())
                        .text_size(px(ROW_FONT))
                        .text_color(text)
                        // Finder's own cut styling: the row dims until pasted or the mark clears.
                        .when(cut.contains(&line.key), |row| row.opacity(0.5));
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
                        .child(art_at(
                            row_art(&line.row, icon, scale, true),
                            icon,
                            (cursor.as_ref() == Some(&line.key)).then(|| ql_icon.clone()),
                        ))
                        .child(if renaming.as_ref() == Some(&line.key) {
                            div().ml(px(LABEL_GAP)).flex_1().child(rename_field(
                                &line,
                                at,
                                this.clone(),
                            ))
                        } else {
                            div()
                                .ml(px(LABEL_GAP))
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(line.row.name.clone())
                        });
                    if let Some(dots) = tags.get(&line.key).and_then(|carried| tag_dots(carried)) {
                        name = name.child(div().ml(px(6.)).child(dots));
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
                                .child(date_text(line.row.modified_at)),
                            Column::Kind => cell
                                .pl(px(CELL_PAD))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(if searching {
                                    crate::data::enclosing_name(&line.key)
                                } else {
                                    line.row.kind.clone()
                                }),
                            // `list_columns` never holds Name; this only keeps the match exhaustive.
                            Column::Name => cell,
                        });
                    }
                    let row = row
                        .child(div().w(px(PANE_INSET)).flex_none())
                        .on_mouse_down(MouseButton::Left, {
                            let this = this.clone();
                            let pressed = line.clone();
                            move |event: &MouseDownEvent, _, cx| {
                                if event.modifiers.shift || event.modifiers.platform {
                                    return;
                                }
                                let line = pressed.clone();
                                this.update(cx, |this: &mut FinderDemo, cx| {
                                    if !this.selected.contains(&line.key) {
                                        this.pick(&line, false, false);
                                        cx.notify();
                                    }
                                })
                                .ok();
                            }
                        })
                        .on_click({
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
                    let row = row.on_drag(
                        RowDrag {
                            key: line.key.clone(),
                        },
                        move |_, _, _, cx| cx.new(|_| DragGhost),
                    );
                    let row = match line.row.opens {
                        Some(into) => row
                            .drag_over::<RowDrag>({
                                let carried = selected.clone();
                                let key = line.key.clone();
                                move |style, grab: &RowDrag, _, _| match carries(
                                    &carried, grab, &key,
                                ) {
                                    true => style,
                                    false => style.bg(selection),
                                }
                            })
                            .on_drop({
                                let this = this.clone();
                                let carried = selected.clone();
                                let key = line.key.clone();
                                move |grab: &RowDrag, window: &mut Window, cx| {
                                    if carries(&carried, grab, &key) {
                                        return;
                                    }
                                    let key = grab.key.clone();
                                    // Finder copies instead of moving while Option is held.
                                    let copy = window.modifiers().alt;
                                    this.update(cx, |demo: &mut FinderDemo, cx| {
                                        let keys = demo.drag_keys(&key);
                                        demo.drop_into(into, &keys, copy, cx);
                                        cx.notify();
                                    })
                                    .ok();
                                }
                            })
                            .on_drag_move({
                                let this = this.clone();
                                let carried = selected.clone();
                                let key = line.key.clone();
                                move |event: &DragMoveEvent<RowDrag>, window: &mut Window, cx| {
                                    if !event.bounds.contains(&event.event.position)
                                        || carries(&carried, event.drag(cx), &key)
                                    {
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

    /// The type-select fallback's pill, bottom-right of the list, while a filter is live.
    pub(crate) fn type_filter_pill(
        &self,
        palette: &Palette,
        right: f32,
        bottom: f32,
    ) -> Option<AnyElement> {
        let text = self.type_filter.clone()?;
        Some(
            div()
                .absolute()
                .right(px(right + PILL_INSET))
                .bottom(px(bottom + PILL_INSET))
                .px(px(PILL_PAD_X))
                .py(px(PILL_PAD_Y))
                .rounded(px(PILL_RADIUS))
                .bg(tokens::system_color(SystemColor::QuaternarySystemFill))
                .font_family(tokens::system_font_family())
                .text_size(px(PILL_FONT))
                .text_color(palette.secondary)
                .child(text)
                .into_any_element(),
        )
    }
}

const PILL_INSET: f32 = 10.;
const PILL_PAD_X: f32 = 8.;
const PILL_PAD_Y: f32 = 3.;
const PILL_FONT: f32 = 11.;
const PILL_RADIUS: f32 = 6.;

/// Finder's group header: the band's name over the rows it holds.
fn band(title: SharedString, colour: Hsla, name_w: f32, row_h: f32) -> Div {
    div()
        .h(px(row_h))
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
