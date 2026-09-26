use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Weak};

use gpui::{
    AnyElement, App, Div, ExternalPaths, Hsla, MouseButton, MouseDownEvent, Pixels, Point,
    ScrollHandle, SharedString, TextAlign, Window, div, point, prelude::*, px,
};
use native_gpui::tokens::{SystemColor, system_color};

use crate::app::FinderDemo;
use crate::art::{Anchor, art_at, row_art};
use crate::data::Line;
use crate::dragdrop::{Grabbed, Lit, drop_hover, drop_target, row_carries, row_keys, row_wandered};
use crate::menus::tiles_menu;
use crate::palette::Palette;
use crate::tags::{tag_dots, tag_dots_w};
use crate::ui::{pick_on_click, pick_on_press, rename_field};

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

type Tile = (SharedString, Point<Pixels>);

pub(crate) struct Layout {
    lines: Weak<Vec<Line>>,
    columns: usize,
    cell: f32,
    moved: u64,
    pub(crate) tiles: Vec<Tile>,
    pub(crate) height: f32,
    /// The tiles placed by hand, which may stand anywhere.
    placed: Vec<usize>,
}

impl Layout {
    /// The tiles on screen, in order: those on the grid found from the rows in view.
    fn visible(&self, top: f32, room: f32) -> Vec<usize> {
        let seen = |index: &usize| {
            let y = f32::from(self.tiles[*index].1.y);
            y + 2. * self.cell > top && y < top + room + self.cell
        };
        let first = ((top / self.cell).floor() - 2.).max(0.) as usize;
        let last = ((top + room) / self.cell).ceil().max(0.) as usize + 2;
        let grid = (first * self.columns).min(self.tiles.len())
            ..((last + 1) * self.columns).min(self.tiles.len());
        let mut shown: Vec<usize> = grid.clone().filter(seen).collect();
        shown.extend(
            self.placed
                .iter()
                .filter(|index| !grid.contains(index) && seen(index)),
        );
        shown.sort_unstable();
        shown
    }
}

#[derive(Default)]
pub(crate) struct Grid {
    placed: HashMap<SharedString, Point<Pixels>>,
    moved: u64,
    /// Finder leaves icons where it first put them, whatever the pane or the window does next.
    kept: Cell<Option<(usize, f32)>>,
    layout: Option<Rc<Layout>>,
    dragging: Option<SharedString>,
    pub(crate) scroll: ScrollHandle,
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
        self.moved += 1;
        self.placed
            .insert(key, point(at.x.max(px(0.)), at.y.max(px(0.))));
    }

    pub(crate) fn hold(&self, fits: usize, cell: f32) -> usize {
        match self.kept.get() {
            Some((columns, at)) if at == cell => columns,
            _ => {
                self.kept.set(Some((fits, cell)));
                fits
            }
        }
    }

    pub(crate) fn forget(&mut self) {
        self.moved += 1;
        self.placed.clear();
        self.kept.set(None);
        self.dragging = None;
    }

    fn layout(&mut self, lines: &Arc<Vec<Line>>, columns: usize, cell: f32) -> Rc<Layout> {
        let held = self.layout.clone().filter(|built| {
            std::ptr::eq(built.lines.as_ptr(), Arc::as_ptr(lines))
                && built.columns == columns
                && built.cell == cell
                && built.moved == self.moved
        });
        if let Some(built) = held {
            return built;
        }
        let tiles: Vec<Tile> = lines
            .iter()
            .enumerate()
            .map(|(index, line)| (line.key.clone(), self.at(&line.key, index, columns, cell)))
            .collect();
        let height = tiles
            .iter()
            .map(|(_, at)| f32::from(at.y) + cell)
            .fold(cell, f32::max);
        let placed = match self.placed.is_empty() {
            true => Vec::new(),
            false => (0..tiles.len())
                .filter(|&index| self.placed.contains_key(&tiles[index].0))
                .collect(),
        };
        let built = Rc::new(Layout {
            lines: Arc::downgrade(lines),
            columns,
            cell,
            moved: self.moved,
            tiles,
            height,
            placed,
        });
        self.layout = Some(built.clone());
        built
    }

    pub(crate) fn reveal(
        &self,
        key: &SharedString,
        index: usize,
        columns: usize,
        icon: f32,
        visible: gpui::Size<f32>,
    ) {
        let cell = cell(icon);
        let at = self.at(key, index, columns, cell);
        let offset = self.scroll.offset();
        let into_view = |start: f32, extent: f32, seen: f32| {
            if start < seen {
                start
            } else if start + cell > seen + extent {
                start + cell - extent
            } else {
                seen
            }
        };
        let x = into_view(f32::from(at.x), visible.width, -f32::from(offset.x));
        let y = into_view(f32::from(at.y), visible.height, -f32::from(offset.y));
        self.scroll
            .set_offset(point(px(-x.max(0.)), px(-y.max(0.))));
    }

    pub(crate) fn scroll_to_end(&self, end: bool) {
        let x = self.scroll.offset().x;
        let y = if end {
            self.scroll.max_offset().y
        } else {
            px(0.)
        };
        self.scroll.set_offset(point(x, -y));
    }
}

