//! The column header band, and the column geometry the list agrees with it on.
//!
//! Owns `COLUMNS` and the three fixed column widths, the paddings a cell and the pane are inset
//! by, and `FinderDemo::header_row`.
//!
//! Deliberately not here: the rows themselves. `list` imports the widths from this module on
//! purpose — the header and the rows are laid out by different functions, and a width written down
//! in only one of them is exactly how the two stop lining up.

use gpui::{AnyElement, ClickEvent, FontWeight, SharedString, div, prelude::*, px};
use native_gpui::tokens::{self, SystemColor};

use crate::FinderDemo;
use crate::data::Column;
use crate::palette::Palette;
use crate::ui::small_glyph;

/// Column widths as Finder *draws* them, which are not the widths it stores — Name is stored 178
/// and drawn 197. Named rather than repeated, because the header and the rows are laid out in
/// different functions and a literal in only one of them is exactly how the two stop lining up.
/// Name takes whatever is left, so the pane can be any width and the rest still land together.
pub(crate) const COL_MODIFIED: f32 = 181.;
pub(crate) const COL_SIZE: f32 = 75.;
pub(crate) const COL_KIND: f32 = 121.;

pub(crate) const COLUMNS: &[(Column, &str, f32)] = &[
    (Column::Name, "Name", 0.),
    (Column::Modified, "Date Modified", COL_MODIFIED),
    (Column::Size, "Size", COL_SIZE),
    (Column::Kind, "Kind", COL_KIND),
];

/// Finder's column header band. Its own bottom hairline is the only line in the chrome above the
/// list: there is none under the toolbar, which shares one continuous background with the header.
pub(crate) const HEADER_H: f32 = 28.;

/// The columns region is inset this far from both pane edges. Both the selection *and* the
/// alternating row band span exactly it, as one rounded shape — neither runs the full pane width.
pub(crate) const PANE_INSET: f32 = 10.;

/// The header's label inset does not follow the icon size the way a row's does: at 16pt icons the
/// two coincide at 46, and at 32pt they do not.
pub(crate) const HEADER_LABEL_X: f32 = 46.;
/// A header separator is 1pt wide and 24 of the band's 28pt — inset from the top, flush to the
/// bottom, where it runs into the hairline.
pub(crate) const SEP_TOP: f32 = 4.;
/// Text inset inside a fixed column. Size measures it from the trailing edge instead, which is how
/// three different strings there come out sharing one right edge.
pub(crate) const CELL_PAD: f32 = 6.;
/// The sort indicator's inset from the sorted column's trailing edge.
pub(crate) const SORT_INSET: f32 = 7.5;

impl FinderDemo {
    /// The column header. 28pt, one hairline along its own bottom edge and none under the toolbar,
    /// and a 1pt separator standing at each interior column boundary — never at the leading edge of
    /// Name or the trailing edge of Kind.
    ///
    /// The sorted column's label is both bolder and brighter than the rest: 11pt semibold in
    /// `labelColor` against 11pt regular in `secondaryLabelColor`.
    pub(crate) fn header_row(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .relative()
            .font_family(tokens::system_font_family())
            .text_size(tokens::small_system_font_size())
            .text_color(palette.secondary)
            // Finder needs no fill here: its toolbar band is already the table's colour, so the
            // header, the rows and the chrome above them read as one surface. This window's
            // backdrop is a material that measures 67 in dark against Finder's 29, so without
            // this the header would be the one part of the list that is visibly not the list.
            .bg(palette.surface);

        let last = COLUMNS.len() - 1;
        for (index, (column, title, width)) in COLUMNS.iter().enumerate() {
            let active = *column == self.sort;
            let column = *column;
            let mut cell = div()
                .id(SharedString::from(format!("header-{title}")))
                .h_full()
                .flex()
                .items_center()
                .relative()
                .overflow_hidden()
                .whitespace_nowrap()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if this.sort == column {
                        this.ascending = !this.ascending;
                    } else {
                        this.sort = column;
                        this.ascending = true;
                    }
                    cx.notify();
                }));
            cell = if *width > 0. {
                cell.w(px(*width))
            } else {
                cell.flex_1().min_w(px(120.))
            };
            // A leading spacer rather than padding: the separator and the sort indicator are
            // positioned from this cell's own edges, and padding would move what those edges mean.
            let inset = if index == 0 { HEADER_LABEL_X } else { CELL_PAD };
            cell = cell.child(div().w(px(inset)).flex_none());
            // Finder left-aligns the Size *header* while right-aligning its values, so this one
            // deliberately does not mirror the row. The label is not centred in the band either:
            // measured 1pt above centre, which a bottom margin buys without moving the cell's own
            // edges — the separator and the sort indicator are positioned from those.
            let label = div().mb(px(2.));
            cell = cell.child(if active {
                label
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.label)
                    .child(*title)
            } else {
                label.child(*title)
            });
            if active {
                let arrow = if self.ascending {
                    "chevron.up"
                } else {
                    "chevron.down"
                };
                if let Some(glyph) = small_glyph(arrow, 9., SystemColor::SecondaryLabel) {
                    cell = cell.child(
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
            if index < last {
                cell = cell.child(
                    div()
                        .absolute()
                        .right_0()
                        .bottom_0()
                        .w(px(1.))
                        .h(px(HEADER_H - SEP_TOP))
                        .bg(palette.separator),
                );
            }
            row = row.child(cell);
        }
        row.child(div().w(px(PANE_INSET)).flex_none())
            // The hairline is drawn rather than bordered so the band keeps its full 28pt: a bottom
            // border would take the pt back out of the content box and shorten every separator.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(1.))
                    .bg(palette.separator),
            )
            .into_any_element()
    }
}
