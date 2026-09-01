//! The column (browser) view.
//!
//! Owns `FinderDemo::column_body`, the chain of columns that descends from the selection, the one
//! column that chain is built out of, and this view's own row metrics.
//!
//! Deliberately not here: the list view's row metrics. Both views have now been measured against a
//! real window and they came out different — 24pt rows against the list's 36, a 7pt pill radius
//! against its 8, offsets from the column's leading edge rather than from the pane's, and a divider
//! that is black where the list's is `separatorColor`. Importing either set into the other would be
//! wrong in every number.

use gpui::{
    AnyElement, ClickEvent, Hsla, Modifiers, SharedString, div, opaque_grey, prelude::*, px,
};
use native_gpui::tokens::{Appearance, SystemColor, current_appearance, system_color};

use crate::FinderDemo;
use crate::data::Line;
use crate::palette::Palette;
use crate::ui::{artwork, small_glyph};

/// Row pitch and row height are the same 24pt: the selection pill fills its row top to bottom with
/// no vertical inset, so there is no second number to carry.
///
/// Three routes agree on the twenty-four. Finder's own column view measures a 24.0pt pitch, over
/// three separate columns, with a pill exactly 24.0pt tall; a stock `NSBrowser` reports `rowHeight`
/// 24.0; and the accessibility tree says Finder's column view *is* an `AXBrowser`, which is what
/// makes the stock figure the same fact rather than a coincidence. Reading `NSBrowser.rowHeight`
/// needs an item-based delegate installed first — against a matrix delegate it raises
/// `NSInternalInconsistencyException` instead of returning a default.
const BROWSER_ROW_H: f32 = 24.;

/// Boundary to boundary, so a column is 163pt of content plus the 1pt divider on its trailing edge.
/// Measured as an exact 164.0 three times in one window and twice in another.
///
/// Finder's columns are not all this wide: the same captures show one at 341pt, and another window
/// one at 205pt. So this is the standard width rather than a rule, and what produces a wider one is
/// unmeasured — an auto-fit invented to explain them would be a guess wearing a measurement's
/// clothes. What the spread does rule out is AppKit's automatic mode, which puts every column back
/// to the same width on the next layout pass.
const COL_W: f32 = 164.;

/// The trailing preview column, boundary to boundary: 239pt of content plus its leading divider.
///
/// Deliberately not `PREVIEW_W`, which is 240 for the pane beside the *list*. Two measurements that
/// happen to agree numerically is exactly where sharing one constant becomes a trap — the list's
/// pane is per-window state that a drag moves, while this column measured 240 in both windows it
/// has been seen in.
const COL_PREVIEW_W: f32 = 240.;

/// The text inset the preview column shares with the pane beside the list, which is why the card
/// takes a content width rather than a total one. Confirmed off the capture rather than adopted:
/// the preview column's thumbnail measures 219pt across, which is this column's content less two
/// of these.
const COL_PREVIEW_INSET: f32 = 10.;

/// Both measured from the column's own leading edge, never from the pane's. The icon is a 16pt box:
/// the artwork ships an exact representation at that size, so it lands crisp rather than resampled.
const COL_ICON_X: f32 = 17.;
const COL_ICON: f32 = 16.;
const COL_LABEL_X: f32 = 39.5;

/// Where the chevron's *ink* ends, in from the column's trailing edge. The glyph is small: 3.5pt of
/// ink across and 6.5 down, at the grey `secondaryLabelColor` resolves to.
///
/// A symbol is laid out by its box and not by its ink, so the box is placed 1.75pt further out —
/// half the difference between a 7pt box and 3.5pt of ink. That step assumes the ink sits centred
/// in the box, which is the one thing here that is reasoned rather than measured.
const COL_CHEVRON_TRAIL: f32 = 16.5;
const COL_CHEVRON: f32 = 7.;

/// The pill is inset from both of the column's edges and is the full row height. Its radius came
/// off the corner's inset-per-scanline profile rather than from an area estimate: a circular 7.0
/// fits five separate depths to within a tenth of a point, where the area conversion the list view
/// had to calibrate reads about a point low.
const COL_PILL_INSET: f32 = 10.;
const COL_PILL_RADIUS: f32 = 7.;

/// The first row starts this far below the top of the column body.
const COL_TOP_GAP: f32 = 6.;

/// One column of the chain: the folder it lists, and the row highlighted inside it if any.
struct Rung {
    folder: &'static str,
    selected: Option<SharedString>,
}

