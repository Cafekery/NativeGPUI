//! The column (browser) view.

use std::collections::HashSet;

use gpui::{
    AnyElement, ClickEvent, Hsla, Modifiers, SharedString, Window, div, opaque_grey, prelude::*, px,
};
use native_gpui::tokens::{Appearance, SystemColor, current_appearance, system_color};

use crate::data::Line;
use crate::palette::Palette;
use crate::ui::{artwork, rename_field, small_glyph};
use crate::{COLUMN_VIEW, FinderDemo, LIST_VIEW};

const BROWSER_ROW_H: f32 = 24.;

const COL_W: f32 = 164.;

const COL_PREVIEW_W: f32 = 240.;

const COL_PREVIEW_INSET: f32 = 10.;

const COL_ICON_X: f32 = 17.;
const COL_ICON: f32 = 16.;
const COL_LABEL_X: f32 = 39.5;

const COL_CHEVRON_TRAIL: f32 = 16.5;
const COL_CHEVRON: f32 = 7.;

const COL_PILL_INSET: f32 = 10.;
const COL_PILL_RADIUS: f32 = 7.;

const COL_TOP_GAP: f32 = 6.;

struct Rung {
    folder: &'static str,
    selected: Option<SharedString>,
}

fn divider() -> Hsla {
    match current_appearance() {
        Appearance::Dark => opaque_grey(0., 1.),
        Appearance::Light => opaque_grey(230. / 255., 1.),
    }
}