pub(crate) struct TileDrag {
    key: SharedString,
    grab: Rc<Cell<Point<Pixels>>>,
}

impl Grabbed for TileDrag {
    fn carries(&self, selected: &HashSet<SharedString>, key: &SharedString) -> bool {
        row_carries(&self.key, selected, key)
    }

    fn keys(&self, selected: &HashSet<SharedString>) -> Vec<SharedString> {
        row_keys(&self.key, selected)
    }

    fn wandered(&self, window: &Window, pressed_at: Point<Pixels>) -> bool {
        row_wandered(window, pressed_at)
    }
}

struct Ghost {
    line: Line,
    size: f32,
    scale: f32,
    look: Look,
    tags: Vec<usize>,
}

impl Render for Ghost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        tile(
            &self.line, self.size, self.scale, self.look, true, &self.tags, None, true, None, None,
        )
        .opacity(GHOST_ALPHA)
    }
}

#[allow(clippy::too_many_arguments)]
fn tile(
    line: &Line,
    size: f32,
    scale: f32,
    look: Look,
    selected: bool,
    tags: &[usize],
    editor: Option<AnyElement>,
    visible: bool,
    anchor: Option<Anchor>,
    drop: Option<Hsla>,
) -> Div {
    let cell = cell(size);
    let lit = selected || drop.is_some();
    let pill = drop.unwrap_or(look.selection);
    let mut icon = div()
        .w(px(size))
        .h(px(size))
        .flex_none()
        .relative()
        .flex()
        .items_center()
        .justify_center();
    if lit {
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

    let dots = tag_dots_w(tags.len());
    let room = cell - 2. * LABEL_INSET - if dots > 0. { dots + 2. } else { 0. };
    let caption = match editor {
        Some(editor) => {
            let fit = line.row.display.chars().count() as f32 * LABEL_FONT * 0.58 + 24.;
            div().w(px(fit.min(room))).child(editor)
        }
        None => div()
            .px(px(4.))
            .max_w(px(room))
            .rounded(px(LABEL_RADIUS))
            .text_size(px(LABEL_FONT))
            .line_height(px(LABEL_LINE))
            .line_clamp(LABEL_LINES)
            .text_align(TextAlign::Center)
            .text_color(if lit { look.selection_text } else { look.label })
            .when(lit, |label| label.bg(pill))
            .child(elide(&line.row.display, room - 8.)),
    };

    div()
        .w(px(cell))
        .h(px(cell))
        .flex()
        .flex_col()
        .items_center()
        .pt(px(ICON_TOP))
        .gap(px(ICON_LABEL_GAP))
        .child(icon.child(art_at(
            row_art(&line.row, size, scale, visible),
            size,
            anchor,
        )))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(4.))
                .child(caption)
                .children(tag_dots(tags)),
        )
}

fn tile_at(
    layout: &[(SharedString, Point<Pixels>)],
    at: Point<Pixels>,
    cell: f32,
) -> Option<usize> {
    layout.iter().position(|(_, tile)| {
        at.x >= tile.x && at.x < tile.x + px(cell) && at.y >= tile.y && at.y < tile.y + px(cell)
    })
}

