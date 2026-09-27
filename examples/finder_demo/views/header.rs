use gpui::{
    AnyElement, ClickEvent, Context, Div, DragMoveEvent, ElementId, FontWeight, Hsla, MouseButton,
    MouseDownEvent, Render, SharedString, StyleRefinement, WeakEntity, Window, WindowTextSystem,
    div, prelude::*, px,
};
use native_gpui::tokens::{self, SystemColor};
use native_gpui::{native_cached, native_label, native_rule, native_symbol};

use crate::app::FinderDemo;
use crate::data::{Column, date_fit};
use crate::fit;
use crate::menus;
use crate::palette::Palette;
use crate::prefs;
use crate::ui::click_kind;
use crate::views::list::{name_prefix, row_font};

pub(crate) const COL_MODIFIED: f32 = 181.;
pub(crate) const COL_SIZE: f32 = 97.;
pub(crate) const COL_KIND: f32 = 121.;

pub(crate) const HEADER_H: f32 = 28.;

pub(crate) const PANE_INSET: f32 = 10.;

pub(crate) const HEADER_LABEL_X: f32 = 53.;
const LABEL_PAD: f32 = 3.;
pub(crate) const SEP_TOP: f32 = 6.;
pub(crate) const SEP_H: f32 = 16.;
pub(crate) const CELL_PAD: f32 = 7.;
pub(crate) const CELL_TRAIL: f32 = 6.;
pub(crate) const SORT_INSET: f32 = 5.;
const SORT_TOP: f32 = 10.;

pub(crate) const NAME_MIN_W: f32 = 168.;
const NAME_FIT: f32 = 14.;
const NAME_FIT_LEAST: f32 = 128.;
const NAME_FIT_PER_POINT: f32 = 5.;
const SIZE_FIT: f32 = 18.;
const SIZE_FIT_LEAST: f32 = 75.;
const KIND_FIT: f32 = 24.;
const KIND_FIT_LEAST: f32 = 66.;
const COL_MIN_W: f32 = 10.;
const DIVIDER_GRAB: f32 = 5.;
const DIVIDER_LEAD: f32 = 3.;
const PRESS_ALPHA: f32 = 0.31;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct ColumnSpec {
    pub(crate) column: Column,
    pub(crate) width: f32,
}

impl ColumnSpec {
    pub(crate) fn of(column: Column) -> Self {
        let width = match column {
            Column::Name => NAME_MIN_W,
            Column::Modified => COL_MODIFIED,
            Column::Size => COL_SIZE,
            Column::Kind => COL_KIND,
        };
        Self { column, width }
    }
}

pub(crate) fn default_columns() -> Vec<ColumnSpec> {
    vec![
        ColumnSpec::of(Column::Modified),
        ColumnSpec::of(Column::Size),
        ColumnSpec::of(Column::Kind),
    ]
}

#[derive(Clone, Copy)]
pub(crate) enum HeaderGrab {
    Column(Column),
    Divider(usize),
}

#[derive(Clone, Copy)]
pub(crate) enum HeaderDrag {
    Reorder {
        column: Column,
        grab: f32,
        left: Option<f32>,
    },
    Resize {
        index: usize,
        origin: f32,
        width: f32,
    },
}

pub(crate) fn column_title(column: Column) -> &'static str {
    match column {
        Column::Name => "Name",
        Column::Modified => "Date Modified",
        Column::Size => "Size",
        Column::Kind => "Kind",
    }
}

fn overlay(fill: Hsla) -> Div {
    div().absolute().inset_0().bg(fill)
}

fn cell_body(
    id: impl Into<ElementId>,
    sort_id: impl Into<ElementId>,
    column: Column,
    sorted: bool,
    ascending: bool,
) -> Div {
    let inset = if matches!(column, Column::Name) {
        HEADER_LABEL_X
    } else {
        LABEL_PAD
    };
    // Hosted, not GPUI: the band's material sits above the whole Metal layer and would blur it.
    let label = div().flex_1().child(
        native_label(id, column_title(column))
            .font_size(tokens::small_system_font_size())
            .weight(if sorted {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            })
            .color(if sorted {
                SystemColor::Label
            } else {
                SystemColor::SecondaryLabel
            }),
    );
    let mut body = div()
        .h_full()
        .flex()
        .items_center()
        .relative()
        .overflow_hidden()
        .child(div().w(px(inset)).flex_none())
        .child(label);
    if sorted {
        let arrow = if ascending {
            "chevron.up"
        } else {
            "chevron.down"
        };
        body = body.child(
            div()
                .absolute()
                .right(px(SORT_INSET))
                .top(px(SORT_TOP))
                .child(
                    native_symbol(sort_id, arrow, 9., FontWeight::NORMAL)
                        .tint(SystemColor::HeaderText),
                ),
        );
    }
    body
}

