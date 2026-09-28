use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Weak};
use std::time::Duration;

use finder_fs::ds_store::{self, IconView};
use finder_fs::sort::name_cmp;
use gpui::{
    AnyElement, App, Div, ExternalPaths, Hsla, MouseButton, MouseDownEvent, Pixels, Point,
    ScrollHandle, SharedString, TextAlign, Window, div, point, prelude::*, px,
};
use native_gpui::tokens::{SystemColor, system_color};

use crate::app::FinderDemo;
use crate::art::{Anchor, art_at, row_art};
use crate::data::{Column, Line, Row};
use crate::dragdrop::{Grabbed, Lit, drop_hover, drop_target, row_carries, row_keys, row_wandered};
use crate::elastic::stretching;
use crate::menus::tiles_menu;
use crate::palette::Palette;
use crate::prefs;
use crate::tags::{tag_dots, tag_dots_w};
use crate::ui::{pick_on_click, pick_on_press, rename_field};
use crate::view::ICON_VIEW;

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
/// How long a folder shown as icons waits on the main thread for Finder's record of them.
const FINDER_WAIT: Duration = Duration::from_millis(30);

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
type Slot = (i32, i32);

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Arrangement {
    Free,
    Grid,
    By(Column),
}

impl Arrangement {
    /// From Finder's `arrangeBy`.
    fn of(id: &str) -> Option<Self> {
        match id {
            "none" => Some(Self::Free),
            "grid" => Some(Self::Grid),
            "name" => Some(Self::By(Column::Name)),
            "kind" => Some(Self::By(Column::Kind)),
            "dateModified" => Some(Self::By(Column::Modified)),
            "size" => Some(Self::By(Column::Size)),
            _ => None,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Free => "none",
            Self::Grid => "grid",
            Self::By(Column::Name) => "name",
            Self::By(Column::Kind) => "kind",
            Self::By(Column::Modified) => "dateModified",
            Self::By(Column::Size) => "size",
        }
    }
}

fn slot_at(index: usize, columns: usize) -> Slot {
    ((index % columns) as i32, (index / columns) as i32)
}

/// Finder records an icon's place as the centre of its image.
fn slot_centre((column, row): Slot, size: f32, cell: f32) -> Point<Pixels> {
    point(
        px(GRID_X + (column as f32 + 0.5) * cell),
        px(row as f32 * cell + ICON_TOP + size / 2.),
    )
}

fn nearest_slot(at: Point<Pixels>, size: f32, cell: f32) -> Slot {
    let column = (f32::from(at.x) - GRID_X) / cell - 0.5;
    let row = (f32::from(at.y) - ICON_TOP - size / 2.) / cell;
    (column.round().max(0.) as i32, row.round().max(0.) as i32)
}

fn nearest_free(at: Point<Pixels>, taken: &HashSet<Slot>, size: f32, cell: f32) -> Slot {
    let (column, row) = nearest_slot(at, size, cell);
    let mut best: Option<(f32, Slot)> = None;
    for ring in 0i32.. {
        let near = (ring as f32 - 0.5).max(0.) * cell;
        if best.is_some_and(|(far, _)| near * near > far) {
            break;
        }
        for dy in -ring..=ring {
            for dx in -ring..=ring {
                let slot = (column + dx, row + dy);
                if dx.abs().max(dy.abs()) != ring
                    || slot.0 < 0
                    || slot.1 < 0
                    || taken.contains(&slot)
                {
                    continue;
                }
                let centre = slot_centre(slot, size, cell);
                let (x, y) = (f32::from(centre.x - at.x), f32::from(centre.y - at.y));
                let far = x * x + y * y;
                if best.is_none_or(|(best, _)| far < best) {
                    best = Some((far, slot));
                }
            }
        }
    }
    best.map_or((column, row), |(_, slot)| slot)
}

fn origin(centre: Point<Pixels>, size: f32, cell: f32) -> Point<Pixels> {
    point(
        centre.x - px(cell / 2.),
        centre.y - px(ICON_TOP + size / 2.),
    )
}

