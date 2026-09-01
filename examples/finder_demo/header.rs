//! The column header band, the column order and widths the list agrees with it on, and the two
//! drags that change them.
//!
//! Owns the default column widths, the paddings a cell and the pane are inset by,
//! `FinderDemo::header_row`, and the reorder/resize arithmetic.
//!
//! Deliberately not here: the rows themselves. `list` reads the order and the widths from the same
//! state this module edits — the header and the rows are laid out by different functions, and a
//! width written down in only one of them is exactly how the two stop lining up.

use gpui::{
    AnyElement, ClickEvent, Div, DragMoveEvent, ElementId, FontWeight, Hsla, Pixels, Render,
    Window, div, prelude::*, px,
};
use native_gpui::tokens::{self, SystemColor};
use native_gpui::{Blending, Layer, Material, material, native_label, native_rule};

use crate::FinderDemo;
use crate::data::Column;
use crate::palette::Palette;
use crate::ui::small_glyph;

/// Column widths as Finder *draws* them, which are not the widths it stores — Name is stored 178
/// and drawn 197. Defaults only: Finder keeps a width per column per folder, which is why two
/// windows on this machine drew the same four columns at 197/181/75/121 and at 168/181/97/128.
///
/// These do not flex with the pane. Finder's list columns are fixed, so the row content keeps one
/// width however narrow the pane leaves the list and the list cuts it. A flexible Name column is
/// the same layout with the cut removed, and removing the cut is what stops a Kind string ever
/// being sliced mid-word.
pub(crate) const COL_MODIFIED: f32 = 181.;
pub(crate) const COL_SIZE: f32 = 97.;
pub(crate) const COL_KIND: f32 = 121.;

/// Finder's column header band. Its own bottom hairline is the only line in the chrome above the
/// list: there is none under the toolbar, which shares one continuous background with the header.
pub(crate) const HEADER_H: f32 = 28.;

/// The columns region is inset this far from both pane edges. Both the selection *and* the
/// alternating row band span exactly it, as one rounded shape — neither runs the full pane width.
pub(crate) const PANE_INSET: f32 = 10.;

/// Where the Name header's label box starts, measured from the pane's leading edge.
///
/// Finder's ink lands 47.5 from the pane, and a label box is not its ink: the system face at 11pt
/// puts `N`'s outline 0.988pt right of the pen. An ink detector reports the first device pixel
/// carrying any ink, so at 2x it floors to half a point — which is why a 47 box reads as 47.5 of
/// ink and the previous 46 read as 46.5. That 46 had the *difference* from `CELL_PAD` exactly
/// right and both absolutes one bearing short, which is what an ink figure written into a box
/// constant looks like.
/// The header's inset does not follow the icon size the way a row's does — at 16pt icons the two
/// coincide and at 32pt they do not.
pub(crate) const HEADER_LABEL_X: f32 = 47.;
/// A header separator is 1pt wide at the boundary, and it is **not** flush to the band's bottom:
/// measured across three captures of a real window, nine rules, it runs y 56.0 to 75.0 in a
/// 52.0-to-80.0 band. So 4pt of top inset, 19pt tall, and a 5pt gap before the hairline. Reading it
/// as 24pt flush is what a detector does when it joins the rule to the hairline it stops short of,
/// and that also accounts for the peak grey of 63 reported alongside: the rule's own pixels are a
/// uniform 51 and the 63 belongs to the hairline. It starts 2pt higher than a stock header view's
/// does, which is one of only two places Finder's header is not the stock component.
pub(crate) const SEP_TOP: f32 = 4.;
pub(crate) const SEP_H: f32 = 19.;
/// Where text starts inside a fixed column, in the header *and* in the rows — one constant for
/// both, which is not what an earlier reading of Finder said.
///
/// Seven readings off one capture, five leading and two trailing, all reproduced by a leading box
/// of 7 and a trailing box of 6 and by no other whole value: header ink 7.5 on all three columns,
/// the date row's ink 7.5, a capital-initial Kind row's 8.0, and the right-aligned Size row's ink
/// ending 6.5 before its trailing boundary. The reason one box explains inks that differ is the
/// bearing: 0.988pt on an 11pt `D`, 0.618pt on an 11pt `S`, 0.643pt on a 14pt `T`, 1.258pt on a
/// 14pt `P`, and a detector that reports the first device pixel carrying ink floors to half a point
/// at 2x. Every leading candidate except 7 misses all five, and every trailing candidate except 6
/// misses both.
///
/// A header-versus-row stagger was the earlier reading, from a row inset published as 6.0. On the
/// capture that number came from, the row's own ink measures 7.5 — the same as the header's — so
/// there is no stagger and one constant is right. What is genuinely asymmetric is leading against
/// trailing, by exactly 1pt, which is why the right-aligned column has its own.
pub(crate) const CELL_PAD: f32 = 7.;
/// The trailing inset for the right-aligned Size column, which is 1pt tighter than the leading one.
/// Measuring it from the trailing edge is how three different strings there come out sharing one
/// right edge.
pub(crate) const CELL_TRAIL: f32 = 6.;
/// The sort indicator's inset from the sorted column's trailing edge.
pub(crate) const SORT_INSET: f32 = 7.5;

