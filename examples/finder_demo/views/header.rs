use std::sync::LazyLock;

use gpui::{
    AnyElement, ClickEvent, Context, Div, DragMoveEvent, ElementId, FontWeight, Hsla, MouseButton,
    MouseDownEvent, Render, SharedString, Stateful, StyleRefinement, WeakEntity, Window,
    WindowTextSystem, div, prelude::*, px,
};
use native_gpui::tokens::{self, SystemColor};
use native_gpui::{native_cached, native_label, native_rule, native_symbol};

use crate::app::FinderDemo;
use crate::data::{Column, date_fit};
use crate::dragdrop::DragGhost;
use crate::fit;
use crate::menus;
use crate::palette::Palette;
use crate::prefs;
use crate::ui::click_kind;
use crate::view::overflows;
use crate::views::list::{ICON_SMALL, detail_text, name_prefix};

const COL_DATE: f32 = 181.;
const COL_SIZE: f32 = 97.;
const COL_KIND: f32 = 115.;
const COL_OPENED: f32 = 200.;
const COL_TAGS: f32 = 100.;
const COL_VERSION: f32 = 75.;
const COL_COMMENTS: f32 = 300.;
const COL_SHARED_BY: f32 = 200.;

pub(crate) const HEADER_H: f32 = 28.;
pub(crate) const PANE_INSET: f32 = 10.;
pub(crate) const LAST_EXTRA: f32 = 6.;

const HEADER_LABEL_X: f32 = 53.;
const LABEL_PAD: f32 = 3.;
const SEP_TOP: f32 = 6.;
const SEP_H: f32 = 16.;
const SORT_INSET: f32 = 5.;
const SORT_TOP: f32 = 10.;

const NAME_FIT: f32 = 14.;
const NAME_FIT_LEAST: f32 = 128.;
const NAME_FIT_PER_POINT: f32 = 5.;
const NAME_OPEN_LEAST: f32 = 206.;
const SIZE_FIT: f32 = 18.;
const SIZE_FIT_LEAST: f32 = 75.;
const KIND_FIT: f32 = 24.;
const KIND_FIT_LEAST: f32 = 66.;
const COL_MIN_W: f32 = 10.;
const DIVIDER_GRAB: f32 = 5.;
const DIVIDER_LEAD: f32 = 3.;
const PRESS_ALPHA: f32 = 0.31;

