//! The icon (grid) view.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Div, Hsla, MouseButton, MouseDownEvent, Pixels, Point,
    ScrollHandle, SharedString, TextAlign, Window, div, point, prelude::*, px,
};
use native_gpui::tokens::{SystemColor, system_color};

use crate::FinderDemo;
use crate::data::Line;
use crate::palette::Palette;
use crate::ui::{artwork, tag_dot};

const CELL_PAD: f32 = 48.;
const ICON_TOP: f32 = 14.;
const GRID_X: f32 = 10.;
const LABEL_FONT: f32 = 12.;
const LABEL_LINE: f32 = 16.;
const LABEL_LINES: usize = 2;
const LABEL_INSET: f32 = 0.5;
const ICON_LABEL_GAP: f32 = 6.;
const LABEL_RADIUS: f32 = 5.;
const PLATE_PAD: f32 = 4.;
const PLATE_RADIUS: f32 = 8.;
const GHOST_ALPHA: f32 = 0.7;
const SOURCE_ALPHA: f32 = 0.35;

pub(crate) fn cell(icon: f32) -> f32 {
    icon + CELL_PAD
}

#[derive(Clone, Copy)]
struct Look {
    label: Hsla,
    selection: Hsla,
    selection_text: Hsla,
    backdrop: Hsla,
}

impl Look {
    fn new(palette: &Palette) -> Self {
        Self {
            label: palette.label,
            selection: palette.selection,
            selection_text: palette.selection_text,
            backdrop: system_color(SystemColor::QuaternaryLabel),
        }
    }
}

#[derive(Default)]
/// Where each icon-view item sits, and the drag in flight.
pub(crate) struct Grid {
    placed: HashMap<SharedString, Point<Pixels>>,
    dragging: Option<SharedString>,
    scroll: ScrollHandle,
}

impl Grid {
    fn slot(index: usize, columns: usize, cell: f32) -> Point<Pixels> {
        let column = index % columns;
        let row = index / columns;
        point(px(GRID_X + column as f32 * cell), px(row as f32 * cell))
    }

    fn at(&self, key: &SharedString, index: usize, columns: usize, cell: f32) -> Point<Pixels> {
        self.placed
            .get(key)
            .copied()
            .unwrap_or_else(|| Self::slot(index, columns, cell))
    }

    fn place(&mut self, key: SharedString, at: Point<Pixels>) {
        self.placed
            .insert(key, point(at.x.max(px(0.)), at.y.max(px(0.))));
    }

    pub(crate) fn forget(&mut self) {
        self.placed.clear();
        self.dragging = None;
    }

    pub(crate) fn reveal(&self, key: &SharedString, index: usize, columns: usize, icon: f32) {
        let cell = cell(icon);
        let at = self.at(key, index, columns, cell);
        let view = self.scroll.bounds().size.height;
        let offset = self.scroll.offset();
        let top = f32::from(at.y);
        let bottom = top + cell;
        let seen = -f32::from(offset.y);
        let next = if top < seen {
            top
        } else if bottom > seen + f32::from(view) {
            bottom - f32::from(view)
        } else {
            return;
        };
        self.scroll.set_offset(point(offset.x, px(-next.max(0.))));
    }
}

struct TileDrag {
    key: SharedString,
    grab: Rc<Cell<Point<Pixels>>>,
}

struct Ghost {
    line: Line,
    size: f32,
    look: Look,
    tag: Option<usize>,
}

impl Render for Ghost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        tile(&self.line, self.size, self.look, true, self.tag).opacity(GHOST_ALPHA)
    }
}

fn tile(line: &Line, size: f32, look: Look, selected: bool, tag: Option<usize>) -> Div {
    let cell = cell(size);
    let mut icon = div()
        .w(px(size))
        .h(px(size))
        .flex_none()
        .relative()
        .flex()
        .items_center()
        .justify_center();
    if selected {
        icon = icon.child(
            div()
                .absolute()
                .top(px(-PLATE_PAD))
                .left(px(-PLATE_PAD))
                .right(px(-PLATE_PAD))
                .bottom(px(-PLATE_PAD))
                .rounded(px(PLATE_RADIUS))
                .bg(look.backdrop),
        );
    }

    let room = cell - 2. * LABEL_INSET - if tag.is_some() { 12. } else { 0. };
    let caption = div()
        .px(px(4.))
        .max_w(px(room))
        .rounded(px(LABEL_RADIUS))
        .text_size(px(LABEL_FONT))
        .line_height(px(LABEL_LINE))
        .line_clamp(LABEL_LINES)
        .text_align(TextAlign::Center)
        .text_color(if selected {
            look.selection_text
        } else {
            look.label
        })
        .when(selected, |pill| pill.bg(look.selection))
        .child(elide(&line.row.name, room - 8.));

    div()
        .w(px(cell))
        .h(px(cell))
        .flex()
        .flex_col()
        .items_center()
        .pt(px(ICON_TOP))
        .gap(px(ICON_LABEL_GAP))
        .child(icon.child(artwork(&line.row, size)))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(4.))
                .child(caption)
                .children(tag.map(tag_dot)),
        )
}

fn elide(name: &SharedString, room: f32) -> SharedString {
    let per_line = (room / (LABEL_FONT * 0.58)).floor() as usize;
    let budget = per_line * LABEL_LINES;
    if name.chars().count() <= budget || budget < 12 {
        return name.clone();
    }
    let chars: Vec<char> = name.chars().collect();
    let tail = 8;
    let head = budget - 1 - tail;
    let front: String = chars[..head].iter().collect();
    let back: String = chars[chars.len() - tail..].iter().collect();
    SharedString::from(format!("{front}…{back}"))
}

