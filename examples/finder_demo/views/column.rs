//! The column (browser) view.

use gpui::{
    AnyElement, ClickEvent, DispatchPhase, FontWeight, Hsla, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString,
    UniformListScrollHandle, Window, canvas, div, opaque_grey, prelude::*, px, uniform_list,
};
use native_gpui::tokens::{Appearance, SystemColor, current_appearance, system_color};

use crate::app::FinderDemo;
use crate::art::{art_at, row_art, small_glyph};
use crate::data::{Face, Line};
use crate::menus::rows_menu;
use crate::palette::Palette;
use crate::prefs;
use crate::ui::{click_kind, rename_field, scroller, text_width};

const ROW_H: f32 = 24.;
const COL_W: f32 = 164.;
const PREVIEW_W: f32 = 240.;
const PREVIEW_INSET: f32 = 10.;
const ICON_X: f32 = 17.;
const ICON: f32 = 16.;
const LABEL_X: f32 = 39.5;
const CHEVRON_TRAIL: f32 = 16.5;
const CHEVRON: f32 = 7.;
const PILL_INSET: f32 = 10.;
const PILL_RADIUS: f32 = 7.;
const TOP_GAP: f32 = 6.;
const GRIP_W: f32 = 7.;
const COL_MIN: f32 = 100.;

pub(crate) struct Rung {
    pub(crate) folder: &'static str,
    pub(crate) selected: Option<SharedString>,
}

fn divider() -> Hsla {
    match current_appearance() {
        Appearance::Dark => opaque_grey(0., 1.),
        Appearance::Light => opaque_grey(230. / 255., 1.),
    }
}