impl FinderDemo {
    pub(crate) fn refit_name_column(&mut self) {
        self.list_span = f32::NAN;
        self.fit_name_column();
    }

    pub(crate) fn fit_name_column(&mut self) {
        let span = self.list_room;
        let fixed = self.list_content_w() - self.name_w;
        if self.list_span.is_nan() {
            self.name_w = (span - fixed).max(NAME_MIN_W);
        } else if span != self.list_span {
            let delta = span - self.list_span;
            let overflow = (self.list_content_w() - self.list_span).max(0.);
            let change = if delta < 0. {
                delta
            } else {
                (delta - overflow).max(0.)
            };
            self.name_w = (self.name_w + change).max(NAME_MIN_W);
        }
        self.list_span = span;
    }

    pub(crate) fn header_row(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.header_drag.is_some() && !cx.has_active_drag() {
            if matches!(self.header_drag, Some(HeaderDrag::Resize { .. })) {
                self.persist_column_widths();
            }
            self.header_drag = None;
        }
        let look = HeaderLook {
            columns: self.list_columns.clone(),
            name_w: self.name_w,
            sort: self.sort,
            ascending: self.ascending,
            carried: match self.header_drag {
                Some(HeaderDrag::Reorder {
                    column,
                    left: Some(left),
                    ..
                }) => Some((column, left)),
                _ => None,
            },
            wash: Hsla {
                a: PRESS_ALPHA,
                ..palette.label
            },
            surface: palette.surface,
        };
        let (band, fresh) = match &self.header {
            Some(band) if band.read(cx).look == look => (band.clone(), false),
            Some(band) => {
                band.update(cx, |band, _| band.look = look);
                (band.clone(), true)
            }
            None => {
                let demo = cx.entity().downgrade();
                let band = cx.new(|_| HeaderBand { look, demo });
                self.header = Some(band.clone());
                (band, false)
            }
        };
        let style = StyleRefinement::default().w_full().h(px(HEADER_H));
        native_cached(band, style, fresh).into_any_element()
    }

    fn resized_column(&self, index: usize) -> (f32, f32) {
        match index {
            0 => (self.name_w, NAME_MIN_W),
            index => (
                self.list_columns
                    .get(index - 1)
                    .map_or(0., |spec| spec.width),
                COL_MIN_W,
            ),
        }
    }

    fn begin_header_drag(&mut self, what: HeaderGrab, origin: f32, grab: f32) {
        self.header_drag = Some(match what {
            HeaderGrab::Column(column) => HeaderDrag::Reorder {
                column,
                grab,
                left: None,
            },
            HeaderGrab::Divider(index) => HeaderDrag::Resize {
                index,
                origin,
                width: self.resized_column(index).0,
            },
        });
    }

    fn on_header_drag(
        &mut self,
        event: &DragMoveEvent<HeaderGrab>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let x = f32::from(event.event.position.x);
        match self.header_drag {
            Some(HeaderDrag::Reorder { column, grab, .. }) => {
                let Some(from) = self
                    .list_columns
                    .iter()
                    .position(|spec| spec.column == column)
                else {
                    return;
                };
                let width = self.list_columns[from].width;
                let total = self.list_columns.iter().map(|spec| spec.width).sum::<f32>();
                let grab = grab.clamp(0., width);
                let start = self.name_w;
                let left = (x - f32::from(event.bounds.origin.x) - grab)
                    .clamp(start, (start + total - width).max(start));

                let mut others = [0.; 3];
                let mut count = 0;
                for (index, spec) in self.list_columns.iter().enumerate() {
                    if index != from {
                        others[count] = spec.width;
                        count += 1;
                    }
                }
                let mut head = start;
                let mut best = (f32::MAX, from);
                for (slot, width) in others[..count].iter().chain([&0.]).enumerate() {
                    let error = (head - left).abs();
                    if error < best.0 {
                        best = (error, slot);
                    }
                    head += width;
                }
                if best.1 != from {
                    let spec = self.list_columns.remove(from);
                    self.list_columns.insert(best.1, spec);
                }
                self.header_drag = Some(HeaderDrag::Reorder {
                    column,
                    grab,
                    left: Some(left),
                });
                cx.notify();
            }
            Some(HeaderDrag::Resize {
                index,
                origin,
                width,
            }) => {
                let next = (width + x - origin).max(self.resized_column(index).1);
                let grown = next - width;
                match index {
                    0 => self.name_w = next,
                    index => {
                        if let Some(spec) = self.list_columns.get_mut(index - 1) {
                            spec.width = next;
                        }
                    }
                }
                if event.event.modifiers.alt {
                    if let Some(neighbour) = self.list_columns.get_mut(index) {
                        neighbour.width = (neighbour.width - grown).max(COL_MIN_W);
                    }
                }
                cx.notify();
            }
            None => {}
        }
    }

