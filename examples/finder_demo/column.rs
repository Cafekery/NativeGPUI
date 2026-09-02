//! The column (browser) view.

use gpui::{
    AnyElement, ClickEvent, Hsla, Modifiers, SharedString, div, opaque_grey, prelude::*, px,
};
use native_gpui::tokens::{Appearance, SystemColor, current_appearance, system_color};

use crate::FinderDemo;
use crate::data::Line;
use crate::palette::Palette;
use crate::ui::{artwork, small_glyph};

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
    /// Columns descending from the cursor, root first, plus the leaf row when it opens nothing.
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

    /// Arrow keys in column view. Claims all four even on a no-op, so `on_key` cannot disclose here.
    pub(crate) fn column_key(
        &mut self,
        key: &str,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        // ⌘-arrows are not ours: ⌘↑ is Go to Enclosing Folder and ⌘↓ opens the selection.
        if modifiers.platform || !matches!(key, "up" | "down" | "left" | "right") {
            return false;
        }
        let (chain, _) = self.column_chain();
        match chain.iter().rposition(|rung| rung.selected.is_some()) {
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

    fn column_take_first(&mut self, folder: &'static str, cx: &mut Context<Self>) {
        if let Some(line) = self.column_listing(folder).first().cloned() {
            self.pick(&line, false, false);
            cx.notify();
        }
    }

    fn column_listing(&self, folder: &'static str) -> Vec<Line> {
        let mut lines = Vec::new();
        self.collect(folder, 0, false, &mut lines);
        lines
    }
}
