use gpui::{
    AnyElement, ClickEvent, DispatchPhase, ExternalPaths, FontWeight, Hsla, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString,
    UniformListScrollHandle, Window, WindowTextSystem, canvas, div, prelude::*, px,
    transparent_black, uniform_list,
};
use native_gpui::native_scroll_bars;
use native_gpui::tokens::{self, SystemColor, system_color};

use crate::app::FinderDemo;
use crate::art::{art_at, row_art};
use crate::data::{Line, split_key};
use crate::dragdrop::{DragGhost, Lit, RowDrag, drop_lands, flashed};
use crate::elastic::{shifted, stretching};
use crate::fit;
use crate::menus::rows_menu;
use crate::palette::Palette;
use crate::prefs;
use crate::ui::{axis_locked, click_kind, menu_ring, rename_field};
use crate::views::{chevron, folder_drop, run_pill};

const ROW_H: f32 = 24.;
const COL_W: f32 = 164.;
const PREVIEW_W: f32 = 239.;
const PREVIEW_INSET: f32 = 10.;
const ICON_X: f32 = 13.;
pub(crate) const ICON: f32 = 18.;
const LABEL_X: f32 = 35.5;
const CHEVRON_TRAIL: f32 = 14.5;
const CHEVRON: f32 = 6.5;
const PILL_INSET: f32 = 7.25;
const PILL_RADIUS: f32 = 7.;
const TOP_GAP: f32 = 6.;
const GRIP_W: f32 = 7.;
const REVEAL_SECS: f32 = 0.2;
const COL_MIN: f32 = 100.;
const BROWSER_FIT: f32 = 77.;
const WIDTHS_HELD: usize = 256;

const AREA_ALPHA: f32 = 0.3;
const AREA_EDGE: f32 = 2.;

#[derive(Clone)]
pub(crate) struct Rung {
    pub(crate) folder: &'static str,
    pub(crate) selected: Option<SharedString>,
}

