//! The list view's column-header band, and the two drags that reorder and resize it.

use gpui::{
    AnyElement, ClickEvent, Div, DragMoveEvent, ElementId, FontWeight, Hsla, Pixels, TextRun,
    Window, div, hsla, prelude::*, px,
};
use native_gpui::tokens::{self, SystemColor};
use native_gpui::{
    Blending, Layer, Material, glass, material, native_label, native_rule, native_symbol,
};

use crate::FinderDemo;
use crate::data::Column;
use crate::list::{ROW_FONT, column_text, name_prefix};
use crate::menus;
use crate::palette::Palette;
use crate::prefs;
use crate::ui::click_kind;

pub(crate) const COL_MODIFIED: f32 = 181.;
pub(crate) const COL_SIZE: f32 = 97.;
pub(crate) const COL_KIND: f32 = 121.;

pub(crate) const HEADER_H: f32 = 28.;

pub(crate) const PANE_INSET: f32 = 10.;

pub(crate) const HEADER_LABEL_X: f32 = 47.;
pub(crate) const SEP_TOP: f32 = 4.;
pub(crate) const SEP_H: f32 = 19.;
pub(crate) const CELL_PAD: f32 = 7.;
pub(crate) const CELL_TRAIL: f32 = 6.;
pub(crate) const SORT_INSET: f32 = 5.;
const SORT_TOP: f32 = 10.;

pub(crate) const NAME_MIN_W: f32 = 168.;
const COL_MIN_W: f32 = 10.;
const DIVIDER_GRAB: f32 = 5.;
const DIVIDER_LEAD: f32 = 3.;
const PRESS_ALPHA: f32 = 0.31;

#[derive(Clone, Copy)]
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
        ColumnSpec {
            column: Column::Modified,
            width: COL_MODIFIED,
        },
        ColumnSpec {
            column: Column::Size,
            width: COL_SIZE,
        },
        ColumnSpec {
            column: Column::Kind,
            width: COL_KIND,
        },
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
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .right_0()
        .bg(fill)
}

/// The pane's glass, frosting the rows. Only its window-side corners round: the glass starts a
/// radius left of the pane so its left corners fall outside the clip, with a rule for that edge.
pub(crate) fn pane_surface(width: Pixels, corner: Pixels) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .right_0()
        .bottom_0()
        .w(width)
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .left(-corner)
                .child(
                    glass("ng.finder.pane")
                        .frosting_gpui()
                        .corner_radius(corner),
                ),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .bottom_0()
                .w(px(1.))
                .child(native_rule("ng.finder.pane.edge")),
        )
        .into_any_element()
}

/// One material over the toolbar, scope and column-header bands, from the window's edge, so it
/// runs on under the floating sidebar and its gutters and corners frost the rows.
pub(crate) fn band_glass(right: Pixels, height: Pixels) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right(right)
        .h(height)
        .child(
            material("ng.finder.band", Material::Header)
                .layer(Layer::Above)
                // At the default BehindWindow this samples what is behind the window, not the rows.
                .blending(Blending::WithinWindow)
                .size_full(),
        )
        .into_any_element()
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
        CELL_PAD
    };
    // Hosted, not GPUI text: the band's material is above the whole Metal layer and would blur it.
    let label = div().flex_1().mb(px(2.)).child(
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
        // Hosted: a GPUI glyph would sit under the band's material.
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
                    native_symbol(sort_id, arrow, 9., FontWeight::MEDIUM)
                        .tint(SystemColor::HeaderText),
                ),
        );
    }
    body
}

