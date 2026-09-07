//! The list view, and the row every other view's list is measured against.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{
    AnyElement, ClickEvent, Div, DragMoveEvent, Font, FontWeight, Hsla, MouseButton, Pixels, Point,
    SharedString, Stateful, WeakEntity, Window, div, prelude::*, px, uniform_list,
};
use native_gpui::tokens::{self, SystemColor};

use crate::app::FinderDemo;
use crate::art::{Anchor, art_at, row_art, small_glyph};
use crate::chrome::scope_bar::SCOPE_H;
use crate::data::{Column, Line, date_text, enclosing_name};
use crate::dragdrop::{DragGhost, RowDrag, carries};
use crate::listing::Slot;
use crate::menus::rows_menu;
use crate::palette::Palette;
use crate::tags::tag_dots;
use crate::ui::{axis_locked, pick_on_click, pick_on_press, rename_field};
use crate::views::header::{CELL_PAD, CELL_TRAIL, ColumnSpec, HEADER_H, PANE_INSET};

/// Finder's two list icon sizes; ⌘= and ⌘- toggle between them.
pub(crate) const ICON_SIZE: f32 = 32.;
pub(crate) const ICON_SMALL: f32 = 16.;
pub(crate) const ICON_X: f32 = 26.;
pub(crate) const INDENT: f32 = 15.;
pub(crate) const TWIST_W: f32 = 15.;
pub(crate) const ROW_FONT: f32 = 14.;
pub(crate) const ROW_PILL_RADIUS: f32 = 8.;
pub(crate) const LIST_TOP_GAP: f32 = 5.;
const BAND_INSET: f32 = 8.;
const LABEL_GAP: f32 = 6.;
const TAG_DOTS_GAP: f32 = 6.;

const PILL_INSET: f32 = 10.;
const PILL_PAD_X: f32 = 8.;
const PILL_PAD_Y: f32 = 3.;
const PILL_FONT: f32 = 11.;
const PILL_RADIUS: f32 = 6.;

/// Everything the rows of one frame draw with, so a row builder takes one of these.
struct Look {
    icon: f32,
    row_h: f32,
    name_w: f32,
    scale: f32,
    label: Hsla,
    secondary: Hsla,
    stripe: Hsla,
    selection: Hsla,
    selection_text: Hsla,
    focused: bool,
    dates: Font,
    searching: bool,
    columns: Vec<ColumnSpec>,
}

struct Marks {
    selected: Arc<HashSet<SharedString>>,
    cut: Arc<HashSet<SharedString>>,
    expanded: Arc<HashSet<SharedString>>,
    tags: HashMap<SharedString, Vec<usize>>,
    renaming: Option<SharedString>,
    cursor: Option<SharedString>,
    ql_icon: Anchor,
}

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