impl FinderDemo {
    pub(crate) fn walk_chain(&self) -> (Vec<Rung>, Option<Line>) {
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
            let (holder, name) = split_key(&cursor)?;
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
        match row
            .opens
            .filter(|_| self.selected.len() <= 1 && row.readable)
        {
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
        self.column_pick(parent, |line| line.row.opens == Some(here));
    }

    fn column_pick(&mut self, folder: &'static str, hit: impl Fn(&Line) -> bool) -> bool {
        let listing = self.column_listing(folder);
        let Some(line) = listing.iter().find(|line| hit(line)).cloned() else {
            return false;
        };
        self.pick_in(&listing, &line, false, false);
        true
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
        let rows = self.column_listing(folder);
        let scale = self.scale;
        let handle = self.column_scrolls[index].clone();
        let this = cx.entity().downgrade();
        let this_menu = this.clone();
        let this_drop = this.clone();
        let hit_scroll = handle.clone();
        let stretch = self.bands.columns[index].shown().y;
        let hit = move |demo: &FinderDemo, at: Point<Pixels>| {
            let top = f32::from(hit_scroll.0.borrow().base_handle.offset().y + stretch);
            let row = (f32::from(at.y) - top - TOP_GAP) / ROW_H;
            if row < 0. {
                return None;
            }
            demo.column_listing(folder).get(row as usize).cloned()
        };
        let renaming = self.renaming.clone();
        let resting = self.drop_row().cloned();
        let springing = self.spring_flash().cloned();
        let cursor = self.cursor.clone();
        let menu_row = self.menu_row.clone();
        let ql_icon = self.ql_icon.clone();
        let chosen = self.selected.snapshot();
        let alone = rung.selected.clone();
        let label = palette.label;
        let (highlight, highlight_text) = (palette.selection, palette.selection_text);
        let (fill, selection_text) = if emphasized {
            (palette.selection, palette.selection_text)
        } else {
            (
                system_color(SystemColor::UnemphasizedSelectedContentBackground),
                palette.label,
            )
        };

        // `uniform_list` measures its items with no width of its own.
        let content_w = self.column_width(folder) - GRIP_W;
        // The count and the rows it indexes come from one read: a second read can be shorter.
        let list = uniform_list(
            ("ng.finder.column.rows", index),
            rows.len(),
            move |range, _, _| {
                let picked = |row: usize| match rows.get(row) {
                    Some(line) if emphasized => chosen.contains(&line.key),
                    Some(line) => alone.as_ref() == Some(&line.key),
                    None => false,
                };
                range
                    .map(|row| {
                        let line = &rows[row];
                        let is_selected = picked(row);
                        let dropping = resting.as_ref() == Some(&line.key);
                        let drop_fill = match springing.as_ref() == Some(&line.key) {
                            true => flashed(highlight),
                            false => highlight,
                        };
                        let joins_above = !dropping && row > 0 && picked(row - 1);
                        let joins_below = !dropping && picked(row + 1);
                        let aimed = menu_row.as_ref().is_some_and(|aim| {
                            *aim == line.key || (is_selected && chosen.contains(aim))
                        });
                        let this = this.clone();
                        let clicked = line.clone();
                        let name = div()
                            .ml(px(LABEL_X - ICON_X - ICON))
                            .flex_1()
                            .overflow_hidden();
                        let cell = div()
                            .id(("ng.finder.column.row", row))
                            .font_family(tokens::system_font_family())
                            .w(px(content_w))
                            .h(px(ROW_H))
                            .relative()
                            .flex()
                            .items_center()
                            .pl(px(ICON_X))
                            .pr(px(CHEVRON_TRAIL - (CHEVRON - 3.5) / 2.))
                            .text_color(match (dropping, is_selected) {
                                (true, _) => highlight_text,
                                (false, true) => selection_text,
                                (false, false) => label,
                            })
                            .when(is_selected || dropping, |cell| {
                                cell.child(run_pill(
                                    if dropping { drop_fill } else { fill },
                                    PILL_INSET,
                                    PILL_RADIUS,
                                    (joins_above, joins_below),
                                ))
                            })
                            .when(aimed, |cell| {
                                let over = is_selected && emphasized;
                                cell.child(menu_ring(
                                    if over { selection_text } else { highlight },
                                    (PILL_INSET, 0.),
                                    PILL_RADIUS,
                                    (joins_above, joins_below),
                                    over,
                                ))
                            })
                            .child(art_at(
                                row_art(&line.row, ICON, scale, true),
                                ICON,
                                (cursor.as_ref() == Some(&line.key)).then(|| ql_icon.clone()),
                            ))
                            .child(if renaming.as_ref() == Some(&line.key) {
                                name.child(rename_field(line, row, this.clone()))
                            } else {
                                name.text_ellipsis_middle()
                                    .whitespace_nowrap()
                                    .child(line.row.display.clone())
                            })
                            .when(line.row.opens.is_some() && line.row.readable, |cell| {
                                let lit = dropping || (is_selected && emphasized);
                                cell.children(
                                    chevron("chevron.right", CHEVRON, FontWeight::SEMIBOLD, lit)
                                        .map(|arrow| div().flex_none().ml(px(4.)).child(arrow)),
                                )
                            })
                            .on_click({
                                let this = this.clone();
                                move |event: &ClickEvent, window, cx| {
                                    let (extend, toggle, twice) = click_kind(event);
                                    this.update(cx, |this: &mut FinderDemo, cx| {
                                        let listing = this.column_listing(folder);
                                        this.pick_in(&listing, &clicked, extend, toggle);
                                        if twice {
                                            this.open(&clicked, window, cx);
                                        }
                                        this.column_arrived(window, cx);
                                    })
                                    .ok();
                                }
                            })
                            .on_drag(
                                RowDrag {
                                    key: line.key.clone(),
                                },
                                move |_, _, _, cx| cx.new(|_| DragGhost),
                            );
                        match line.row.opens {
                            Some(into) => {
                                folder_drop(cell, &line.key, into, None, this).into_any_element()
                            }
                            None => cell.into_any_element(),
                        }
                    })
                    .map(|row| shifted(row, stretch))
                    .collect()
            },
        )
        .flex_1()
        .track_scroll(&handle)
        .on_scroll_wheel(stretching(
            move |this| {
                let handle = this.column_scrolls[index].0.borrow().base_handle.clone();
                (&mut this.bands.columns[index], handle)
            },
            cx,
        ));

        let bars = native_scroll_bars(("ng.finder.column.scroll_bars", index))
            .vertical(&handle.0.borrow().base_handle);
        // Under the rows, so a folder row's own drop lands first.
        let edge = match self.spring.is_some() {
            true => transparent_black(),
            false => Hsla {
                a: AREA_ALPHA,
                ..system_color(SystemColor::ControlAccent)
            },
        };
        let area = div()
            .id(("ng.finder.column.area", index))
            .absolute()
            .inset_0()
            .border(px(AREA_EDGE))
            .border_color(transparent_black());
        let key = SharedString::from(folder);
        let area = drop_lands::<RowDrag>(
            area,
            key.clone(),
            folder,
            Lit::Outline(edge),
            this_drop.clone(),
        );
        let area = drop_lands::<ExternalPaths>(area, key, folder, Lit::Outline(edge), this_drop);
        let column = div()
            .id(("ng.finder.column", index))
            .w(px(self.column_width(folder) - GRIP_W))
            .flex_none()
            .relative()
            .flex()
            .flex_col()
            .pt(px(TOP_GAP))
            .child(area)
            .child(list)
            .child(
                div()
                    .absolute()
                    .top(px(TOP_GAP))
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .child(bars),
            );
        rows_menu(("ng.finder.column.menu", index), this_menu, hit, column).into_any_element()
    }

    pub(crate) fn column_open(
        &mut self,
        key: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((holder, _)) = split_key(key) else {
            return;
        };
        if self.column_pick(holder, |line| &line.key == key) {
            self.column_arrived(window, cx);
        }
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

    pub(crate) fn column_width(&self, folder: &'static str) -> f32 {
        self.column_widths
            .get(folder)
            .copied()
            .unwrap_or_else(|| prefs::column_view_width(folder).unwrap_or(COL_W))
    }

    fn fit_browser_column(&mut self, folder: &'static str, cx: &mut Context<Self>) {
        let names: Vec<SharedString> = self
            .column_listing(folder)
            .iter()
            .map(|line| line.row.display.clone())
            .collect();
        let size = tokens::system_font_size();
        fit::in_background(
            cx,
            move |texts| browser_fit_width(texts, &names, size),
            move |this, width, cx| {
                this.column_widths.insert(folder, width);
                prefs::set_column_view_width(folder, width);
                cx.notify();
            },
        );
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
            .child(div().w(px(1.)).h_full().bg(crate::palette::browser_rule()));
        let rule = match grab {
            None => rule.w(px(1.)).into_any_element(),
            Some((index, folder)) => rule
                .id(("ng.finder.column.grip", index))
                .w(px(GRIP_W))
                .cursor_ew_resize()
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    let (_, _, twice) = click_kind(event);
                    if twice {
                        this.fit_browser_column(folder, cx);
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
        let shift = self.bands.browser.shown().x;
        div()
            .when(shift != px(0.), |wrap| wrap.relative().left(shift))
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
            .min_w(px(PREVIEW_W))
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w(px(PREVIEW_W))
                    .flex()
                    .flex_col()
                    .child(self.info_card(
                        Some(line),
                        palette,
                        PREVIEW_W - 2. * PREVIEW_INSET,
                        &cx.entity(),
                    )),
            )
            .into_any_element()
    }

    pub(crate) fn column_body(
        &mut self,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (chain, leaf) = self.column_chain();
        while self.column_scrolls.len() < chain.len() {
            self.column_scrolls.push(UniformListScrollHandle::new());
            self.bands.columns.push(Default::default());
        }
        for rung in &chain {
            if !self.column_widths.contains_key(rung.folder) {
                if self.column_widths.len() >= WIDTHS_HELD && self.column_grab.is_none() {
                    self.column_widths.clear();
                }
                let width = prefs::column_view_width(rung.folder).unwrap_or(COL_W);
                self.column_widths.insert(rung.folder, width);
            }
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
        if let Some(leaf) = leaf.as_ref().filter(|_| self.preview) {
            columns.push(self.ruled(self.preview_column(leaf, palette, cx), true, None, cx));
        }

        let depth = columns.len();
        let picked = self.column_cursor != self.cursor;
        self.column_cursor = self.cursor.clone();
        if depth != self.column_depth {
            match self.column_depth != 0 && picked {
                false => {
                    self.column_reveal = None;
                    // The browser's first child is the grip canvas, so the last column is `depth`.
                    self.column_scroll.scroll_to_item(depth);
                }
                true => {
                    let from = f32::from(self.column_scroll.offset().x);
                    self.column_reveal = Some((from, std::time::Instant::now()));
                }
            }
            self.column_depth = depth;
        }
        if let Some((from, started)) = self.column_reveal {
            let t = (started.elapsed().as_secs_f32() / REVEAL_SECS).min(1.);
            let to = -f32::from(self.column_scroll.max_offset().x);
            let x = from + (to - from) * gpui::ease_in_out(t);
            self.column_scroll.set_offset(gpui::point(px(x), px(0.)));
            match t < 1. {
                true => window.request_animation_frame(),
                false => self.column_reveal = None,
            }
        }

        let this = cx.entity();
        let browser = axis_locked(div())
            .id("ng.finder.columns")
            .flex_1()
            .flex()
            .child(canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    // Window-level: the pointer leaves the divider as the column grows.
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
            .on_scroll_wheel(stretching(
                |this| (&mut this.bands.browser, this.column_scroll.clone()),
                cx,
            ))
            .children(columns);
        // Beside the browser, not in it: a child of a scrolled element scrolls away with it.
        div()
            .flex_1()
            .relative()
            .flex()
            .child(browser)
            .child(div().absolute().inset_0().child(
                native_scroll_bars("ng.finder.columns.scroll_bars").horizontal(&self.column_scroll),
            ))
            .into_any_element()
    }

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
                self.column_pick(chain[0].folder, |_| true);
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
            ("up", Some(_)) if modifiers.alt => {
                self.pick_in(&lines, &lines[0], modifiers.shift, false)
            }
            ("down", Some(_)) if modifiers.alt => {
                self.pick_in(&lines, &lines[lines.len() - 1], modifiers.shift, false)
            }
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
                    self.column_pick(into, |_| true);
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
        self.column_pick(parent.folder, |line| {
            Some(&line.key) == parent.selected.as_ref()
        });
    }

    pub(crate) fn column_arrived(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.arriving = None;
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
}

fn browser_fit_width(texts: &WindowTextSystem, names: &[SharedString], size: Pixels) -> f32 {
    let font = tokens::system_font(FontWeight::NORMAL);
    let widest = fit::widest(
        texts,
        &font,
        size,
        names,
        |name| name.clone(),
        |_, width| width,
    );
    (widest + BROWSER_FIT).ceil().max(COL_W)
}