/// The 1pt rule on a column's trailing edge, which is **not** `separatorColor`.
///
/// Measured (0,0,0) opaque in Finder — alpha 255, so a real fill rather than a hole the desktop
/// shows through — in eight instances across two separate windows, and (0,0,0) again from a stock
/// `NSBrowser` rendered offscreen. `separatorColor` over this body would read 51, which is what
/// the *list* view's divider measures, so the two views genuinely differ here. It runs from the
/// body's top edge to the bottom of the window, which is why it belongs to the column rather than
/// to the rows.
///
/// The light value is the stock browser's rather than Finder's: the reference window was dark, and
/// the system appearance is not this demo's to flip.
fn divider() -> Hsla {
    match current_appearance() {
        Appearance::Dark => opaque_grey(0., 1.),
        Appearance::Light => opaque_grey(230. / 255., 1.),
    }
}

impl FinderDemo {
    /// The chain of columns descending from the cursor, root first, and the cursor's own row when
    /// it opens nothing — which is what decides whether a preview column closes the chain.
    ///
    /// Derived from the cursor rather than stored, because the chain *is* the cursor's ancestry:
    /// the row highlighted in each column is the one that opened the next. That leaves nothing to
    /// keep in sync — every way the cursor can move, the keyboard included, moves the chain with it.
    ///
    /// A `Line` key is `{folder}/{name}` and neither half ever contains a slash, so the split is
    /// exact. It is still checked against the store rather than trusted, and a key that fails to
    /// resolve falls back to the current folder alone instead of drawing a chain built on a guess.
    fn column_chain(&self) -> (Vec<Rung>, Option<Line>) {
        let here = self.here();
        let root = || {
            (
                vec![Rung {
                    folder: here,
                    selected: None,
                }],
                None,
            )
        };
        let Some(cursor) = self.cursor.clone() else {
            return root();
        };
        let Some((holder, name)) = cursor.rsplit_once('/') else {
            return root();
        };
        let Some((holder, row)) = self.store.get_key_value(holder).and_then(|(id, folder)| {
            folder
                .rows
                .iter()
                .find(|row| row.name == name)
                .map(|row| (*id, row))
        }) else {
            return root();
        };

        // Up to the folder the window is showing. Bounded by the store's size, so a parent link
        // that ever pointed into a cycle cannot spin here.
        let mut up = vec![holder];
        while *up.last().expect("seeded with the cursor's folder") != here {
            let Some(parent) = self
                .store
                .get(*up.last().expect("seeded, and only pushed to"))
                .and_then(|folder| folder.parent)
            else {
                return root();
            };
            if up.len() >= self.store.len() {
                return root();
            }
            up.push(parent);
        }
        up.reverse();

        // Each column's highlighted row is the one that opens the column after it; the last one's
        // is the cursor itself.
        let mut chain: Vec<Rung> = up
            .iter()
            .copied()
            .enumerate()
            .map(|(index, folder)| Rung {
                folder,
                selected: match up.get(index + 1) {
                    Some(next) => self.store.get(folder).and_then(|listing| {
                        listing
                            .rows
                            .iter()
                            .find(|candidate| candidate.opens == Some(*next))
                            .map(|found| SharedString::from(format!("{folder}/{}", found.name)))
                    }),
                    None => Some(cursor.clone()),
                },
            })
            .collect();

        // A folder highlighted in the last column opens one more, with nothing selected in it; a
        // file instead closes the chain with the preview column.
        match row.opens {
            Some(into) => {
                chain.push(Rung {
                    folder: into,
                    selected: None,
                });
                (chain, None)
            }
            None => (
                chain,
                Some(Line {
                    key: cursor,
                    row: row.clone(),
                    depth: 0,
                }),
            ),
        }
    }