impl FinderDemo {
    pub(crate) fn icon_body(
        &mut self,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !cx.has_active_drag() {
            self.icons.dragging = None;
        }

        let lines = self.lines(false);
        let columns = self.columns(window);
        let size = self.icon_size as f32;
        let cell = cell(size);
        let look = Look::new(palette);
        let this = cx.entity().downgrade();
        let scroll = self.icons.scroll.clone();
        let dragging = self.icons.dragging.clone();

        let layout: Vec<(SharedString, Point<Pixels>)> = lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                (
                    line.key.clone(),
                    self.icons.at(&line.key, index, columns, cell),
                )
            })
            .collect();
        let height = layout
            .iter()
            .map(|(_, at)| f32::from(at.y) + cell)
            .fold(cell, f32::max);

        let mut canvas = div()
            .id("ng.finder.icons")
            .flex_1()
            .relative()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            // The only child in flow: the tiles are absolute and contribute no scrollable height.
            .child(div().w_full().h(px(height)).flex_none());

        for (index, line) in lines.iter().enumerate() {
            let at = layout[index].1;
            let is_selected = self.selected.contains(&line.key);
            let tag = self.tags.get(&line.key).copied();
            let held = dragging.as_ref() == Some(&line.key);
            let grab = Rc::new(Cell::new(point(px(0.), px(0.))));

            let mut slot = div()
                .id(("ng.finder.tile", index))
                .absolute()
                .left(at.x)
                .top(at.y)
                .w(px(cell))
                .h(px(cell))
                .child(tile(line, size, look, is_selected, tag))
                .when(held, |tile| tile.opacity(SOURCE_ALPHA));

            slot = slot.on_drag(
                TileDrag {
                    key: line.key.clone(),
                    grab: grab.clone(),
                },
                {
                    let this = this.clone();
                    let line = line.clone();
                    move |drag: &TileDrag, offset, _window, cx: &mut App| {
                        drag.grab.set(offset);
                        let key = drag.key.clone();
                        this.update(cx, |this: &mut FinderDemo, _| {
                            this.icons.dragging = Some(key);
                        })
                        .ok();
                        cx.new(|_| Ghost {
                            line: line.clone(),
                            size,
                            look,
                            tag,
                        })
                    }
                },
            );

            let pressed = line.clone();
            let this_down = this.clone();
            slot = slot.on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                if event.modifiers.shift || event.modifiers.platform {
                    return;
                }
                let line = pressed.clone();
                this_down
                    .update(cx, |this: &mut FinderDemo, cx| {
                        if !this.selected.contains(&line.key) {
                            this.pick(&line, false, false);
                            cx.notify();
                        }
                    })
                    .ok();
            });

            let clicked = line.clone();
            let this_click = this.clone();
            canvas = canvas.child(slot.on_click(move |event: &ClickEvent, window, cx| {
                let (extend, toggle, twice) = match event {
                    ClickEvent::Mouse(mouse) => (
                        mouse.up.modifiers.shift,
                        mouse.up.modifiers.platform,
                        mouse.up.click_count >= 2,
                    ),
                    _ => (false, false, false),
                };
                let line = clicked.clone();
                this_click
                    .update(cx, |this: &mut FinderDemo, cx| {
                        this.pick(&line, extend, toggle);
                        if twice {
                            this.open(&line, window, cx);
                        }
                        cx.notify();
                    })
                    .ok();
            }));
        }

        let dropped = layout.clone();
        let drop_scroll = scroll.clone();
        let this_drop = this.clone();
        canvas = canvas.on_drop(move |drag: &TileDrag, window, cx| {
            let Some(from) = dropped
                .iter()
                .find(|(key, _)| key == &drag.key)
                .map(|(_, at)| *at)
            else {
                return;
            };
            let landed =
                window.mouse_position() - drop_scroll.bounds().origin - drop_scroll.offset();
            let delta = landed - drag.grab.get() - from;
            let moved = dropped.clone();
            this_drop
                .update(cx, |this: &mut FinderDemo, cx| {
                    let group: Vec<(SharedString, Point<Pixels>)> =
                        if this.selected.contains(&drag.key) {
                            moved
                                .into_iter()
                                .filter(|(key, _)| this.selected.contains(key))
                                .collect()
                        } else {
                            vec![(drag.key.clone(), from)]
                        };
                    for (key, at) in group {
                        this.icons.place(key, at + delta);
                    }
                    this.icons.dragging = None;
                    cx.notify();
                })
                .ok();
        });

        let empty = layout;
        let empty_scroll = scroll;
        // Hit-tested by hand: GPUI's drag is a bubble listener and misses a press a tile swallows.
        canvas
            .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                let at = event.position - empty_scroll.bounds().origin - empty_scroll.offset();
                if empty.iter().any(|(_, tile)| {
                    at.x >= tile.x
                        && at.x < tile.x + px(cell)
                        && at.y >= tile.y
                        && at.y < tile.y + px(cell)
                }) {
                    return;
                }
                this.update(cx, |this: &mut FinderDemo, cx| {
                    this.selected.clear();
                    this.cursor = None;
                    this.anchor = None;
                    cx.notify();
                })
                .ok();
            })
            .into_any_element()
    }
}