impl FinderDemo {
    pub(crate) fn header_row(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.header_drag.is_some() && !cx.has_active_drag() {
            if matches!(self.header_drag, Some(HeaderDrag::Resize { .. })) {
                self.persist_column_widths();
            }
            self.header_drag = None;
        }
        let carried = match self.header_drag {
            Some(HeaderDrag::Reorder {
                column,
                left: Some(left),
                ..
            }) => Some((column, left)),
            _ => None,
        };
        let wash = Hsla {
            a: PRESS_ALPHA,
            ..palette.label
        };

        let mut row = div()
            .w_full()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .relative()
            // No fill: the band's material has only the rows behind it left to sample.
            .on_drag_move(cx.listener(Self::on_header_drag));

        let dragged = carried.map(|(column, _)| column);
        row = row.child(self.header_cell(None, palette, wash, dragged, cx));
        for index in 0..self.list_columns.len() {
            row = row.child(self.header_cell(Some(index), palette, wash, dragged, cx));
        }
        row = row.child(div().w(px(PANE_INSET)).flex_none());

        // After every cell, so the hit test gives the grab zone to the divider, not the header.
        let mut boundary = self.name_w;
        for index in 0..=self.list_columns.len() {
            if index > 0 {
                boundary += self.list_columns[index - 1].width;
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
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        let (_, _, twice) = click_kind(event);
                        if twice {
                            this.fit_list_column(index, window, cx);
                        }
                    }))
                    .on_drag(HeaderGrab::Divider(index), {
                        let this = cx.entity().downgrade();
                        move |grab: &HeaderGrab, _, window, cx| {
                            let grab = *grab;
                            let origin = f32::from(window.mouse_position().x);
                            this.update(cx, |this: &mut FinderDemo, cx| {
                                this.begin_header_drag(grab, origin, 0.);
                                cx.notify();
                            })
                            .ok();
                            cx.new(|_| crate::dragdrop::DragGhost)
                        }
                    }),
            );
        }

        if let Some((column, left)) = carried {
            let width = self
                .list_columns
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
                    .bg(palette.surface)
                    .child(overlay(wash))
                    .child(cell_body(
                        "ng.finder.header.carried",
                        "ng.finder.header.carried.sort",
                        column,
                        self.sort == column,
                        self.ascending,
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
        .into_any_element()
    }

    fn header_cell(
        &self,
        index: Option<usize>,
        palette: &Palette,
        wash: Hsla,
        dragged: Option<Column>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let last = self.list_columns.len().saturating_sub(1);
        let column = match index {
            Some(index) => self.list_columns[index].column,
            None => Column::Name,
        };
        let slot = index.map_or(0, |index| index + 1);
        let mut cell = cell_body(
            ("ng.finder.header.label", slot),
            ("ng.finder.header.sort", slot),
            column,
            self.sort == column,
            self.ascending,
        )
        .id(("ng.finder.header", slot))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            if this.sort == column {
                this.ascending = !this.ascending;
            } else {
                this.sort = column;
                this.ascending = true;
            }
            cx.notify();
        }));
        cell = match index {
            Some(index) => cell.w(px(self.list_columns[index].width)),
            None => cell.w(px(self.name_w)),
        }
        .flex_none();
        if dragged == Some(column) {
            cell = cell.child(overlay(palette.surface).child(overlay(wash)));
        }
        let interior = match index {
            Some(index) => index < last,
            None => true,
        };
        if interior {
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
                menus::header(self),
                cx.entity().downgrade(),
                cell,
            )
            .into_any_element()
        };
        match index {
            None => with_menu(cell.into_any_element()),
            Some(_) => cell
                .on_drag(HeaderGrab::Column(column), {
                    let this = cx.entity().downgrade();
                    move |grab: &HeaderGrab, offset, window, cx| {
                        let grab = *grab;
                        let origin = f32::from(window.mouse_position().x);
                        let inside = f32::from(offset.x);
                        this.update(cx, |this: &mut FinderDemo, cx| {
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
                for slot in 0..=count {
                    let error = (head - left).abs();
                    if error < best.0 {
                        best = (error, slot);
                    }
                    if slot < count {
                        head += others[slot];
                    }
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
                // Option holds the divider after it still, borrowing the growth from its neighbour
                // instead of letting every column past it slide.
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

    /// Loads this folder's saved widths, or the built-in default for any column it never resized.
    pub(crate) fn load_column_widths(&mut self) {
        let here = self.here();
        self.name_w = prefs::list_column_width(here, Column::Name).unwrap_or(NAME_MIN_W);
        for spec in &mut self.list_columns {
            spec.width = prefs::list_column_width(here, spec.column)
                .unwrap_or_else(|| ColumnSpec::of(spec.column).width);
        }
        self.refit_name_column();
    }

    /// Finder's own gesture: fit a column to its widest currently-loaded cell, standard padding.
    fn fit_list_column(&mut self, index: usize, window: &Window, cx: &mut Context<Self>) {
        let column = match index {
            0 => Column::Name,
            index => self.list_columns[index - 1].column,
        };
        let lines = self.focused_lines();
        let icon = self.list_icon_now();
        let run = TextRun {
            len: 0,
            font: tokens::system_font(FontWeight::NORMAL),
            color: hsla(0., 0., 0., 1.),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let size = px(ROW_FONT);
        let widest = lines.iter().fold(0_f32, |widest, line| {
            let text = column_text(column, line);
            let mut run = run.clone();
            run.len = text.len();
            let text_w = f32::from(
                window
                    .text_system()
                    .shape_line(text, size, &[run], None)
                    .width,
            );
            let total = if column == Column::Name {
                name_prefix(line, icon) + text_w + CELL_PAD
            } else {
                text_w + CELL_PAD + CELL_TRAIL
            };
            widest.max(total)
        });
        let fitted = widest.max(self.resized_column(index).1);
        match index {
            0 => self.name_w = fitted,
            index => {
                if let Some(spec) = self.list_columns.get_mut(index - 1) {
                    spec.width = fitted;
                }
            }
        }
        prefs::set_list_column_width(self.here(), column, fitted);
        cx.notify();
    }
}
