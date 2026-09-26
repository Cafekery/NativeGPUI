use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{
    AnyElement, ClickEvent, Div, ExternalPaths, Font, FontWeight, Hsla, MouseButton, Pixels, Point,
    SharedString, Stateful, WeakEntity, Window, div, prelude::*, px, uniform_list,
};
use native_gpui::tokens::{self, SystemColor};

use crate::app::FinderDemo;
use crate::art::{Anchor, art_at, list_art, small_glyph};
use crate::data::{Column, Line, date_text, enclosing_name};
use crate::dragdrop::{DragGhost, Lit, RowDrag, drop_target, flashed};
use crate::listing::Slot;
use crate::menus::rows_menu;
use crate::palette::Palette;
use crate::tags::tag_dots;
use crate::ui::{axis_locked, menu_ring, pick_on_click, pick_on_press, rename_field, text_width};
use crate::views::header::{CELL_PAD, CELL_TRAIL, ColumnSpec, HEADER_H, PANE_INSET};

pub(crate) const ICON_SIZE: f32 = 32.;
pub(crate) const ICON_SMALL: f32 = 16.;
pub(crate) const ICON_X: f32 = 26.;
pub(crate) const INDENT: f32 = 15.;
pub(crate) const TWIST_W: f32 = 15.;
pub(crate) const ROW_FONT: f32 = 14.;
pub(crate) const ROW_PILL_RADIUS: f32 = 6.;
const SELECTION_JOIN_ALPHA: f32 = 0.1;
const SELECTION_JOIN_SHADE: f32 = 0.05;
const SELECTION_JOIN_LEAD: f32 = 30.;
const SELECTION_JOIN_TRAIL: f32 = 6.;
const SELECTION_DETAIL_ALPHA: f32 = 0.7;
pub(crate) const LIST_TOP_GAP: f32 = 5.;
pub(crate) const LIST_FOOT_GAP: f32 = 5.;
const BAND_INSET: f32 = 8.;
const LABEL_GAP: f32 = 6.;
const TAG_DOTS_GAP: f32 = 6.;

const CLOUD_ART: f32 = 13.;

const PILL_INSET: f32 = 10.;
const PILL_PAD_X: f32 = 8.;
const PILL_PAD_Y: f32 = 3.;
const PILL_FONT: f32 = 11.;
const PILL_RADIUS: f32 = 6.;

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
    selection_detail: Hsla,
    selection_join: Hsla,
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
    menu_row: Option<SharedString>,
    cursor: Option<SharedString>,
    ql_icon: Anchor,
    dropping: Option<SharedString>,
    springing: Option<SharedString>,
}

#[derive(Clone, Copy, Default)]
struct Run {
    above: bool,
    join: Option<Hsla>,
}

pub(crate) fn column_text(column: Column, line: &Line) -> SharedString {
    match column {
        Column::Name => line.row.display.clone(),
        Column::Modified => date_text(line.row.modified_at),
        Column::Size => line.row.size.clone(),
        Column::Kind => line.row.kind.clone(),
    }
}

pub(crate) fn name_prefix(line: &Line, icon: f32) -> f32 {
    ICON_X + line.depth as f32 * INDENT + icon + LABEL_GAP
}

pub(crate) fn row_h(icon: f32) -> f32 {
    icon + 5.
}

impl FinderDemo {
    pub(crate) fn list_inset(&self) -> (usize, f32) {
        // Blank rows, not `uniform_list` padding, whose visible-range arithmetic under-counts it.
        let scope_h = if self.in_search() {
            self.search_bars_h()
        } else {
            0.
        };
        let inset = f32::from(self.metrics().top) + scope_h + HEADER_H + LIST_TOP_GAP;
        let row_h = self.row_h();
        let rows = (inset / row_h).ceil() as usize;
        (rows, rows as f32 * row_h - inset)
    }

    pub(crate) fn list_tail(&self) -> (usize, f32) {
        let foot = LIST_FOOT_GAP + self.path_bar_h();
        let row_h = self.row_h();
        let rows = (foot / row_h).ceil() as usize;
        (rows, rows as f32 * row_h - foot)
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
            selection_detail: palette.selection_text.opacity(SELECTION_DETAIL_ALPHA),
            selection_join: match palette.focused {
                true => tokens::system_color(SystemColor::AlternateSelectedControlText)
                    .opacity(SELECTION_JOIN_ALPHA),
                false => tokens::system_color(SystemColor::Shadow).opacity(SELECTION_JOIN_SHADE),
            },
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
            menu_row: self.menu_row.clone(),
            cursor: self.cursor.clone(),
            ql_icon: self.ql_icon.clone(),
            dropping: self.drop_row().cloned(),
            springing: self.spring_flash().cloned(),
        };

        let (pads, lift) = self.list_inset();
        let slots = self.slots(&lines);
        let hit = self.row_at(pads, look.row_h);
        let count = slots.as_ref().map_or(lines.len(), |slots| slots.len());

