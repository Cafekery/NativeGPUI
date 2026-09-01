//! The icon (grid) view.
//!
//! Owns `FinderDemo::icon_body`, the cell metrics, and [`Grid`] — the per-item positions a drag
//! writes. The grid's column count is `FinderDemo::columns`, which lives in `main` because the
//! keyboard needs it too.
//!
//! What real Finder draws here and this cannot is the **content thumbnail** — a movie's own first
//! frame, a text file's own words. Those come from `QLThumbnailGenerator`, which needs a file on
//! disk, and nothing in this demo reads the filesystem. A type lookup is the whole of what is
//! reachable, so a `.mov` shows the QuickTime document artwork where Finder shows the video.
//!
//! Deliberately not here: the gallery view, which is a different arrangement of the same rows and
//! lives in `gallery`.

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

/// How much wider and taller than its icon one grid cell is. Icon size and grid spacing are two
/// separate sliders in Finder's View Options, so the surround is a constant that does not scale
/// with the icon: at the stock spacing the cell is the icon plus 48pt, which is the 112 x 112 grid
/// a 64pt icon sits in. The pitch is the cell — there is no gap between cells to leave out.
const CELL_PAD: f32 = 48.;
/// The icon box hangs 14pt below the cell top, which leaves 34pt underneath for the label. So the
/// icon is above the cell's centre line rather than on it, and the two numbers stay put as the
/// icon grows because only the icon does.
const ICON_TOP: f32 = 14.;
/// The first column's centre sits 10pt in from the pane's leading edge, and the first row is flush
/// with the top of the body.
const GRID_X: f32 = 10.;
/// Finder's own `textSize` for the icon view, which is not a system metric: it is a per-folder
/// view option that defaults to 12 where the list view's defaults to 13.
const LABEL_FONT: f32 = 12.;
const LABEL_LINE: f32 = 16.;
/// A name too long for two lines is elided in the **middle**, not at the end, so the extension
/// survives — which is the whole reason to truncate a filename rather than a sentence.
const LABEL_LINES: usize = 2;
/// The label runs almost the full width of the cell, leaving half a point either side, so a name
/// that fits on one line in Finder must not be wrapped here for want of room.
const LABEL_INSET: f32 = 0.5;
const ICON_LABEL_GAP: f32 = 6.;
const LABEL_RADIUS: f32 = 5.;
/// The selected icon's plate is the icon box grown 4pt on every side, so it is 72pt around a 64pt
/// icon and does not scale with anything else.
const PLATE_PAD: f32 = 4.;
const PLATE_RADIUS: f32 = 8.;
/// The drag image and the icon left behind. Finder draws both translucent; the exact alphas are
/// not measurable without driving a real Finder window, so these are chosen to read the same way.
const GHOST_ALPHA: f32 = 0.7;
const SOURCE_ALPHA: f32 = 0.35;

/// The pitch of the grid, which is also the width and height of one cell.
pub(crate) fn cell(icon: f32) -> f32 {
    icon + CELL_PAD
}

/// The four colours a tile draws with, so the drag image can be built away from a [`Palette`].
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
            // A selected item wears two separate shapes and only one of them is a selection
            // colour: the label takes the accent pill, and the icon sits on a plate that is the
            // faintest fill in the palette and does not change when the window loses key. Picking
            // a selection colour for the plate would tint the icon on focus, which Finder never
            // does. Measured over the body it is white at alpha 0.098; `separatorColor` resolves
            // to that same value in dark, so this build cannot tell the two apart.
            backdrop: system_color(SystemColor::QuaternaryLabel),
        }
    }
}

/// Where each item sits, and the drag in flight.
///
/// Finder's icon view stores one position per item rather than an order, which is why a dropped
/// icon stays exactly where it was let go and nothing around it reflows. Items nobody has moved
/// have no entry and fall into the next auto-placed slot, filling left to right.
#[derive(Default)]
pub(crate) struct Grid {
    placed: HashMap<SharedString, Point<Pixels>>,
    dragging: Option<SharedString>,
    scroll: ScrollHandle,
}

impl Grid {
    /// The auto-placed slot for the `index`th item, which is what an item that has never been
    /// dragged uses.
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

    /// A folder change invalidates every placement: the keys are folder-scoped, so keeping them
    /// would only leak memory, and a re-entered folder should look the way it was left.
    pub(crate) fn forget(&mut self) {
        self.placed.clear();
        self.dragging = None;
    }

    /// Scroll the `index`th item into view. The grid is a canvas rather than a uniform list, so
    /// the row a keyboard step lands on is arithmetic here rather than an index the list resolves.
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

/// What a drag carries. The grab offset is filled in by GPUI when the drag starts — after the
/// payload is built — so it travels in a cell the drop handler reads back.
struct TileDrag {
    key: SharedString,
    grab: Rc<Cell<Point<Pixels>>>,
}

/// The drag image: the same tile, translucent, drawn by GPUI at the cursor with the grab offset
/// preserved so the icon does not jump under the pointer when the drag begins.
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

/// One cell's contents: the icon over its label, both centred, with the icon high in the cell.
fn tile(line: &Line, size: f32, look: Look, selected: bool, tag: Option<usize>) -> Div {
    let cell = cell(size);
    // The plate is drawn out of flow, inset negatively from the icon box, so growing it by 4pt a
    // side does not push the icon 4pt down the cell and take the label with it.
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

/// Middle elision, keeping the extension — which is the whole reason to truncate a filename
/// rather than a sentence. Finder wraps first and then elides only the **last** line, so its head
/// runs further than this one does when the first line breaks early; the tail is the part that
/// carries the meaning and it is eight characters either way.
///
/// The budget is a character count off the label's width, because a GPUI element cannot measure
/// its own text before it is laid out. It is deliberately tight: eliding a name a little sooner
/// than Finder would is invisible, where being one character generous hands the string to the
/// line clamp, which cuts the extension off the end.
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
        // A drag that ends anywhere but on the canvas never reaches the drop handler, so the
        // dimmed source is cleared from the drag's own absence rather than from a cancel event.
        if !cx.has_active_drag() {
            self.icons.dragging = None;
        }

        let lines = self.lines(false);
        let columns = self.columns(window, cx);
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
            // In flow, and the only child that is: the tiles are absolute and so contribute no
            // height, which would leave the canvas unscrollable however far down the last row is.
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

            // Finder selects on press, not on release, so the drag image already shows the
            // selection the drop will move. A modified click is left to the click handler, which
            // is where extend and toggle are resolved.
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
                    // Finder carries the whole selection, keeping the icons' offsets from each
                    // other, so a group arrives in the shape it left in.
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

        // A press on bare canvas clears the selection. The tiles are hit-tested here rather than
        // by stopping propagation in each one, because the drag GPUI starts is itself a
        // bubble-phase mouse listener and would never see a press that a tile swallowed.
        let empty = layout;
        let empty_scroll = scroll;
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