impl FinderDemo {
    pub(crate) fn column_chain(&self) -> (Vec<Rung>, Option<Line>) {
        let root = || {
            (
                vec![Rung {
                    folder: self.here(),
                    selected: None,
                }],
                None,
            )
        };
        let Some((cursor, holder, row)) = self.cursor.clone().and_then(|cursor| {
            let (holder, name) = crate::data::split_key(&cursor)?;
            let folder = self.folders.get(holder)?;
            let row = folder.rows.iter().find(|row| row.name == name)?.clone();
            Some((cursor, holder, row))
        }) else {
            return root();
        };

        let mut up = vec![holder];
        while let Some(parent) = self
            .folders
            .get(up[up.len() - 1])
            .and_then(|folder| folder.parent)
        {
            if up.len() > self.folders.len() {
                return root();
            }
            up.push(parent);
        }
        up.reverse();

        let mut chain: Vec<Rung> = up
            .iter()
            .enumerate()
            .map(|(index, &folder)| Rung {
                folder,
                selected: match up.get(index + 1) {
                    Some(next) => self.folders.get(folder).and_then(|listing| {
                        listing
                            .rows
                            .iter()
                            .find(|row| row.opens == Some(*next))
                            .map(|row| row.path.clone())
                    }),
                    None => Some(cursor.clone()),
                },
            })
            .collect();
        let alone = self.selected.len() <= 1;
        match row.opens.filter(|_| alone).or(match row.face {
            Face::Folder if alone => Some(""),
            _ => None,
        }) {
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
                    row,
                    depth: 0,
                }),
            ),
        }
    }

    pub(crate) fn column_seed(&mut self) {
        let here = self.here();
        let Some(parent) = self.folders.get(here).and_then(|folder| folder.parent) else {
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
        let folder = rung.folder;
        let lines = self.column_listing(folder);
        let scale = self.scale;
        let handle = self.column_scrolls[index].clone();
        let this = cx.entity().downgrade();
        let this_menu = this.clone();
        let hit_scroll = handle.clone();
        let hit = move |demo: &FinderDemo, at: Point<Pixels>| {
            let top = f32::from(hit_scroll.0.borrow().base_handle.offset().y);
            let row = (f32::from(at.y) - top - TOP_GAP) / ROW_H;
            (row >= 0.)
                .then(|| demo.column_listing(folder).get(row as usize).cloned())
                .flatten()
        };
        let renaming = self.renaming.clone();
        let cursor = self.cursor.clone();
        let ql_icon = self.ql_icon.clone();
        let chosen = self.selected.snapshot();
        let alone = rung.selected.clone();
        let label = palette.label;
        let (fill, selection_text) = if emphasized {
            (palette.selection, palette.selection_text)
        } else {
            (
                system_color(SystemColor::UnemphasizedSelectedContentBackground),
                palette.label,
            )
        };

        let list = uniform_list(
            ("ng.finder.column.rows", index),
            lines.len(),
            move |range, _, cx| {
                let Ok(rows) =
                    this.update(cx, |demo: &mut FinderDemo, _| demo.column_listing(folder))
                else {
                    return Vec::new();
                };
                let picked = |row: usize| match rows.get(row) {
                    Some(line) if emphasized => chosen.contains(&line.key),
                    Some(line) => alone.as_ref() == Some(&line.key),
                    None => false,
                };
                range
                    .map(|row| {
                        let line = rows[row].clone();
                        let is_selected = picked(row);
                        let joins_above = row > 0 && picked(row - 1);
                        let joins_below = picked(row + 1);
                        let this = this.clone();
                        let clicked = line.clone();
                        let name = div()
                            .ml(px(LABEL_X - ICON_X - ICON))
                            .flex_1()
                            .overflow_hidden();
                        let mut cell = div()
                            .id(("ng.finder.column.row", row))
                            .h(px(ROW_H))
                            .relative()
                            .flex()
                            .items_center()
                            .pl(px(ICON_X))
                            .pr(px(CHEVRON_TRAIL - (CHEVRON - 3.5) / 2.))
                            .text_color(if is_selected { selection_text } else { label })
                            .when(is_selected, |cell| {
                                cell.child(
                                    div()
                                        .absolute()
                                        .left(px(PILL_INSET))
                                        .right(px(PILL_INSET))
                                        .top_0()
                                        .bottom_0()
                                        .when(!joins_above, |pill| {
                                            pill.rounded_tl(px(PILL_RADIUS))
                                                .rounded_tr(px(PILL_RADIUS))
                                        })
                                        .when(!joins_below, |pill| {
                                            pill.rounded_bl(px(PILL_RADIUS))
                                                .rounded_br(px(PILL_RADIUS))
                                        })
                                        .bg(fill),
                                )
                            })
                            .child(art_at(
                                row_art(&line.row, ICON, scale, true),
                                ICON,
                                (cursor.as_ref() == Some(&line.key)).then(|| ql_icon.clone()),
                            ))
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
                            cell = cell.children(
                                small_glyph("chevron.right", CHEVRON, FontWeight::SEMIBOLD, color)
                                    .map(|arrow| div().flex_none().ml(px(4.)).child(arrow)),
                            );
                        }
                        cell.on_click(move |event: &ClickEvent, window, cx| {
                            let (extend, toggle, twice) = click_kind(event);
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
                        .into_any_element()
                    })
                    .collect()
            },
        )
        .flex_1()
        .track_scroll(&handle);

        let bar = scroller(
            ("ng.finder.column.scroller", index),
            &handle.0.borrow().base_handle,
            0.,
        );
        let column = div()
            .id(("ng.finder.column", index))
            .w(px(self.column_width(folder) - GRIP_W))
            .flex_none()
            .relative()
            .flex()
            .flex_col()
            .pt(px(TOP_GAP))
            .child(list)
            .children(bar.map(|bar| div().absolute().top_0().bottom_0().right_0().child(bar)));
        rows_menu(("ng.finder.column.menu", index), this_menu, hit, column).into_any_element()
    }

    fn on_column_grip(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some((folder, origin)) = self.column_grab else {
            return;
        };
        let width = (self.column_width(folder) + x - origin).max(COL_MIN);
        self.column_widths.insert(folder, width);
        self.column_grab = Some((folder, x));
        cx.notify();
    }

    /// What this column was dragged to, what was saved for it, or Finder's default.
    pub(crate) fn column_width(&self, folder: &'static str) -> f32 {
        self.column_widths
            .get(folder)
            .copied()
            .unwrap_or_else(|| prefs::column_view_width(folder).unwrap_or(COL_W))
    }

    /// Fits a browser column to its longest currently-loaded name, the standard divider gesture.
    fn fit_browser_column(&mut self, folder: &'static str, window: &Window) {
        let lines = self.column_listing(folder);
        let size = px(ROW_H - 8.);
        let widest = lines.iter().fold(0_f32, |widest, line| {
            widest.max(text_width(window, line.row.name.clone(), size))
        });
        let width = (LABEL_X + widest + PREVIEW_INSET).max(COL_MIN);
        self.column_widths.insert(folder, width);
        prefs::set_column_view_width(folder, width);
    }

    fn ruled(
        &self,
        column: AnyElement,
        grow: bool,
        grab: Option<(usize, &'static str)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rule = div()
            .h_full()
            .flex_none()
            .flex()
            .justify_center()
            .child(div().w(px(1.)).h_full().bg(divider()));
        let rule = match grab {
            None => rule.w(px(1.)).into_any_element(),
            Some((index, folder)) => rule
                .id(("ng.finder.column.grip", index))
                .w(px(GRIP_W))
                .cursor_ew_resize()
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    let (_, _, twice) = click_kind(event);
                    if twice {
                        this.fit_browser_column(folder, window);
                        cx.notify();
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.column_grab = Some((folder, f32::from(event.position.x)));
                        cx.notify();
                    }),
                )
                .into_any_element(),
        };
        div()
            .map(|wrap| match grow {
                true => wrap.flex_1(),
                false => wrap.flex_none(),
            })
            .flex()
            .child(column)
            .child(rule)
            .into_any_element()
    }

    fn preview_column(&self, line: &Line, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("ng.finder.column.preview")
            .min_w(px(PREVIEW_W - 1.))
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w(px(PREVIEW_W - 1.))
                    .flex()
                    .flex_col()
                    .child(self.info_card(
                        Some(line),
                        palette,
                        PREVIEW_W - 1. - 2. * PREVIEW_INSET,
                        &cx.entity(),
                    )),
            )
            .into_any_element()
    }

    pub(crate) fn column_body(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let (chain, leaf) = self.column_chain();
        while self.column_scrolls.len() < chain.len() {
            self.column_scrolls.push(UniformListScrollHandle::new());
        }
        let focused = chain.iter().rposition(|rung| rung.selected.is_some());
        let mut columns: Vec<AnyElement> = chain
            .iter()
            .enumerate()
            .map(|(index, rung)| {
                let column = self.browser_column(index, rung, Some(index) == focused, palette, cx);
                self.ruled(column, false, Some((index, rung.folder)), cx)
            })
            .collect();
        if let Some(leaf) = &leaf {
            columns.push(self.ruled(self.preview_column(leaf, palette, cx), true, None, cx));
        }

        let depth = columns.len();
        if depth != self.column_depth {
            self.column_depth = depth;
            self.column_scroll.scroll_to_item(depth - 1);
        }

        let this = cx.entity();
        div()
            .id("ng.finder.columns")
            .flex_1()
            .flex()
            .child(canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    // Window-level, like the pane's grip: the pointer leaves the divider as the
                    // column grows.
                    let moved = this.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            moved.update(cx, |demo, cx| {
                                demo.on_column_grip(f32::from(event.position.x), cx);
                            });
                        }
                    });
                    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            this.update(cx, |demo, _| {
                                if let Some((folder, _)) = demo.column_grab.take() {
                                    prefs::set_column_view_width(folder, demo.column_width(folder));
                                }
                            });
                        }
                    });
                },
            ))
            .overflow_x_scroll()
            .track_scroll(&self.column_scroll)
            .children(columns)
            .into_any_element()
    }

    /// Claims every plain arrow, so `on_key` never discloses in the browser.
    pub(crate) fn column_key(
        &mut self,
        key: &str,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
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
            ("up", Some(at)) => {
                self.pick_in(&lines, &lines[at.saturating_sub(1)], modifiers.shift, false)
            }
            ("down", Some(at)) => {
                let next = (at + 1).min(lines.len() - 1);
                self.pick_in(&lines, &lines[next], modifiers.shift, false);
            }
            ("right", Some(at)) if self.selected.len() > 1 => {
                let next = (at + 1).min(lines.len() - 1);
                self.pick_in(&lines, &lines[next], false, false);
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

    pub(crate) fn column_up(&mut self) {
        let (chain, _) = self.column_chain();
        let Some(parent) = chain
            .iter()
            .rposition(|rung| rung.selected.is_some())
            .filter(|&at| at > 0)
            .map(|at| &chain[at - 1])
        else {
            return;
        };
        let listing = self.column_listing(parent.folder);
        if let Some(line) = listing
            .iter()
            .find(|line| Some(&line.key) == parent.selected.as_ref())
            .cloned()
        {
            self.pick_in(&listing, &line, false, false);
        }
    }

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
}