fn elide(name: &SharedString, room: f32) -> SharedString {
    let per_line = (room / (LABEL_FONT * 0.58)).floor() as usize;
    let budget = per_line * LABEL_LINES;
    let count = name.chars().count();
    if count <= budget || budget < 12 {
        return name.clone();
    }
    let tail = 8;
    let head = budget - 1 - tail;
    let at = |nth: usize| {
        name.char_indices()
            .nth(nth)
            .map_or(name.len(), |(at, _)| at)
    };
    SharedString::from(format!(
        "{}…{}",
        &name[..at(head)],
        &name[at(count - tail)..]
    ))
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
        let scale = self.scale;
        let cell = cell(size);
        let look = Look::new(palette);
        let this = cx.entity().downgrade();
        let scroll = self.icons.scroll.clone();
        let dragging = self.icons.dragging.clone();

        let layout = self.icons.layout(&lines, columns, cell);
        let height = layout.height;

        let width = GRID_X + columns as f32 * cell;
        let mut canvas = div()
            .id("ng.finder.icons")
            .flex_1()
            .relative()
            .overflow_x_scroll()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            // The only child in flow: the tiles are absolute and contribute no scrollable size.
            .child(
                div()
                    .w(px(width))
                    .h(px(height))
                    .flex_none(),
            );

        let top = -f32::from(self.icons.scroll.offset().y);
        let room = f32::from(window.viewport_size().height);
        for index in layout.visible(top, room) {
            let line = &lines[index];
            let at = layout.tiles[index].1;
            let is_selected = self.selected.contains(&line.key);
            let carried = self.tags.get(&line.key).cloned().unwrap_or_default();
            let held = dragging.as_ref() == Some(&line.key);
            let grab = Rc::new(Cell::new(point(px(0.), px(0.))));
            let editor = (self.renaming.as_ref() == Some(&line.key))
                .then(|| rename_field(line, index, this.clone()).into_any_element());

            let tile = div()
                .id(("ng.finder.tile", index))
                .absolute()
                .left(at.x)
                .top(at.y)
                .w(px(cell))
                .h(px(cell))
                .child(tile(
                    line,
                    size,
                    scale,
                    look,
                    is_selected,
                    &carried,
                    editor,
                    true,
                    (self.cursor.as_ref() == Some(&line.key)).then(|| self.ql_icon.clone()),
                    self.drop_fill(&line.key, look.selection),
                ))
                .when(held, |tile| tile.opacity(SOURCE_ALPHA))
                .on_drag(
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
                                scale,
                                look,
                                tags: carried.clone(),
                            })
                        }
                    },
                )
                .on_mouse_down(MouseButton::Left, pick_on_press(line, this.clone()))
                .on_click(pick_on_click(line, this.clone()));
            let tile = match line.row.opens {
                Some(into) => drop_target::<ExternalPaths>(
                    drop_hover::<TileDrag>(
                        tile,
                        line.key.clone(),
                        into,
                        None,
                        self.selected.snapshot(),
                        this.clone(),
                    ),
                    line.key.clone(),
                    into,
                    None,
                    self.selected.snapshot(),
                    Lit::Drawn,
                    this.clone(),
                ),
                None => tile,
            };
            canvas = canvas.child(tile);
        }

        let dropped = layout.clone();
        let drop_scroll = scroll.clone();
        let this_drop = this.clone();
        let dropped_lines = lines.clone();
        canvas = canvas.on_drop(move |drag: &TileDrag, window, cx| {
            let Some(from) = dropped
                .tiles
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
            let under = tile_at(&dropped.tiles, landed, cell)
                .and_then(|index| dropped_lines.get(index))
                .cloned();
            this_drop
                .update(cx, |this: &mut FinderDemo, cx| {
                    let into = under
                        .filter(|line| !drag.carries(&this.selected.snapshot(), &line.key))
                        .and_then(|line| line.row.opens);
                    if let Some(into) = into {
                        if !drag.wandered(window, this.pressed_at) {
                            return;
                        }
                        let keys = this.drag_keys(&drag.key);
                        this.drop_into(into, &keys, window, cx);
                        this.icons.dragging = None;
                        cx.notify();
                        return;
                    }
                    let group: Vec<Tile> = if this.selected.contains(&drag.key) {
                        moved
                            .tiles
                            .iter()
                            .filter(|(key, _)| this.selected.contains(key))
                            .cloned()
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

        let hit_layout = layout.clone();
        let hit_scroll = scroll.clone();
        let hit = move |demo: &FinderDemo, at: Point<Pixels>| {
            let at = at - hit_scroll.offset();
            let index = tile_at(&hit_layout.tiles, at, cell)?;
            demo.lines(false).get(index).cloned()
        };

        let empty = layout;
        let empty_scroll = scroll;
        let this_menu = this.clone();
        // Hit-tested by hand: GPUI's drag is a bubble listener and misses a press a tile swallows.
        let canvas =
            canvas.on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                let at = event.position - empty_scroll.bounds().origin - empty_scroll.offset();
                if tile_at(&empty.tiles, at, cell).is_some() {
                    return;
                }
                this.update(cx, |this: &mut FinderDemo, cx| {
                    this.deselect();
                    cx.notify();
                })
                .ok();
            });
        tiles_menu("ng.finder.icons.menu", this_menu, hit, canvas).into_any_element()
    }
}