        // The count and the rows it indexes come from one read: a second read can be shorter.
        let (tails, trim) = self.list_tail();
        let items = pads + count + tails;
        // Finder stripes the empty rows below the last one; drawn under the list, which cannot scroll there.
        let (stripe, pitch) = (look.stripe, look.row_h);
        let top = f32::from(self.scroll.0.borrow().base_handle.offset().y);
        let fill = (count..count + pads + (self.pane_h / pitch) as usize + 2)
            .filter(|at| at % 2 == 1)
            .map(move |at| {
                div()
                    .absolute()
                    .top(px((pads + at) as f32 * pitch + top))
                    .h(px(pitch))
                    .left(px(PANE_INSET))
                    .right(px(PANE_INSET))
                    .rounded(px(ROW_PILL_RADIUS))
                    .bg(stripe)
            });
        let list = uniform_list("ng.finder.list", items, move |range, window, _| {
            let selected = |index: usize| {
                index.checked_sub(pads).is_some_and(|at| {
                    let at = match slots.as_ref().map(|slots| slots.get(at)) {
                        Some(Some(Slot::Row(at))) => *at,
                        Some(_) => return false,
                        None => at,
                    };
                    lines
                        .get(at)
                        .is_some_and(|line| marks.selected.contains(&line.key))
                })
            };
            range
                .map(|index| {
                    let Some(at) = index.checked_sub(pads).filter(|at| *at < count) else {
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
                    let run = Run {
                        above: index.checked_sub(1).is_some_and(&selected),
                        join: selected(index + 1).then_some(look.selection_join),
                    };
                    row(&lines[at], at, run, &look, &marks, window, this.clone())
                })
                .collect::<Vec<_>>()
        })
        .flex_1()
        .track_scroll(&self.scroll);

        div()
            .flex_1()
            .flex()
            .flex_col()
            .relative()
            .mt(px(-lift))
            .mb(px(-trim))
            .children(fill)
            .child(rows_menu(
                "ng.finder.list.menu",
                cx.entity().downgrade(),
                hit,
                axis_locked(list),
            ))
            .into_any_element()
    }

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

    pub(crate) fn type_filter_pill(&self, palette: &Palette, bottom: f32) -> Option<AnyElement> {
        let text = self.type_filter.clone()?;
        Some(
            div()
                .absolute()
                .right(px(PILL_INSET))
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

/// Finder's selection pill: a run of adjacent selected rows is one block, only its ends rounded,
/// with a hairline at each join.
fn pill(fill: Hsla, run: Run) -> Div {
    let radius = |end: bool| px(if end { ROW_PILL_RADIUS } else { 0. });
    div()
        .absolute()
        .left(px(PANE_INSET))
        .right(px(PANE_INSET))
        .top_0()
        .bottom_0()
        .rounded_t(radius(!run.above))
        .rounded_b(radius(run.join.is_none()))
        .bg(fill)
        .when_some(run.join, |pill, colour| {
            pill.child(
                div()
                    .absolute()
                    .left(px(SELECTION_JOIN_LEAD))
                    .right(px(SELECTION_JOIN_TRAIL))
                    .bottom_0()
                    .h(px(1.))
                    .bg(colour),
            )
        })
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
    if !line.row.discloses {
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
            this.disclose_at(at, &key, into, !open, event.modifiers().alt);
            cx.notify();
        })
        .ok();
    })
}

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
            list_art(&line.row, look.icon, look.scale),
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
                .text_ellipsis_middle()
                .child(line.row.display.clone())
        });
    if let Some(dots) = marks
        .tags
        .get(&line.key)
        .and_then(|carried| tag_dots(carried))
    {
        name = name.child(div().ml(px(TAG_DOTS_GAP)).child(dots));
    }
    if line.row.dataless {
        name = name.child(div().flex_1());
        if let Some(cloud) = small_glyph(
            "icloud",
            CLOUD_ART,
            FontWeight::NORMAL,
            SystemColor::SecondaryLabel,
        ) {
            name = name.child(div().pr(px(CELL_TRAIL)).child(cloud));
        }
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

/// Finder takes the disclosure triangle, the icon and the name it labels, and no more of the row.
fn drop_band(line: &Line, look: &Look, window: &Window) -> (f32, f32) {
    let twist = ICON_X + line.depth as f32 * INDENT;
    let name = twist
        + look.icon
        + LABEL_GAP
        + text_width(window, line.row.display.clone(), px(ROW_FONT))
        + LABEL_GAP;
    (twist, name.min(look.name_w))
}

fn row(
    line: &Line,
    at: usize,
    run: Run,
    look: &Look,
    marks: &Marks,
    window: &Window,
    this: WeakEntity<FinderDemo>,
) -> AnyElement {
    let is_selected = marks.selected.contains(&line.key);
    let aimed =
        |row: &SharedString| *row == line.key || (is_selected && marks.selected.contains(row));
    let dropping = (marks.dropping.as_ref() == Some(&line.key)).then(|| {
        match marks.springing.as_ref() == Some(&line.key) {
            true => flashed(look.selection),
            false => look.selection,
        }
    });
    let lit = dropping.is_some() || (is_selected && look.focused);
    let text = if is_selected || dropping.is_some() {
        look.selection_text
    } else {
        look.label
    };
    let detail = if lit {
        look.selection_detail
    } else {
        look.secondary
    };
    let fill = match (is_selected, at % 2 == 1) {
        (true, _) => Some((look.selection, run)),
        (false, true) => Some((look.stripe, Run::default())),
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
        .children(fill.map(|(fill, run)| pill(fill, run)))
        .children(dropping.map(|fill| pill(fill, Run::default())))
        .children(marks.menu_row.as_ref().filter(|row| aimed(row)).map(|_| {
            let over = is_selected && look.focused;
            let colour = if over {
                look.selection_text
            } else {
                look.selection
            };
            menu_ring(
                colour,
                PANE_INSET,
                ROW_PILL_RADIUS,
                (run.above, run.join.is_some()),
                over,
            )
        }))
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
        Some(into) => drop_target::<ExternalPaths>(
            drop_target::<RowDrag>(
                row,
                line.key.clone(),
                into,
                Some(drop_band(line, look, window)),
                marks.selected.clone(),
                Lit::Drawn,
                this.clone(),
            ),
            line.key.clone(),
            into,
            Some(drop_band(line, look, window)),
            marks.selected.clone(),
            Lit::Drawn,
            this,
        )
        .into_any_element(),
        None => row.into_any_element(),
    }
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