/// Finder keeps a sorted view in the order a first click on that column gives.
fn arranged(lines: &[Line], column: Column, columns: usize, size: f32) -> Vec<Point<Pixels>> {
    let mut rows: Vec<Arc<Row>> = lines.iter().map(|line| line.row.clone()).collect();
    crate::read::sort_rows(
        &mut rows,
        (column, matches!(column, Column::Name | Column::Kind)),
    );
    let order: HashMap<*const Row, usize> = rows
        .iter()
        .enumerate()
        .map(|(at, row)| (Arc::as_ptr(row), at))
        .collect();
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let at = order.get(&Arc::as_ptr(&line.row)).copied().unwrap_or(index);
            slot_centre(slot_at(at, columns), size, cell(size))
        })
        .collect()
}

pub(crate) struct Layout {
    lines: Weak<Vec<Line>>,
    columns: usize,
    size: f32,
    cell: f32,
    moved: u64,
    pub(crate) tiles: Vec<Tile>,
    pub(crate) width: f32,
    pub(crate) height: f32,
    by_top: Vec<usize>,
}

impl Layout {
    fn visible(&self, top: f32, room: f32) -> Vec<usize> {
        let y = |index: &usize| f32::from(self.tiles[*index].1.y);
        let first = self
            .by_top
            .partition_point(|index| y(index) + 2. * self.cell <= top);
        let mut shown: Vec<usize> = self.by_top[first..]
            .iter()
            .take_while(|index| y(index) < top + room + self.cell)
            .copied()
            .collect();
        shown.sort_unstable();
        shown
    }

    fn centre(&self, index: usize) -> Point<Pixels> {
        let at = self.tiles[index].1;
        point(
            at.x + px(self.cell / 2.),
            at.y + px(ICON_TOP + self.size / 2.),
        )
    }
}

/// Where a folder's icons stand: those put here, then those Finder recorded.
struct Places {
    folder: &'static str,
    arrangement: Arrangement,
    own: HashMap<SharedString, Point<Pixels>>,
    finder: HashMap<SharedString, Point<Pixels>>,
}

impl Places {
    fn new(folder: &'static str, view: IconView) -> Self {
        let points = |spots: HashMap<String, (f32, f32)>| {
            spots
                .into_iter()
                .map(|(name, (x, y))| (SharedString::from(name), point(px(x), px(y))))
                .collect()
        };
        let arrangement = prefs::icon_arrangement(folder)
            .and_then(|id| Arrangement::of(&id))
            .or_else(|| Arrangement::of(view.arrange_by.as_deref()?))
            .or_else(|| Arrangement::of(prefs::finder().icon_arrange.as_deref()?))
            .unwrap_or(Arrangement::Free);
        Self {
            folder,
            arrangement,
            own: points(prefs::icon_spots(folder)),
            finder: points(view.locations),
        }
    }

    fn spot(&self, name: &SharedString) -> Option<Point<Pixels>> {
        self.own
            .get(name)
            .or_else(|| self.finder.get(name))
            .copied()
    }

    /// Finder fills a folder's free slots in name order.
    fn place(
        &mut self,
        lines: &[Line],
        columns: usize,
        size: f32,
        settled: bool,
    ) -> Vec<Point<Pixels>> {
        let cell = cell(size);
        let mut centres: Vec<Option<Point<Pixels>>> =
            lines.iter().map(|line| self.spot(&line.row.name)).collect();
        let taken: HashSet<Slot> = centres
            .iter()
            .flatten()
            .map(|at| nearest_slot(*at, size, cell))
            .collect();
        let mut waiting: Vec<usize> = (0..lines.len())
            .filter(|&index| centres[index].is_none())
            .collect();
        waiting.sort_by(|&a, &b| name_cmp(&lines[a].row.name, &lines[b].row.name));
        let mut free = (0..)
            .map(|index| slot_at(index, columns))
            .filter(|slot| !taken.contains(slot));
        for &index in &waiting {
            let centre = slot_centre(free.next().unwrap_or_default(), size, cell);
            centres[index] = Some(centre);
            if settled {
                self.own.insert(lines[index].row.name.clone(), centre);
            }
        }
        if settled && !waiting.is_empty() {
            self.keep(lines);
        }
        centres.into_iter().flatten().collect()
    }