pub(crate) fn row_h(icon: f32) -> f32 {
    icon + 4.
}

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
        let icon = self.list_icon_now();
        let look = Look {
            icon,
            row_h: row_h(icon),
            name_w: self.name_w,
            scale: self.scale,
            label: palette.label,
            secondary: palette.secondary,
            stripe: palette.stripe,
            selection: palette.selection,
            selection_text: palette.selection_text,
            focused: palette.focused,
            dates: tokens::system_font_tabular(FontWeight::NORMAL),
            // Finder's search results show a path where the Kind column sits.
            searching: self.results(),
            columns: self.list_columns.clone(),
        };
        let marks = Marks {
            selected: self.selected.snapshot(),
            cut: self.cut.snapshot(),
            expanded: self.expanded.snapshot(),
            tags: self.tags.clone(),
            renaming: self.renaming.clone(),
            cursor: self.cursor.clone(),
            ql_icon: self.ql_icon.clone(),
        };

        let (pads, lift) = self.list_inset();
        let slots = self.slots(&lines);
        let hit = self.row_at(pads, look.row_h);
        let count = slots.as_ref().map_or(lines.len(), |slots| slots.len());

        // The count and the rows it indexes come from one read: a second read can be shorter.
        let list = uniform_list("ng.finder.list", pads + count, move |range, _, _| {
            range
                .map(|index| {
                    let Some(at) = index.checked_sub(pads) else {
                        return div()
                            .id(("ng.finder.pad", index))
                            .h(px(look.row_h))
                            .w_full()
                            .into_any_element();
                    };
                    let at = match slots.as_ref().map(|slots| &slots[at]) {
                        Some(Slot::Header(title)) => {
                            return band(title.clone(), look.secondary, look.name_w, look.row_h)
                                .into_any_element();
                        }
                        Some(Slot::Row(at)) => *at,
                        None => at,
                    };
                    row(&lines[at], at, &look, &marks, this.clone())
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
            .child(rows_menu(
                "ng.finder.list.menu",
                cx.entity().downgrade(),
                hit,
                axis_locked(list),
            ))
            .into_any_element()
    }

    /// Which row a point in the list's own coordinates lands on, for the one menu the rows share.
    fn row_at(
        &self,
        pads: usize,
        row_h: f32,
    ) -> impl Fn(&FinderDemo, Point<Pixels>) -> Option<Line> + use<> {
        let scroll = self.scroll.clone();
        move |demo, at| {
            let top = f32::from(scroll.0.borrow().base_handle.offset().y);
            let index = ((f32::from(at.y) - top) / row_h).floor();
            let at = (index.max(0.) as usize).checked_sub(pads)?;
            let lines = demo.lines(true);
            match demo.slots(&lines).as_ref().map(|slots| slots.get(at)) {
                Some(Some(Slot::Row(row))) => lines.get(*row).cloned(),
                Some(_) => None,
                None => lines.get(at).cloned(),
            }
        }
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

/// Finder's selection pill, inset from both edges so it never runs under the pane.
fn pill(fill: Hsla) -> Div {
    div()
        .absolute()
        .left(px(PANE_INSET))
        .right(px(PANE_INSET))
        .top_0()
        .bottom_0()
        .rounded(px(ROW_PILL_RADIUS))
        .bg(fill)
}

fn twist(
    line: &Line,
    at: usize,
    open: bool,
    lit: bool,
    this: WeakEntity<FinderDemo>,
) -> Stateful<Div> {
    let mut slot = div()
        .id(("ng.finder.twist", at))
        .w(px(TWIST_W))
        .h_full()
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    if line.row.opens.is_none() {
        return slot;
    }
    let arrow = if open {
        "chevron.down"
    } else {
        "chevron.right"
    };
    let color = if lit {
        SystemColor::AlternateSelectedControlText
    } else {
        SystemColor::SecondaryLabel
    };
    if let Some(glyph) = small_glyph(arrow, 9., FontWeight::BOLD, color) {
        slot = slot.child(glyph);
    }
    let into = line.row.opens;
    let key = line.key.clone();
    slot.on_click(move |event: &ClickEvent, _, cx| {
        this.update(cx, |this: &mut FinderDemo, cx| {
            // Finder opens the whole subtree when Option is down.
            match (event.modifiers().alt, into) {
                (true, Some(into)) => this.disclose_tree(&key, into, !open),
                _ => this.disclose(&key, !open),
            }
            cx.notify();
        })
        .ok();
    })
}

/// The Name column: the twist, the icon, the name or the field renaming it, and the tag dots.
fn name_cell(
    line: &Line,
    at: usize,
    look: &Look,
    marks: &Marks,
    lit: bool,
    this: WeakEntity<FinderDemo>,
) -> Div {
    let open = marks.expanded.contains(&line.key);
    let indent = line.depth as f32 * INDENT;
    let mut name = div()
        .w(px(look.name_w))
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
                .child(twist(line, at, open, lit, this.clone())),
        )
        .child(art_at(
            row_art(&line.row, look.icon, look.scale, true),
            look.icon,
            (marks.cursor.as_ref() == Some(&line.key)).then(|| marks.ql_icon.clone()),
        ))
        .child(if marks.renaming.as_ref() == Some(&line.key) {
            div()
                .ml(px(LABEL_GAP))
                .flex_1()
                .child(rename_field(line, at, this))
        } else {
            div()
                .ml(px(LABEL_GAP))
                .overflow_hidden()
                .text_ellipsis()
                .child(line.row.name.clone())
        });
    if let Some(dots) = marks
        .tags
        .get(&line.key)
        .and_then(|carried| tag_dots(carried))
    {
        name = name.child(div().ml(px(TAG_DOTS_GAP)).child(dots));
    }
    name
}

fn detail_cell(spec: &ColumnSpec, line: &Line, look: &Look, detail: Hsla) -> Div {
    let cell = div().w(px(spec.width)).flex_none().text_color(detail);
    match spec.column {
        Column::Size => cell
            .pr(px(CELL_TRAIL))
            .flex()
            .justify_end()
            .font(look.dates.clone())
            .child(line.row.size.clone()),
        Column::Modified => cell
            .pl(px(CELL_PAD))
            .font(look.dates.clone())
            .whitespace_nowrap()
            .overflow_hidden()
            .child(date_text(line.row.modified_at)),
        Column::Kind => cell
            .pl(px(CELL_PAD))
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(if look.searching {
                enclosing_name(&line.key)
            } else {
                line.row.kind.clone()
            }),
        // `list_columns` never holds Name; this only keeps the match exhaustive.
        Column::Name => cell,
    }
}

fn row(
    line: &Line,
    at: usize,
    look: &Look,
    marks: &Marks,
    this: WeakEntity<FinderDemo>,
) -> AnyElement {
    let is_selected = marks.selected.contains(&line.key);
    let lit = is_selected && look.focused;
    let text = if is_selected {
        look.selection_text
    } else {
        look.label
    };
    let detail = if lit {
        look.selection_text
    } else {
        look.secondary
    };
    let fill = match (is_selected, at % 2 == 1) {
        (true, _) => Some(look.selection),
        (false, true) => Some(look.stripe),
        (false, false) => None,
    };
    let mut row = div()
        .id(("ng.finder.row", at))
        .w_full()
        .h(px(look.row_h))
        .relative()
        .flex()
        .items_center()
        .font_family(tokens::system_font_family())
        .text_size(px(ROW_FONT))
        .text_color(text)
        .when(marks.cut.contains(&line.key), |row| row.opacity(0.5))
        .children(fill.map(pill))
        .child(name_cell(line, at, look, marks, lit, this.clone()));
    for spec in &look.columns {
        row = row.child(detail_cell(spec, line, look, detail));
    }
    let row = row
        .child(div().w(px(PANE_INSET)).flex_none())
        .on_mouse_down(MouseButton::Left, pick_on_press(line, this.clone()))
        .on_click(pick_on_click(line, this.clone()))
        .on_drag(
            RowDrag {
                key: line.key.clone(),
            },
            move |_, _, _, cx| cx.new(|_| DragGhost),
        );
    match line.row.opens {
        Some(into) => drop_target(row, line, into, marks, look.selection, this).into_any_element(),
        None => row.into_any_element(),
    }
}

/// A folder row takes the rows dragged onto it, and springs open under a drag that rests there.
fn drop_target(
    row: Stateful<Div>,
    line: &Line,
    into: &'static str,
    marks: &Marks,
    selection: Hsla,
    this: WeakEntity<FinderDemo>,
) -> Stateful<Div> {
    row.drag_over::<RowDrag>({
        let carried = marks.selected.clone();
        let key = line.key.clone();
        move |style, grab: &RowDrag, _, _| match carries(&carried, grab, &key) {
            true => style,
            false => style.bg(selection),
        }
    })
    .on_drop({
        let this = this.clone();
        let carried = marks.selected.clone();
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
        let carried = marks.selected.clone();
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
    })
}

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