    fn persist_column_widths(&self) {
        let here = self.here();
        prefs::set_list_column_width(here, Column::Name, self.name_w);
        for spec in &self.list_columns {
            prefs::set_list_column_width(here, spec.column, spec.width);
        }
    }

    pub(crate) fn load_column_widths(&mut self) {
        let here = self.here();
        self.name_w = prefs::list_column_width(here, Column::Name).unwrap_or(NAME_MIN_W);
        for spec in &mut self.list_columns {
            spec.width = prefs::list_column_width(here, spec.column)
                .unwrap_or_else(|| ColumnSpec::of(spec.column).width);
        }
        self.refit_name_column();
    }

    fn fit_list_column(&mut self, index: usize, cx: &mut Context<Self>) {
        let column = match index {
            0 => Column::Name,
            index => self.list_columns[index - 1].column,
        };
        let size = row_font();
        if column == Column::Modified {
            return self.fitted(index, column, date_fit(size), cx);
        }
        let rows = self.fit_rows(column);
        let here = self.here();
        fit::in_background(
            cx,
            move |texts| fit_width(texts, column, &rows, size),
            move |this, width, cx| {
                let now = match index {
                    0 => Some(Column::Name),
                    index => this.list_columns.get(index - 1).map(|spec| spec.column),
                };
                if this.here() == here && now == Some(column) {
                    this.fitted(index, column, width, cx);
                }
            },
        );
    }

    /// Each row's lead before its text, and the text as `fit_width` measures it.
    fn fit_rows(&self, column: Column) -> Vec<(f32, SharedString)> {
        let icon = self.list_icon_now();
        self.focused_lines()
            .iter()
            .map(|line| match column {
                Column::Name => (name_prefix(line, icon), line.row.display.clone()),
                Column::Size => (0., line.row.size.clone()),
                _ => (0., line.row.kind.clone()),
            })
            .collect()
    }

    fn fitted(&mut self, index: usize, column: Column, width: f32, cx: &mut Context<Self>) {
        match index {
            0 => self.name_w = width,
            index => self.list_columns[index - 1].width = width,
        }
        prefs::set_list_column_width(self.here(), column, width);
        cx.notify();
    }
}

/// Everything the header band paints: an unchanged band replays its last frame.
#[derive(PartialEq)]
pub(crate) struct HeaderLook {
    columns: Vec<ColumnSpec>,
    name_w: f32,
    sort: Column,
    ascending: bool,
    carried: Option<(Column, f32)>,
    wash: Hsla,
    surface: Hsla,
}

pub(crate) struct HeaderBand {
    look: HeaderLook,
    demo: WeakEntity<FinderDemo>,
}

impl Render for HeaderBand {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        band_row(&self.look, &self.demo)
    }
}

fn band_row(look: &HeaderLook, demo: &WeakEntity<FinderDemo>) -> Div {
    let mut row = div()
        .w_full()
        .h(px(HEADER_H))
        .flex_none()
        .flex()
        .relative()
        // No fill: the band's material has only the rows behind it left to sample.
        .on_drag_move({
            let demo = demo.clone();
            move |event: &DragMoveEvent<HeaderGrab>, window, cx| {
                demo.update(cx, |this, cx| this.on_header_drag(event, window, cx))
                    .ok();
            }
        });

    let dragged = look.carried.map(|(column, _)| column);
    row = row.child(header_cell(look, None, dragged, demo));
    for index in 0..look.columns.len() {
        row = row.child(header_cell(look, Some(index), dragged, demo));
    }
    row = row.child(div().w(px(PANE_INSET)).flex_none());

    // After every cell, so the hit test gives the grab zone to the divider, not the header.
    let mut boundary = look.name_w;
    for index in 0..=look.columns.len() {
        if index > 0 {
            boundary += look.columns[index - 1].width;
        }
        row = row.child(
            div()
                .id(("ng.finder.divider", index))
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(boundary - DIVIDER_LEAD))
                .w(px(DIVIDER_GRAB))
                .cursor_ew_resize()
                .on_click({
                    let demo = demo.clone();
                    move |event: &ClickEvent, _, cx| {
                        let (_, _, twice) = click_kind(event);
                        if twice {
                            demo.update(cx, |this, cx| this.fit_list_column(index, cx))
                                .ok();
                        }
                    }
                })
                .on_mouse_down(MouseButton::Left, {
                    let demo = demo.clone();
                    move |event: &MouseDownEvent, _, cx| {
                        demo.update(cx, |this, _| {
                            this.header_press = Some(f32::from(event.position.x));
                        })
                        .ok();
                    }
                })
                .on_drag(HeaderGrab::Divider(index), {
                    let demo = demo.clone();
                    move |grab: &HeaderGrab, _, window, cx| {
                        let grab = *grab;
                        let now = f32::from(window.mouse_position().x);
                        demo.update(cx, |this: &mut FinderDemo, cx| {
                            // gpui starts a drag only once the pointer has moved; measured from
                            // there, the divider falls behind the pointer by that much.
                            let origin = this.header_press.take().unwrap_or(now);
                            this.begin_header_drag(grab, origin, 0.);
                            cx.notify();
                        })
                        .ok();
                        cx.new(|_| crate::dragdrop::DragGhost)
                    }
                }),
        );
    }

    if let Some((column, left)) = look.carried {
        let width = look
            .columns
            .iter()
            .find(|spec| spec.column == column)
            .map_or(0., |spec| spec.width);
        row = row.child(
            div()
                .absolute()
                .top_0()
                .h_full()
                .left(px(left))
                .w(px(width))
                .bg(look.surface)
                .child(overlay(look.wash))
                .child(cell_body(
                    "ng.finder.header.carried",
                    "ng.finder.header.carried.sort",
                    column,
                    look.sort == column,
                    look.ascending,
                )),
        );
    }

    row.child(
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(1.))
            .child(native_rule("ng.finder.header.hairline")),
    )
}