/// The Name column's floor, and also where Finder starts it: four independent readings put Finder's
/// Name at 168 and none of them went below it, so the default sits exactly on the floor. The
/// earlier 120 came from a layout where Name absorbed whatever the fixed columns left, which is a
/// floor no reading ever produced — it was low enough never to bind, and a floor that never binds
/// is a floor that was never measured.
pub(crate) const NAME_MIN_W: f32 = 168.;
/// `NSTableColumn.minWidth`'s default, which Finder has no reason to have changed.
const COL_MIN_W: f32 = 10.;
/// The resize hit zone at a column boundary. `NSTableHeaderView` adds a `resizeLeftRight` cursor
/// rect 5pt wide over the band's full height, reaching further back into the column than forward.
const DIVIDER_GRAB: f32 = 5.;
/// How much of that zone lies before the boundary: AppKit's rect runs boundary−3 to boundary+2.
const DIVIDER_LEAD: f32 = 3.;
/// A pressed header cell's fill goes (29,29,29) → (99,99,99), which is the drawn colour plus white
/// at this alpha. No public colour reaches it — the whole `systemFill` ladder tops out at 51 and
/// `controlColor` at 85 — so it is applied over `labelColor`'s hue, which inverts in light mode
/// where a literal white would not.
const PRESS_ALPHA: f32 = 0.31;

/// One fixed column: the order these are in is the order they are drawn in.
///
/// Name is deliberately not representable here. It is pinned first, and `NSOutlineView` will
/// happily move its own `outlineTableColumn` out of position 0, so the pin is Finder's policy,
/// applied through `outlineView:shouldReorderColumn:toColumn:`. Leaving Name out of the
/// reorderable list is what enforces it here; its width lives on the demo as `name_w` because a
/// resize can move it.
#[derive(Clone, Copy)]
pub(crate) struct ColumnSpec {
    pub(crate) column: Column,
    pub(crate) width: f32,
}

/// The three fixed columns at Finder's drawn widths, in Finder's order.
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

/// What a header drag is carrying. gpui filters `on_drag_move` on the payload's type, so both drags
/// share one type and discriminate inside it.
#[derive(Clone, Copy)]
pub(crate) enum HeaderGrab {
    /// A column being carried to a new position.
    Column(Column),
    /// The boundary at the *trailing* edge of the column it resizes: index 0 is Name's, and index
    /// `i` thereafter is `list_columns[i - 1]`'s. That is where `NSTableHeaderView` puts its own
    /// resize rect, and it is the only mapping under which a fixed left-anchored block resizes the
    /// column the pointer is pulling rather than the one after it.
    Divider(usize),
}

/// A header drag in progress, held on the demo because the header lays out from it.
#[derive(Clone, Copy)]
pub(crate) enum HeaderDrag {
    /// `grab` is where inside the cell the pointer took hold. `left` is where the carried cell's
    /// leading edge is drawn, pane-relative and clamped into the band — `None` until the first
    /// move reports the band's bounds, because gpui runs the row's move listener before the cell's
    /// drag-start listener and so the frame that begins a drag has no position yet. Drawing that
    /// frame at zero would flash the carried cell over the Name column.
    Reorder {
        column: Column,
        grab: f32,
        left: Option<f32>,
    },
    /// `origin` is the pointer at grab time and `width` the resized column's width as it was then,
    /// so every frame applies one absolute delta to a snapshot instead of accumulating deltas. One
    /// column moves: with fixed columns the block is anchored at the pane's leading edge, so a
    /// boundary drag changes the total content width instead of trading width with a neighbour.
    Resize {
        index: usize,
        origin: f32,
        width: f32,
    },
}

/// The header title for a column. A free function rather than a method because `data` owns
/// `Column` and a display string is the header's business.
pub(crate) fn column_title(column: Column) -> &'static str {
    match column {
        Column::Name => "Name",
        Column::Modified => "Date Modified",
        Column::Size => "Size",
        Column::Kind => "Kind",
    }
}