    /// One column of the browser view: a flat listing that reports clicks back through `pick`.
    ///
    /// `emphasized` carries the whole of the selection carry-through. Finder keeps every ancestor's
    /// row highlighted so the path stays readable, and separates them by colour rather than by
    /// dropping them: the ancestors take `unemphasizedSelectedContentBackground` and only the column
    /// holding the cursor takes the vivid one. The discriminator that proves it is which of the two
    /// moves when the window stops being key — the ancestors' grey does not, the leaf's blue does.
    fn browser_column(
        &self,
        index: usize,
        rung: &Rung,
        emphasized: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut lines = Vec::new();
        self.collect(rung.folder, 0, false, &mut lines);

        let this = cx.entity().downgrade();
        let selected = rung.selected.clone();
        let label = palette.label;
        let fill = if emphasized {
            palette.selection
        } else {
            system_color(SystemColor::UnemphasizedSelectedContentBackground)
        };
        let selection_text = if emphasized {
            palette.selection_text
        } else {
            palette.label
        };

        div()
            .id(("ng.finder.column", index))
            .w(px(COL_W - 1.))
            .flex_none()
            .flex()
            .flex_col()
            .pt(px(COL_TOP_GAP))
            .overflow_y_scroll()
            .children(lines.into_iter().enumerate().map(|(row, line)| {
                let is_selected = selected.as_ref() == Some(&line.key);
                let this = this.clone();
                let clicked = line.clone();
                let mut cell = div()
                    .id(("ng.finder.column.row", row))
                    .h(px(BROWSER_ROW_H))
                    .relative()
                    .flex()
                    .items_center()
                    .pl(px(COL_ICON_X))
                    .pr(px(COL_CHEVRON_TRAIL - (COL_CHEVRON - 3.5) / 2.))
                    .text_color(if is_selected { selection_text } else { label });
                if is_selected {
                    cell = cell.child(
                        div()
                            .absolute()
                            .left(px(COL_PILL_INSET))
                            .right(px(COL_PILL_INSET))
                            .top_0()
                            .bottom_0()
                            .rounded(px(COL_PILL_RADIUS))
                            .bg(fill),
                    );
                }
                cell = cell
                    .child(
                        div()
                            .w(px(COL_ICON))
                            .h(px(COL_ICON))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(artwork(&line.row, COL_ICON)),
                    )
                    .child(
                        div()
                            .ml(px(COL_LABEL_X - COL_ICON_X - COL_ICON))
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(line.row.name.clone()),
                    );
                // Folders only, and decided by what the row opens rather than by what it is: a
                // bundle is a directory too, and Finder still gives it no chevron.
                if line.row.opens.is_some() {
                    let color = if is_selected && emphasized {
                        SystemColor::AlternateSelectedControlText
                    } else {
                        SystemColor::SecondaryLabel
                    };
                    if let Some(arrow) = small_glyph("chevron.right", COL_CHEVRON, color) {
                        cell = cell.child(div().flex_none().ml(px(4.)).child(arrow));
                    }
                }
                cell.on_click(move |event: &ClickEvent, window, cx| {
                    let twice =
                        matches!(event, ClickEvent::Mouse(mouse) if mouse.up.click_count >= 2);
                    let line = clicked.clone();
                    this.update(cx, |this: &mut FinderDemo, cx| {
                        this.pick(&line, false, false);
                        if twice {
                            this.open(&line, window, cx);
                        }
                        cx.notify();
                    })
                    .ok();
                })
            }))
            .into_any_element()
    }

    /// A column, and the rule that separates it from the next one.
    ///
    /// `last` carries no rule. Finder's dividers sit *between* columns and not on each one: a
    /// five-column chain measures four of them, at 293/457/621/785 with content running 786…1025
    /// after the last. The 1pt black at the window's own trailing edge in those captures is the
    /// window, not a divider — it spans the toolbar band too, where no column reaches.
    fn ruled(&self, column: AnyElement, last: bool) -> AnyElement {
        div()
            .flex_none()
            .flex()
            .child(column)
            .when(!last, |wrap| {
                wrap.child(div().w(px(1.)).flex_none().bg(divider()))
            })
            .into_any_element()
    }

    /// The preview column that closes the chain when the cursor is a file. The card is the pane's,
    /// not this view's: same content, one implementation, and it takes the width inside the insets.
    fn preview_column(&self, line: &Line, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("ng.finder.column.preview")
            .w(px(COL_PREVIEW_W - 1.))
            .flex_none()
            .flex()
            .flex_col()
            .child(self.info_card(
                Some(line),
                palette,
                COL_PREVIEW_W - 1. - 2. * COL_PREVIEW_INSET,
                cx,
            ))
            .into_any_element()
    }

    pub(crate) fn column_body(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let (chain, leaf) = self.column_chain();
        // The emphasized column is the last one carrying a highlight. With a folder selected the
        // chain has one more rung than that — the folder's own contents, nothing chosen in it yet —
        // so taking the last rung would put the vivid pill on an empty column.
        let focused = chain.iter().rposition(|rung| rung.selected.is_some());

        let last = chain.len() - 1;
        let mut columns: Vec<AnyElement> = chain
            .iter()
            .enumerate()
            .map(|(index, rung)| {
                let column = self.browser_column(index, rung, Some(index) == focused, palette, cx);
                self.ruled(column, index == last && leaf.is_none())
            })
            .collect();
        if let Some(leaf) = &leaf {
            columns.push(self.ruled(self.preview_column(leaf, palette, cx), true));
        }

        // Only when the chain grew or shrank. `scroll_to_item` moves the offset the minimal amount
        // needed to bring a child into view, so re-requesting it every frame would keep dragging
        // the chain back rightwards under a user who had scrolled left to see an ancestor.
        let depth = columns.len();
        if depth != self.column_depth {
            self.column_depth = depth;
            self.column_scroll.scroll_to_item(depth - 1);
        }

        div()
            .id("ng.finder.columns")
            .flex_1()
            .flex()
            .overflow_x_scroll()
            .track_scroll(&self.column_scroll)
            .children(columns)
            .into_any_element()
    }