fn header_cell(
    look: &HeaderLook,
    index: Option<usize>,
    dragged: Option<Column>,
    demo: &WeakEntity<FinderDemo>,
) -> AnyElement {
    let column = match index {
        Some(index) => look.columns[index].column,
        None => Column::Name,
    };
    let slot = index.map_or(0, |index| index + 1);
    let mut cell = cell_body(
        ("ng.finder.header.label", slot),
        ("ng.finder.header.sort", slot),
        column,
        look.sort == column,
        look.ascending,
    )
    .id(("ng.finder.header", slot))
    .on_click({
        let demo = demo.clone();
        move |_: &ClickEvent, _, cx| {
            demo.update(cx, |this, cx| {
                this.sort_by(column, cx);
                cx.notify();
            })
            .ok();
        }
    });
    cell = match index {
        Some(index) => cell.w(px(look.columns[index].width)),
        None => cell.w(px(look.name_w)),
    }
    .flex_none();
    if dragged == Some(column) {
        cell = cell.child(overlay(look.surface).child(overlay(look.wash)));
    }
    if slot < look.columns.len() {
        cell = cell.child(
            div()
                .absolute()
                .right_0()
                .top(px(SEP_TOP))
                .w(px(1.))
                .h(px(SEP_H))
                .child(native_rule(("ng.finder.header.sep", slot))),
        );
    }
    let with_menu = |cell: AnyElement| {
        menus::attach(
            ("ng.finder.header.menu", slot),
            menus::header(&look.columns),
            demo.clone(),
            cell,
        )
        .into_any_element()
    };
    match index {
        None => with_menu(cell.into_any_element()),
        Some(_) => cell
            .on_drag(HeaderGrab::Column(column), {
                let demo = demo.clone();
                move |grab: &HeaderGrab, offset, window, cx| {
                    let grab = *grab;
                    let origin = f32::from(window.mouse_position().x);
                    let inside = f32::from(offset.x);
                    demo.update(cx, |this: &mut FinderDemo, cx| {
                        this.begin_header_drag(grab, origin, inside);
                        cx.notify();
                    })
                    .ok();
                    cx.new(|_| crate::dragdrop::DragGhost)
                }
            })
            .map(|cell| with_menu(cell.into_any_element())),
    }
}

/// The width Finder fits a list column to: its widest cell, in the font the cell draws, plus a
/// margin, rounded up and no narrower than its least.
fn fit_width(
    texts: &WindowTextSystem,
    column: Column,
    rows: &[(f32, SharedString)],
    size: f32,
) -> f32 {
    let (margin, least) = match column {
        Column::Name => (NAME_FIT, NAME_FIT_LEAST + NAME_FIT_PER_POINT * size),
        Column::Size => (SIZE_FIT, SIZE_FIT_LEAST),
        _ => (KIND_FIT, KIND_FIT_LEAST),
    };
    let font = match column {
        Column::Size => tokens::system_font_tabular(FontWeight::NORMAL),
        _ => tokens::system_font(FontWeight::NORMAL),
    };
    let widest = fit::widest(
        texts,
        &font,
        px(size),
        rows,
        |row| row.1.clone(),
        |row, text_w| row.0 + text_w + margin,
    );
    widest.ceil().max(least)
}