fn name_least(size: f32, icon: f32) -> f32 {
    PANE_INSET + NAME_FIT_LEAST + icon - ICON_SMALL + NAME_FIT_PER_POINT * size
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct ColumnSpec {
    pub(crate) column: Column,
    pub(crate) width: f32,
}

impl ColumnSpec {
    pub(crate) fn of(column: Column) -> Self {
        let width = match column {
            Column::Name => name_least(prefs::finder().text_size, ICON_SMALL),
            Column::Modified | Column::Created | Column::Added => COL_DATE,
            Column::Opened => COL_OPENED,
            Column::Size => COL_SIZE,
            Column::Kind | Column::Application => COL_KIND,
            Column::Tags => COL_TAGS,
            Column::Version => COL_VERSION,
            Column::Comments => COL_COMMENTS,
            Column::SharedBy => COL_SHARED_BY,
        };
        Self { column, width }
    }
}

pub(crate) fn columns_for(folder: &str) -> Vec<ColumnSpec> {
    prefs::list_columns(folder)
        .unwrap_or_else(|| prefs::finder().columns.clone())
        .into_iter()
        .filter(|column| column.offered_at(folder))
        .map(ColumnSpec::of)
        .collect()
}

#[derive(Clone, Copy)]
enum HeaderGrab {
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
    const KEYS: [(Column, &str, &str); 12] = [
        (Column::Name, "N220", "Name"),
        (Column::Kind, "N224", "Kind"),
        (Column::Opened, "N169.32", "Date Last Opened"),
        (Column::Added, "N230", "Date Added"),
        (Column::Modified, "N221", "Date Modified"),
        (Column::Created, "N222", "Date Created"),
        (Column::Size, "N223", "Size"),
        (Column::Tags, "N225", "Tags"),
        (Column::Version, "N226", "Version"),
        (Column::Comments, "N227", "Comments"),
        (Column::Application, "KIND_FORMATTER_20_0", "Application"),
        (Column::SharedBy, "N231", "Shared By"),
    ];
    static TITLES: LazyLock<Vec<String>> = LazyLock::new(|| {
        KEYS.iter()
            .map(|(_, key, english)| finder_fs::words::finder(key, english))
            .collect()
    });
    let at = KEYS.iter().position(|(held, _, _)| *held == column);
    &TITLES[at.unwrap_or(0)]
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
    let inset = match column {
        Column::Name => HEADER_LABEL_X,
        _ => LABEL_PAD,
    };
    let (weight, color) = match sorted {
        true => (FontWeight::SEMIBOLD, SystemColor::Label),
        false => (FontWeight::NORMAL, SystemColor::SecondaryLabel),
    };
    let arrow = if ascending {
        "chevron.up"
    } else {
        "chevron.down"
    };
    div()
        .h_full()
        .flex()
        .items_center()
        .relative()
        .overflow_hidden()
        .child(div().w(px(inset)).flex_none())
        // Hosted, not GPUI: the band's material sits above the whole Metal layer and would blur it.
        .child(
            div().flex_1().child(
                native_label(id, column_title(column))
                    .font_size(tokens::small_system_font_size())
                    .weight(weight)
                    .color(color),
            ),
        )
        .when(sorted, |body| {
            body.child(
                div()
                    .absolute()
                    .right(px(SORT_INSET))
                    .top(px(SORT_TOP))
                    .child(
                        native_symbol(sort_id, arrow, 9., FontWeight::NORMAL)
                            .tint(SystemColor::HeaderText),
                    ),
            )
        })
}

impl FinderDemo {
    pub(crate) fn name_min_w(&self) -> f32 {
        name_least(self.list_look.text, self.list_look.icon)
    }

    /// Name absorbs window resizes while the list fits; an overflowing list keeps its widths.
    pub(crate) fn fit_name_column(&mut self) {
        let span = self.list_room;
        let content = self.list_content_w();
        if self.list_span.is_nan() {
            self.name_w = (span - content + self.name_w).max(self.name_min_w());
        } else if span != self.list_span {
            let change = match overflows(content, self.list_span) {
                true => (span - content).max(0.),
                false => span - self.list_span,
            };
            self.name_w = (self.name_w + change).max(self.name_min_w());
        }
        self.list_span = span;
    }

    pub(crate) fn shown_columns(&self) -> Vec<ColumnSpec> {
        let mut columns = self.list_columns.clone();
        if let Some(last) = columns.last_mut() {
            last.width += LAST_EXTRA;
        }
        columns
    }

    pub(crate) fn header_row(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.header_drag.is_some() && !cx.has_active_drag() {
            match self.header_drag.take() {
                Some(HeaderDrag::Resize { .. }) => self.persist_column_widths(),
                _ => self.persist_columns(),
            }
        }
        let look = HeaderLook {
            columns: self.shown_columns(),
            here: self.here(),
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

    fn column_at(&self, index: usize) -> Option<Column> {
        match index {
            0 => Some(Column::Name),
            index => self.list_columns.get(index - 1).map(|spec| spec.column),
        }
    }

    fn resized_column(&self, index: usize) -> (f32, f32) {
        match index {
            0 => (self.name_w, self.name_min_w()),
            index => (
                self.list_columns
                    .get(index - 1)
                    .map_or(0., |spec| spec.width),
                COL_MIN_W,
            ),
        }
    }

    fn set_column_w(&mut self, index: usize, width: f32) {
        match index {
            0 => self.name_w = width,
            index => {
                if let Some(spec) = self.list_columns.get_mut(index - 1) {
                    spec.width = width;
                }
            }
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

    fn on_header_drag(&mut self, event: &DragMoveEvent<HeaderGrab>, cx: &mut Context<Self>) {
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
                let total: f32 = self.list_columns.iter().map(|spec| spec.width).sum();
                let grab = grab.clamp(0., width);
                let start = self.name_w;
                let left = (x - f32::from(event.bounds.origin.x) - grab)
                    .clamp(start, (start + total - width).max(start));

                let others = (self.list_columns.iter().enumerate())
                    .filter(|(index, _)| *index != from)
                    .map(|(_, spec)| spec.width);
                let mut head = start;
                let mut best = (f32::MAX, from);
                for (slot, width) in others.chain([0.]).enumerate() {
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
            }
            Some(HeaderDrag::Resize {
                index,
                origin,
                width,
            }) => {
                let next = (width + x - origin).max(self.resized_column(index).1);
                self.set_column_w(index, next);
                if event.event.modifiers.alt {
                    if let Some(neighbour) = self.list_columns.get_mut(index) {
                        neighbour.width = (neighbour.width - (next - width)).max(COL_MIN_W);
                    }
                }
            }
            None => return,
        }
        cx.notify();
    }

    fn persist_column_widths(&self) {
        let here = self.here();
        prefs::set_list_column_width(here, Column::Name, self.name_w);
        for spec in &self.list_columns {
            prefs::set_list_column_width(here, spec.column, spec.width);
        }
    }

    fn persist_columns(&self) {
        let columns: Vec<Column> = self.list_columns.iter().map(|spec| spec.column).collect();
        prefs::set_list_columns(self.here(), &columns);
    }

    pub(crate) fn load_columns(&mut self) {
        let here = self.here();
        self.list_columns = columns_for(here);
        for spec in &mut self.list_columns {
            spec.width = prefs::list_column_width(here, spec.column)
                .unwrap_or_else(|| ColumnSpec::of(spec.column).width);
        }
        let least = self.name_min_w().max(PANE_INSET + NAME_OPEN_LEAST);
        self.name_w =
            prefs::list_column_width(here, Column::Name).map_or(least, |kept| kept.max(least));
        let fill = self.list_room - self.list_content_w() + self.name_w;
        if fill >= least {
            self.name_w = fill;
        }
        self.list_span = self.list_room;
    }

    pub(crate) fn toggle_column(&mut self, column: Column, cx: &mut Context<Self>) {
        let read = self.wants_extras();
        match self
            .list_columns
            .iter()
            .position(|spec| spec.column == column)
        {
            Some(index) => {
                self.list_columns.remove(index);
            }
            None => self.list_columns.push(ColumnSpec::of(column)),
        }
        self.persist_columns();
        self.list_span = f32::NAN;
        self.fit_name_column();
        if !read && self.wants_extras() {
            self.resort((self.sort, self.ascending), cx);
        }
    }

    fn fit_list_column(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(column) = self.column_at(index) else {
            return;
        };
        let look = self.list_look;
        if column.dated() {
            return self.fitted(index, column, date_fit(look.text, look.relative), cx);
        }
        let rows = self.fit_rows(column);
        let here = self.here();
        fit::in_background(
            cx,
            move |texts| fit_width(texts, column, &rows, look.text, look.icon),
            move |this, width, cx| {
                if this.here() == here && this.column_at(index) == Some(column) {
                    this.fitted(index, column, width, cx);
                }
            },
        );
    }

    fn fit_rows(&self, column: Column) -> Vec<(f32, SharedString)> {
        let icon = self.list_icon_now();
        self.focused_lines()
            .iter()
            .map(|line| match column {
                Column::Name => (name_prefix(line, icon), line.row.display.clone()),
                Column::Size => (0., line.row.size_shown(self.list_look.sizes)),
                Column::Kind => (0., line.row.kind.clone()),
                column => (0., detail_text(&line.row, column)),
            })
            .collect()
    }

    fn fitted(&mut self, index: usize, column: Column, width: f32, cx: &mut Context<Self>) {
        self.set_column_w(index, width);
        prefs::set_list_column_width(self.here(), column, width);
        cx.notify();
    }
}

#[derive(PartialEq)]
struct HeaderLook {
    columns: Vec<ColumnSpec>,
    here: &'static str,
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
        let (look, demo) = (&self.look, &self.demo);
        // No fill: the band's material has only the rows behind it left to sample.
        let mut row = div()
            .w_full()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .relative()
            .on_drag_move({
                let demo = demo.clone();
                move |event: &DragMoveEvent<HeaderGrab>, _, cx| {
                    demo.update(cx, |this, cx| this.on_header_drag(event, cx))
                        .ok();
                }
            })
            .child(header_cell(look, None, demo))
            .children((0..look.columns.len()).map(|index| header_cell(look, Some(index), demo)))
            .child(div().w(px(PANE_INSET)).flex_none());

        // After every cell, so the hit test gives the grab zone to the divider, not the header.
        let mut boundary = look.name_w;
        for index in 0..=look.columns.len() {
            row = row.child(divider(index, boundary, demo));
            boundary += look.columns.get(index).map_or(0., |spec| spec.width);
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
}

fn divider(index: usize, x: f32, demo: &WeakEntity<FinderDemo>) -> Stateful<Div> {
    div()
        .id(("ng.finder.divider", index))
        .absolute()
        .top_0()
        .bottom_0()
        .left(px(x - DIVIDER_LEAD))
        .w(px(DIVIDER_GRAB))
        .cursor_ew_resize()
        .on_click({
            let demo = demo.clone();
            move |event: &ClickEvent, _, cx| {
                if click_kind(event).2 {
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
                    // gpui starts a drag only once the pointer has moved: measure from the press.
                    let origin = this.header_press.take().unwrap_or(now);
                    this.begin_header_drag(grab, origin, 0.);
                    cx.notify();
                })
                .ok();
                cx.new(|_| DragGhost)
            }
        })
}

fn header_cell(
    look: &HeaderLook,
    index: Option<usize>,
    demo: &WeakEntity<FinderDemo>,
) -> AnyElement {
    let (column, width) = match index {
        Some(index) => (look.columns[index].column, look.columns[index].width),
        None => (Column::Name, look.name_w),
    };
    let slot = index.map_or(0, |index| index + 1);
    let cell = cell_body(
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
    })
    .w(px(width))
    .flex_none()
    .when(
        look.carried.is_some_and(|(carried, _)| carried == column),
        |cell| cell.child(overlay(look.surface).child(overlay(look.wash))),
    )
    .when(slot < look.columns.len(), |cell| {
        cell.child(
            div()
                .absolute()
                .right_0()
                .top(px(SEP_TOP))
                .w(px(1.))
                .h(px(SEP_H))
                .child(native_rule(("ng.finder.header.sep", slot))),
        )
    })
    .when(index.is_some(), |cell| {
        cell.on_drag(HeaderGrab::Column(column), {
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
                cx.new(|_| DragGhost)
            }
        })
    });
    menus::attach(
        ("ng.finder.header.menu", slot),
        menus::header(&look.columns, look.here),
        demo.clone(),
        cell.into_any_element(),
    )
    .into_any_element()
}

fn fit_width(
    texts: &WindowTextSystem,
    column: Column,
    rows: &[(f32, SharedString)],
    size: f32,
    icon: f32,
) -> f32 {
    let (margin, least) = match column {
        Column::Name => (PANE_INSET + NAME_FIT, name_least(size, icon)),
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