    /// Arrow keys in column view, which are not the list view's.
    ///
    /// `on_key`'s own ↑/↓ walk the flat listing of the folder the *window* is showing — the first
    /// column — so once the cursor is deeper than that they find it nowhere in their list and snap
    /// it back to the top of column one. ↑/↓ therefore have to be answered here, against the
    /// focused column's own listing, or descending would be a one-way trip.
    ///
    /// ←/→ move between columns, which has no analogue in the list view: → descends into the
    /// highlighted folder and takes its first row, ← climbs back to the folder that opened the
    /// column being left. `on_key` maps them to the disclosure triangles, which this view has none
    /// of.
    ///
    /// All four are claimed **whether or not they move anything**, and the return value says only
    /// whether the key belongs to this view. Falling through on a no-op — ← in the leftmost column,
    /// → on a file — would reach `on_key`'s disclosure arm and quietly open or close a triangle in
    /// the *list* view, which shares `expanded` and would show the change after a view switch.
    ///
    /// A **⌘**-arrow is not ours, and the first version of this claimed it. ⌘↑ is Go to Enclosing
    /// Folder and ⌘↓ opens the selection; swallowing them left column view with no way out of the
    /// folder it started in, which is how it was found — the only route up was to switch to list
    /// view and press ⌘↑ there.
    ///
    /// The guard is ⌘ specifically, not "any modifier". Every other modified arrow still belongs
    /// here, because `on_key`'s plain ↑/↓ arm reads `modifiers.shift` to extend a selection across
    /// the *window folder's* listing — so declining ⇧↑ would hand it the same wrong-column defect
    /// this method exists to fix, and a stock browser has `allowsMultipleSelection` false, so there
    /// is no extend to hand it in the first place. `Modifiers::modified()` would also be wrong for
    /// a second reason: it counts `function`, which is set on a Mac's arrow keys.
    pub(crate) fn column_key(
        &mut self,
        key: &str,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        if modifiers.platform || !matches!(key, "up" | "down" | "left" | "right") {
            return false;
        }
        let (chain, _) = self.column_chain();
        match chain.iter().rposition(|rung| rung.selected.is_some()) {
            // Nothing chosen yet: ↓ and → both mean the top of the first column, ↑ and ← have
            // nowhere to go.
            None => {
                if matches!(key, "down" | "right") {
                    self.column_take_first(chain[0].folder, cx);
                }
            }
            Some(focused) => match key {
                "up" | "down" => {
                    let lines = self.column_listing(chain[focused].folder);
                    let step: isize = if key == "up" { -1 } else { 1 };
                    let at = chain[focused]
                        .selected
                        .as_ref()
                        .and_then(|key| lines.iter().position(|line| &line.key == key));
                    if let Some(at) = at {
                        let next = (at as isize + step).clamp(0, lines.len() as isize - 1) as usize;
                        let line = lines[next].clone();
                        self.pick(&line, false, false);
                        cx.notify();
                    }
                }
                "right" => {
                    let into = chain[focused]
                        .selected
                        .as_ref()
                        .and_then(|key| {
                            self.column_listing(chain[focused].folder)
                                .into_iter()
                                .find(|line| &line.key == key)
                        })
                        .and_then(|line| line.row.opens);
                    if let Some(into) = into {
                        self.column_take_first(into, cx);
                    }
                }
                _ => {
                    let previous = focused
                        .checked_sub(1)
                        .and_then(|parent| chain[parent].selected.clone().map(|key| (parent, key)))
                        .and_then(|(parent, key)| {
                            self.column_listing(chain[parent].folder)
                                .into_iter()
                                .find(|line| line.key == key)
                        });
                    if let Some(line) = previous {
                        self.pick(&line, false, false);
                        cx.notify();
                    }
                }
            },
        }
        true
    }

    /// Put the cursor on the first row of `folder`, which is how → and ↓-into-nothing both open a
    /// column. An empty folder still opens one, it just has nothing to take.
    fn column_take_first(&mut self, folder: &'static str, cx: &mut Context<Self>) {
        if let Some(line) = self.column_listing(folder).first().cloned() {
            self.pick(&line, false, false);
            cx.notify();
        }
    }

    /// One column's rows, in the current sort order.
    fn column_listing(&self, folder: &'static str) -> Vec<Line> {
        let mut lines = Vec::new();
        self.collect(folder, 0, false, &mut lines);
        lines
    }
}