impl FinderDemo {
    /// Columns from the top of the tree down to the cursor, root first, plus the leaf row when it
    /// opens nothing. Finder's browser shows the whole path: a folder sits selected in its
    /// parent's column with its contents in the next.
    fn column_chain(&self) -> (Vec<Rung>, Option<Line>) {
        let root = || {
            (
                vec![Rung {
                    folder: self.here(),
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

        let mut up = vec![holder];
        while let Some(parent) = self
            .store
            .get(*up.last().expect("seeded, and only pushed to"))
            .and_then(|folder| folder.parent)
        {
            if up.len() >= self.store.len() {
                return root();
            }
            up.push(parent);
        }
        up.reverse();

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

        // One folder opens its column; one file, or several of anything, the preview column.
        match row.opens.filter(|_| self.selected.len() == 1) {
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

    /// Finder opens a folder in column view with the folder itself selected in its parent's
    /// column, so the keys start from there.
    fn column_seed(&mut self) {
        let here = self.here();
        let Some(parent) = self.store.get(here).and_then(|folder| folder.parent) else {
            return;
        };
        let listing = self.column_listing(parent);
        if let Some(line) = listing
            .iter()
            .find(|line| line.row.opens == Some(here))
            .cloned()
        {
            self.pick_in(&listing, &line, false, false);
        }
    }

    fn browser_column(
        &self,
        index: usize,
        rung: &Rung,
        emphasized: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lines = self.column_listing(rung.folder);
        let folder = rung.folder;
        let keys: Vec<SharedString> = lines.iter().map(|line| line.key.clone()).collect();
        // The focused column shows the whole selection; an ancestor shows the one row on the path.
        let picked: HashSet<SharedString> = if emphasized {
            self.selected.clone()
        } else {
            rung.selected.iter().cloned().collect()
        };

        let this = cx.entity().downgrade();
        let renaming = self.renaming.clone();
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
                let is_selected = picked.contains(&line.key);
                // Adjacent selected rows share one pill, as Finder's do.
                let joins_above = row > 0 && picked.contains(&keys[row - 1]);
                let joins_below = keys.get(row + 1).is_some_and(|key| picked.contains(key));
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
                            .when(!joins_above, |pill| {
                                pill.rounded_tl(px(COL_PILL_RADIUS))
                                    .rounded_tr(px(COL_PILL_RADIUS))
                            })
                            .when(!joins_below, |pill| {
                                pill.rounded_bl(px(COL_PILL_RADIUS))
                                    .rounded_br(px(COL_PILL_RADIUS))
                            })
                            .bg(fill),
                    );
                }
                let name = div()
                    .ml(px(COL_LABEL_X - COL_ICON_X - COL_ICON))
                    .flex_1()
                    .overflow_hidden();
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
                    .child(if renaming.as_ref() == Some(&line.key) {
                        name.child(rename_field(&line, row, this.clone()))
                    } else {
                        name.text_ellipsis()
                            .whitespace_nowrap()
                            .child(line.row.name.clone())
                    });
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
                    let (extend, toggle, twice) = match event {
                        ClickEvent::Mouse(mouse) => (
                            mouse.up.modifiers.shift,
                            mouse.up.modifiers.platform,
                            mouse.up.click_count >= 2,
                        ),
                        _ => (false, false, false),
                    };
                    let line = clicked.clone();
                    this.update(cx, |this: &mut FinderDemo, cx| {
                        let listing = this.column_listing(folder);
                        this.pick_in(&listing, &line, extend, toggle);
                        if twice {
                            this.open(&line, window, cx);
                        }
                        this.column_arrived(window, cx);
                    })
                    .ok();
                })
            }))
            .into_any_element()
    }

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
                &cx.entity(),
            ))
            .into_any_element()
    }

    pub(crate) fn column_body(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.cursor.is_none() {
            self.column_seed();
        }
        let (chain, leaf) = self.column_chain();
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

    /// Arrow keys in column view, measured on Finder: Up and Down walk the focused column and
    /// extend with Shift; Right takes the first row of a folder, nothing of a file, and the next
    /// row of a multiple selection; Left selects the folder the column belongs to. Claims all
    /// four even on a no-op, so `on_key` cannot disclose here.
    pub(crate) fn column_key(
        &mut self,
        key: &str,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // ⌘-arrows are not ours: ⌘↑ is Go to Enclosing Folder and ⌘↓ opens the selection.
        if modifiers.platform || !matches!(key, "up" | "down" | "left" | "right") {
            return false;
        }
        let (chain, _) = self.column_chain();
        let Some(focused) = chain.iter().rposition(|rung| rung.selected.is_some()) else {
            if matches!(key, "down" | "right") {
                self.column_take_first(chain[0].folder);
            }
            self.column_arrived(window, cx);
            return true;
        };
        let lines = self.column_listing(chain[focused].folder);
        let at = chain[focused]
            .selected
            .as_ref()
            .and_then(|key| lines.iter().position(|line| &line.key == key));
        match (key, at) {
            ("up" | "down", Some(at)) => {
                let next = if key == "up" {
                    at.saturating_sub(1)
                } else {
                    (at + 1).min(lines.len() - 1)
                };
                let line = lines[next].clone();
                self.pick_in(&lines, &line, modifiers.shift, false);
            }
            ("right", Some(at)) if self.selected.len() > 1 => {
                let line = lines[(at + 1).min(lines.len() - 1)].clone();
                self.pick_in(&lines, &line, false, false);
            }
            ("right", Some(at)) => {
                if let Some(into) = lines[at].row.opens {
                    self.column_take_first(into);
                }
            }
            ("left", _) => self.column_up(),
            _ => {}
        }
        self.column_arrived(window, cx);
        true
    }

    /// Left and ⌘↑: the folder holding the selection, selected in its own column.
    pub(crate) fn column_up(&mut self) {
        let Some(holder) = self
            .cursor
            .as_ref()
            .and_then(|key| key.rsplit_once('/'))
            .map(|(holder, _)| holder.to_owned())
        else {
            return;
        };
        let Some(parent) = self
            .store
            .get(holder.as_str())
            .and_then(|folder| folder.parent)
        else {
            return;
        };
        let listing = self.column_listing(parent);
        if let Some(line) = listing
            .iter()
            .find(|line| line.row.opens.is_some_and(|id| id == holder))
            .cloned()
        {
            self.pick_in(&listing, &line, false, false);
        }
    }

    /// After the selection moves: the title, the sidebar and the history follow the target, as
    /// Finder's do, so Back returns to the folder last shown.
    pub(crate) fn column_arrived(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.target();
        if target != self.here() {
            self.history.truncate(self.at + 1);
            self.history.push(target);
            self.at = self.history.len() - 1;
        }
        self.retitle(window);
        self.sidebar_follow();
        cx.notify();
    }

    fn column_take_first(&mut self, folder: &'static str) {
        let listing = self.column_listing(folder);
        if let Some(line) = listing.first().cloned() {
            self.pick_in(&listing, &line, false, false);
        }
    }

    fn column_listing(&self, folder: &'static str) -> Vec<Line> {
        let mut lines = Vec::new();
        self.collect(folder, 0, false, &mut lines);
        lines
    }

    /// The rows the keyboard works on: the focused browser column here, else the view's own.
    pub(crate) fn focused_lines(&self) -> Vec<Line> {
        if self.view != COLUMN_VIEW {
            return self.lines(self.view == LIST_VIEW);
        }
        let (chain, _) = self.column_chain();
        let focused = chain
            .iter()
            .rposition(|rung| rung.selected.is_some())
            .unwrap_or(0);
        self.column_listing(chain[focused].folder)
    }
}