/// gpui paints an active drag's view at the cursor in **both** axes, and a dragged column header
/// stays inside the band. So the carried cell is drawn by the header itself and this renders
/// nothing; its only job is to be the payload view gpui insists on.
pub(crate) struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A full-bleed overlay inside a `relative` parent, for the pressed and carried washes.
fn overlay(fill: Hsla) -> Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .right_0()
        .bg(fill)
}

/// The glass over the toolbar band and the column-header band, as one material spanning both.
///
/// `Layer::Above` is what makes it sample and blur GPUI's own rendering, and that is the whole
/// mechanism: macOS 26 hands a real `NSScrollView` a scroll-edge surface the chrome above it
/// samples, and a GPUI list has no scroll view to inherit one from. `Blending::WithinWindow` is
/// required alongside it — at the default `BehindWindow` the material samples what is behind the
/// *window*, so over a list it composites over nothing and reports its own flat tint.
///
/// One material across both bands rather than one each. Finder's own band carries visible content
/// at y 54 and y 78, inside the header band as well as the toolbar's, and the contrast across the
/// pair measures monotonic — a seam between two materials would be a step in it.
pub(crate) fn band_glass(top: Pixels) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(top + px(HEADER_H))
        .child(
            material("ng.finder.band", Material::Header)
                .layer(Layer::Above)
                .blending(Blending::WithinWindow)
                .size_full(),
        )
        .into_any_element()
}

/// The inside of a header cell: the leading inset, the label, and the sort chevron when this is the
/// sorted column. Shared by the cell in its slot and by the copy a reorder drag carries, so the two
/// cannot drift apart.
///
/// The label is a hosted `NSTextField`, not GPUI text, and that is the whole reason this band can
/// be glass. A `Layer::Above` material is above the entire Metal layer rather than only the pixels
/// painted before it, so GPUI text here would be sampled and blurred by the very material that is
/// meant to blur the rows behind it. `id` keys the hosted view's pool slot, so it has to be stable
/// per cell across frames — keyed by slot, which is a fixed set whatever order the columns are in.
fn cell_body(id: impl Into<ElementId>, column: Column, sorted: bool, ascending: bool) -> Div {
    // A leading spacer rather than padding: the separator and the sort indicator are positioned
    // from this cell's own edges, and padding would move what those edges mean.
    let inset = if matches!(column, Column::Name) {
        HEADER_LABEL_X
    } else {
        CELL_PAD
    };
    // Finder left-aligns the Size *header* while right-aligning its values, so this one
    // deliberately does not mirror the row. The label is not centred in the band either: measured
    // 1pt above centre, which a bottom margin buys without moving the cell's own edges — the
    // separator and the sort indicator are positioned from those.
    //
    // The sorted column's weight still has to be written even hosted: `highlightedTableColumn` is
    // what applies it in a stock header and a plain label has no such notion. A stock header's
    // sorted `Date Modified` measures 75.0pt of ink, which is Finder's own number: semibold
    // reproduces it to 0.5pt where medium is 1.0 out and bold 2.0.
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
        let arrow = if ascending {
            "chevron.up"
        } else {
            "chevron.down"
        };
        // Still a GPUI raster, and therefore still sampled by the band's material: an SF Symbol
        // arrives as an `NSImage` and nothing in the crate hosts one. The sorted column stays
        // identifiable without it — its label is hosted, and semibold in `labelColor` against
        // regular in `secondaryLabelColor` survives sharp — so what the wash costs is the
        // direction, not the column.
        if let Some(glyph) = small_glyph(arrow, 9., SystemColor::SecondaryLabel) {
            body = body.child(
                div()
                    .absolute()
                    .right(px(SORT_INSET))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(glyph),
            );
        }
    }
    body
}