    fn keep(&mut self, lines: &[Line]) {
        let listed: HashSet<&SharedString> = lines.iter().map(|line| &line.row.name).collect();
        self.own.retain(|name, _| listed.contains(name));
        self.save();
    }

    fn save(&self) {
        prefs::set_icon_spots(
            self.folder,
            self.own
                .iter()
                .map(|(name, at)| (name.as_ref(), (f32::from(at.x), f32::from(at.y)))),
        );
    }
}

#[derive(Default)]
pub(crate) struct Grid {
    places: Option<Places>,
    reading: Option<&'static str>,
    moved: u64,
    /// Finder leaves icons where it first put them, whatever the pane or the window does next.
    kept: Cell<Option<(usize, f32)>>,
    layout: Option<Rc<Layout>>,
    dragging: Option<SharedString>,
    pub(crate) scroll: ScrollHandle,
}

impl Grid {
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
        self.places = None;
        self.reading = None;
        self.kept.set(None);
        self.dragging = None;
    }

    fn read(&mut self, folder: &'static str, view: IconView) {
        self.places = Some(Places::new(folder, view));
        self.reading = None;
        self.moved += 1;
    }

    fn layout(
        &mut self,
        lines: &Arc<Vec<Line>>,
        columns: usize,
        size: f32,
        settled: bool,
    ) -> Rc<Layout> {
        let held = self.layout.clone().filter(|built| {
            std::ptr::eq(built.lines.as_ptr(), Arc::as_ptr(lines))
                && built.columns == columns
                && built.size == size
                && built.moved == self.moved
        });
        if let Some(built) = held {
            return built;
        }
        let cell = cell(size);
        let centres = match self.places.as_mut() {
            Some(places) => match places.arrangement {
                Arrangement::By(column) => arranged(lines, column, columns, size),
                Arrangement::Free | Arrangement::Grid => {
                    places.place(lines, columns, size, settled)
                }
            },
            None => (0..lines.len())
                .map(|index| slot_centre(slot_at(index, columns), size, cell))
                .collect(),
        };
        let tiles: Vec<Tile> = lines
            .iter()
            .zip(centres)
            .map(|(line, centre)| (line.key.clone(), origin(centre, size, cell)))
            .collect();
        let width = tiles
            .iter()
            .map(|(_, at)| f32::from(at.x) + cell)
            .fold(GRID_X + columns as f32 * cell, f32::max);
        let height = tiles
            .iter()
            .map(|(_, at)| f32::from(at.y) + cell)
            .fold(cell, f32::max);
        let mut by_top: Vec<usize> = (0..tiles.len()).collect();
        by_top.sort_by(|&a, &b| f32::from(tiles[a].1.y).total_cmp(&f32::from(tiles[b].1.y)));
        let built = Rc::new(Layout {
            lines: Arc::downgrade(lines),
            columns,
            size,
            cell,
            moved: self.moved,
            tiles,
            width,
            height,
            by_top,
        });
        self.layout = Some(built.clone());
        built
    }

    fn standing(&self) -> Option<(Rc<Layout>, Arc<Vec<Line>>)> {
        let layout = self.layout.clone()?;
        let lines = layout.lines.upgrade()?;
        Some((layout, lines))
    }

    fn put(&mut self, moves: impl IntoIterator<Item = (usize, Point<Pixels>)>) {
        let (Some((_, lines)), Some(places)) = (self.standing(), self.places.as_mut()) else {
            return;
        };
        for (index, centre) in moves {
            if let Some(line) = lines.get(index) {
                places.own.insert(line.row.name.clone(), centre);
            }
        }
        places.keep(&lines);
        self.moved += 1;
    }

    fn arrange(&mut self, arrangement: Arrangement) {
        let Some(places) = self.places.as_mut() else {
            return;
        };
        let was = std::mem::replace(&mut places.arrangement, arrangement);
        if was == arrangement {
            return;
        }
        prefs::set_icon_arrangement(places.folder, arrangement.id());
        self.moved += 1;
        let Some((layout, _)) = self.standing() else {
            return;
        };
        if let Arrangement::By(_) = was {
            self.put((0..layout.tiles.len()).map(|index| (index, layout.centre(index))));
        }
    }

    fn tidy(&mut self, chosen: impl Fn(&SharedString) -> bool) {
        let Some((layout, _)) = self.standing() else {
            return;
        };
        let (size, cell) = (layout.size, layout.cell);
        let (mut moving, staying): (Vec<usize>, Vec<usize>) =
            (0..layout.tiles.len()).partition(|&index| chosen(&layout.tiles[index].0));
        let mut taken: HashSet<Slot> = staying
            .iter()
            .map(|&index| nearest_slot(layout.centre(index), size, cell))
            .collect();
        let off = |index: usize| {
            let at = layout.centre(index);
            let on = slot_centre(nearest_slot(at, size, cell), size, cell);
            f32::from(at.x - on.x).hypot(f32::from(at.y - on.y))
        };
        moving.sort_by(|&a, &b| {
            let (at, to) = (layout.centre(a), layout.centre(b));
            off(a)
                .total_cmp(&off(b))
                .then(f32::from(at.y).total_cmp(&f32::from(to.y)))
                .then(f32::from(at.x).total_cmp(&f32::from(to.x)))
        });
        let moves: Vec<(usize, Point<Pixels>)> = moving
            .into_iter()
            .map(|index| {
                let slot = nearest_free(layout.centre(index), &taken, size, cell);
                taken.insert(slot);
                (index, slot_centre(slot, size, cell))
            })
            .collect();
        self.put(moves);
    }

    fn sort_into(&mut self, column: Column, columns: usize) {
        let Some((layout, lines)) = self.standing() else {
            return;
        };
        let centres = arranged(&lines, column, columns, layout.size);
        self.put(centres.into_iter().enumerate());
    }

    fn shift(&mut self, keys: &[SharedString], delta: Point<Pixels>) {
        let Some(arrangement) = self.places.as_ref().map(|places| places.arrangement) else {
            return;
        };
        let Some((layout, _)) = self.standing() else {
            return;
        };
        let dragged: Vec<usize> = (0..layout.tiles.len())
            .filter(|&index| keys.contains(&layout.tiles[index].0))
            .collect();
        let (size, cell) = (layout.size, layout.cell);
        let landed = |index: usize| {
            let at = layout.centre(index) + delta;
            point(at.x.max(px(cell / 2.)), at.y.max(px(ICON_TOP + size / 2.)))
        };
        let moves: Vec<(usize, Point<Pixels>)> = match arrangement {
            Arrangement::By(_) => return,
            Arrangement::Free => dragged
                .iter()
                .map(|&index| (index, landed(index)))
                .collect(),
            Arrangement::Grid => {
                let mut taken: HashSet<Slot> = (0..layout.tiles.len())
                    .filter(|index| !dragged.contains(index))
                    .map(|index| nearest_slot(layout.centre(index), size, cell))
                    .collect();
                dragged
                    .iter()
                    .map(|&index| {
                        let slot = nearest_free(landed(index), &taken, size, cell);
                        taken.insert(slot);
                        (index, slot_centre(slot, size, cell))
                    })
                    .collect()
            }
        };
        self.put(moves);
    }

    pub(crate) fn renamed(&mut self, from: &str, to: &str) {
        let Some(places) = self.places.as_mut() else {
            return;
        };
        if let Some(at) = places.spot(&SharedString::from(from.to_owned())) {
            places.own.insert(SharedString::from(to.to_owned()), at);
            places.save();
        }
    }

    pub(crate) fn reveal(&self, key: &SharedString, visible: gpui::Size<f32>) {
        let Some(layout) = &self.layout else {
            return;
        };
        let Some(at) = layout
            .tiles
            .iter()
            .find(|(tile, _)| tile == key)
            .map(|(_, at)| *at)
        else {
            return;
        };
        let cell = layout.cell;
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
    layout.iter().rposition(|(_, tile)| {
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

        let lines = match self.know_places(cx) {
            true => self.lines(false),
            false => Arc::default(),
        };
        let columns = self.columns(window);
        let size = self.icon_size as f32;
        let scale = self.scale;
        let cell = cell(size);
        let look = Look::new(palette);
        let this = cx.entity().downgrade();
        let scroll = self.icons.scroll.clone();
        let dragging = self.icons.dragging.clone();

        let settled = self
            .folders
            .get(self.here())
            .is_some_and(|folder| !folder.loading);
        let layout = self.icons.layout(&lines, columns, size, settled);
        let (width, height) = (layout.width, layout.height);

        let shift = self.bands.icons.shown();
        let mut canvas = div()
            .id("ng.finder.icons")
            .flex_1()
            .relative()
            .overflow_x_scroll()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .on_scroll_wheel(stretching(
                |this| (&mut this.bands.icons, this.icons.scroll.clone()),
                cx,
            ))
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
            let editor = (self.renaming.as_ref() == Some(&line.key))
                .then(|| rename_field(line, index, this.clone()).into_any_element());

            let tile = div()
                .id(("ng.finder.tile", index))
                .absolute()
                .left(at.x + shift.x)
                .top(at.y + shift.y)
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
                    },
                    {
                        let this = this.clone();
                        let line = line.clone();
                        move |drag: &TileDrag, _, _window, cx: &mut App| {
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
                    drop_hover::<TileDrag>(tile, line.key.clone(), into, None, this.clone()),
                    line.key.clone(),
                    into,
                    None,
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
            if !dropped.tiles.iter().any(|(key, _)| key == &drag.key) {
                return;
            }
            let landed = window.mouse_position()
                - drop_scroll.bounds().origin
                - drop_scroll.offset()
                - shift;
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
                    let group: Vec<SharedString> = match this.selected.contains(&drag.key) {
                        true => moved
                            .tiles
                            .iter()
                            .map(|(key, _)| key)
                            .filter(|key| this.selected.contains(*key))
                            .cloned()
                            .collect(),
                        false => vec![drag.key.clone()],
                    };
                    this.icons
                        .shift(&group, window.mouse_position() - this.pressed_at);
                    this.icons.dragging = None;
                    cx.notify();
                })
                .ok();
        });

        let hit_layout = layout.clone();
        let hit_scroll = scroll.clone();
        let hit = move |demo: &FinderDemo, at: Point<Pixels>| {
            let at = at - hit_scroll.offset() - shift;
            let index = tile_at(&hit_layout.tiles, at, cell)?;
            demo.lines(false).get(index).cloned()
        };

        let empty = layout;
        let empty_scroll = scroll;
        let this_menu = this.clone();
        // Hit-tested by hand: GPUI's drag is a bubble listener and misses a press a tile swallows.
        let canvas =
            canvas.on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                let at =
                    event.position - empty_scroll.bounds().origin - empty_scroll.offset() - shift;
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

    fn keeps_places(&self) -> bool {
        self.here().starts_with('/')
            && !self.results()
            && self.filtered.trim().is_empty()
            && self.type_filter.is_none()
    }

    /// Whether Finder's record of the folder's icons has been read.
    fn know_places(&mut self, cx: &mut Context<Self>) -> bool {
        let here = self.here();
        if !self.keeps_places() {
            self.icons.places = None;
            return true;
        }
        if self
            .icons
            .places
            .as_ref()
            .is_some_and(|places| places.folder == here)
        {
            return true;
        }
        if self.icons.reading == Some(here) {
            return false;
        }
        let (sent, answer) = mpsc::channel();
        std::thread::spawn(move || sent.send(ds_store::icon_view(Path::new(here))).ok());
        if let Ok(view) = answer.recv_timeout(FINDER_WAIT) {
            self.icons.read(here, view);
            return true;
        }
        self.icons.reading = Some(here);
        cx.spawn(async move |this, cx| {
            let view = loop {
                cx.background_executor().timer(FINDER_WAIT).await;
                match answer.try_recv() {
                    Ok(view) => break view,
                    Err(TryRecvError::Empty) => continue,
                    Err(TryRecvError::Disconnected) => return,
                }
            };
            this.update(cx, |this, cx| {
                if this.icons.reading == Some(here) {
                    this.icons.read(here, view);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        false
    }

    /// `None` for a listing that is not one folder's.
    pub(crate) fn arrangement(&self) -> Option<Arrangement> {
        let here = self.here();
        self.icons
            .places
            .as_ref()
            .filter(|places| places.folder == here)
            .map(|places| places.arrangement)
    }

    /// Finder cleans up only icons that are not kept sorted.
    pub(crate) fn cleans_up(&self) -> bool {
        self.view == ICON_VIEW
            && matches!(
                self.arrangement(),
                Some(Arrangement::Free | Arrangement::Grid)
            )
    }

    pub(crate) fn arrange(&mut self, arrangement: Arrangement, cx: &mut Context<Self>) {
        self.icons.arrange(arrangement);
        cx.notify();
    }

    pub(crate) fn icon_sort(&mut self, column: Column, cx: &mut Context<Self>) {
        self.arrange(Arrangement::By(column), cx);
    }

    pub(crate) fn clean_up(&mut self, selection: bool, cx: &mut Context<Self>) {
        if !self.cleans_up() {
            return;
        }
        let chosen = self.selected.snapshot();
        self.icons.tidy(|key| !selection || chosen.contains(key));
        cx.notify();
    }

    pub(crate) fn clean_up_by(&mut self, column: Column, window: &Window, cx: &mut Context<Self>) {
        if !self.cleans_up() {
            return;
        }
        let columns = self.fits(window);
        self.icons.sort_into(column, columns);
        cx.notify();
    }

    /// The nearest icon that way, straying off the row or column counting double.
    pub(crate) fn icon_step(&self, key: &str, from: Option<&SharedString>) -> Option<SharedString> {
        let layout = self.icons.layout.as_ref()?;
        let tiles = &layout.tiles;
        let reading = |a: &&Tile, b: &&Tile| {
            f32::from(a.1.y)
                .total_cmp(&f32::from(b.1.y))
                .then(f32::from(a.1.x).total_cmp(&f32::from(b.1.x)))
        };
        let Some(here) = from.and_then(|from| tiles.iter().find(|(tile, _)| tile == from)) else {
            let pick = match key {
                "down" | "right" => tiles.iter().min_by(reading),
                _ => tiles.iter().max_by(reading),
            };
            return pick.map(|(tile, _)| tile.clone());
        };
        let reach = |(_, at): &&Tile| {
            let (x, y) = (f32::from(at.x - here.1.x), f32::from(at.y - here.1.y));
            let (ahead, aside) = match key {
                "left" => (-x, y),
                "right" => (x, y),
                "up" => (-y, x),
                _ => (y, x),
            };
            (ahead > 0.5).then_some(ahead + 2. * aside.abs())
        };
        let next = tiles
            .iter()
            .filter_map(|tile| Some((reach(&tile)?, tile)))
            .min_by(|(a, _), (b, _)| a.total_cmp(b))
            .map_or(here, |(_, tile)| tile);
        Some(next.0.clone())
    }
}