impl FinderDemo {
    /// The column header. 28pt, one hairline along its own bottom edge and none under the toolbar,
    /// and a 1pt separator standing at each interior column boundary — never at the leading edge of
    /// Name or the trailing edge of the last column.
    ///
    /// The sorted column's label is both bolder and brighter than the rest: 11pt semibold in
    /// `labelColor` against 11pt regular in `secondaryLabelColor`.
    ///
    /// Dragging a header carries the column. Dragging a boundary resizes the column it is the
    /// trailing edge of and slides everything after it, which is what a fixed block anchored at the
    /// pane's leading edge does and is the direction `NSTableHeaderView` moves in. Neither drag
    /// needs a mouse-up listener: gpui drops the active drag on the first mouse event that is not a
    /// move, so the carried cell is gone the frame after the pointer lifts.
    pub(crate) fn header_row(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.header_drag.is_some() && !cx.has_active_drag() {
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
            // The band spans the pane; the cells inside it do not. Each is `flex_none` at a fixed
            // width, so a pane too narrow for them overflows and the pane's clip cuts the last
            // column mid-glyph — which is the visible form of the list going behind the preview.
            .w_full()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .relative()
            // No fill, and that is what makes the band glass. Finder draws no backdrop here either:
            // the 29 in its band is the list showing through a transparent titlebar, which is why
            // blurred rows turn up inside the band while a folder is being scrolled. A fill of our
            // own would be the only thing the band's material had left to sample, and the rows
            // behind it would never appear.
            .on_drag_move(cx.listener(Self::on_header_drag));

        let dragged = carried.map(|(column, _)| column);
        row = row.child(self.header_cell(None, palette, wash, dragged, cx));
        for index in 0..self.list_columns.len() {
            row = row.child(self.header_cell(Some(index), palette, wash, dragged, cx));
        }
        row = row.child(div().w(px(PANE_INSET)).flex_none());

        // Painted after every cell so gpui's hit test hands the grab zone to the divider rather
        // than to the header under it, which is what keeps a resize from also sorting.
        //
        // Anchored from the leading edge, one per column trailing boundary including the last:
        // Finder's own header lets the final column be resized too, and with a fixed block there is
        // no neighbour behind it to take the width from.
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
                            cx.new(|_| DragGhost)
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
                        column,
                        self.sort == column,
                        self.ascending,
                    )),
            );
        }

        row
            // The hairline is drawn rather than bordered so the band keeps its full 28pt: a bottom
            // border would take the pt back out of the content box and shorten every separator.
            .child(
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

    /// One header cell. `index` is a slot in `columns`; `None` is the Name column, which carries no
    /// reorder drag and has no separator at its leading edge.
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
            column,
            self.sort == column,
            self.ascending,
        )
        .id(("ng.finder.header", slot))
        .active(move |style| style.bg(wash))
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
            // The slot the carried column currently occupies. Its own body stays laid out so the
            // separator holds the boundary; the wash over it is what reads as a gap.
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
        match index {
            None => cell.into_any_element(),
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
                        cx.new(|_| DragGhost)
                    }
                })
                .into_any_element(),
        }
    }

    /// The width a divider index resizes, and its floor. Index 0 is Name's trailing boundary.
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

    /// Record what a drag took hold of. `grab` is the pointer's offset inside the cell for a
    /// reorder and is unused for a resize, where the pointer's absolute `origin` is what matters.
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

    /// Every pointer move while a header drag is live. gpui delivers these wherever the pointer is,
    /// so a drag that leaves the band keeps tracking, as AppKit's own does.
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
                // gpui measures the grab offset on the move that crosses its 2pt drag threshold, not
                // on the press, so a pointer that jumps far in one event reports an offset outside
                // the cell it started in. A real mouse cannot, but a synthetic gesture or a fast
                // flick can, and an unbounded offset puts the carried cell nowhere near the pointer
                // and picks the wrong slot.
                let grab = grab.clamp(0., width);
                // The fixed block starts at Name's trailing boundary and its extent no longer
                // depends on the pane, so the carried cell is clamped against the block rather than
                // against the pane's trailing edge. Clamping against the pane is what let a drag in
                // a narrow pane push a column outside the block it belongs to.
                let start = self.name_w;
                let left = (x - f32::from(event.bounds.origin.x) - grab)
                    .clamp(start, (start + total - width).max(start));

                // The target is chosen against the layout of the *other* columns, which does not
                // change for the whole drag. Choosing it against the live layout makes the columns
                // oscillate: a narrow column dropped past a wide one lands back under the pointer,
                // which immediately re-targets the slot it just left.
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
                // The boundary follows the pointer, so the column it is the trailing edge of takes
                // the whole delta and every column after it slides. Nothing is traded with a
                // neighbour: with the block anchored at the pane's leading edge, trading would move
                // a boundary the pointer is not on.
                let next = (width + x - origin).max(self.resized_column(index).1);
                match index {
                    0 => self.name_w = next,
                    index => {
                        if let Some(spec) = self.list_columns.get_mut(index - 1) {
                            spec.width = next;
                        }
                    }
                }
                cx.notify();
            }
            None => {}
        }
    }
}
