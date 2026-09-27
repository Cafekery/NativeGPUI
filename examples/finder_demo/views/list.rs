use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::mem;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, AnyView, App, Bounds, ClickEvent, Context, Div, Entity, ExternalPaths, Font,
    FontWeight, Hsla, MouseButton, MouseDownEvent, Pixels, Point, SharedString, Stateful,
    StyleRefinement, UniformListScrollHandle, WeakEntity, Window, canvas, div, point, prelude::*,
    px, uniform_list,
};
use native_gpui::tokens::{self, SystemColor};

use crate::app::FinderDemo;
use crate::art::{Anchor, Art, art_at, list_art, small_glyph};
use crate::data::{Column, Line, date_style, enclosing_name, kind_text, list_date, today};
use crate::dragdrop::{DragGhost, Lit, RowDrag, drop_target, flashed};
use crate::listing::Slot;
use crate::menus::rows_menu;
use crate::palette::Palette;
use crate::tags::{Tags, tag_dots};
use crate::ui::{axis_locked, menu_ring, pick_on_click, pick_on_press, rename_field, text_width};
use crate::views::header::{CELL_PAD, CELL_TRAIL, ColumnSpec, HEADER_H, PANE_INSET};

pub(crate) const ICON_SIZE: f32 = 32.;
pub(crate) const ICON_SMALL: f32 = 16.;
pub(crate) const ICON_X: f32 = 26.;
pub(crate) const INDENT: f32 = 15.;
pub(crate) const TWIST_W: f32 = 15.;
pub(crate) fn row_font() -> f32 {
    crate::prefs::finder().text_size
}
pub(crate) const ROW_PILL_RADIUS: f32 = 8.;
const SELECTION_JOIN_ALPHA: f32 = 0.1;
const SELECTION_JOIN_SHADE: f32 = 0.05;
const SELECTION_JOIN_LEAD: f32 = 15.;
const SELECTION_JOIN_TRAIL: f32 = 6.;
const SELECTION_DETAIL_ALPHA: f32 = 0.7;
pub(crate) const LIST_TOP_GAP: f32 = 5.;
pub(crate) const LIST_FOOT_GAP: f32 = 5.;
const LIST_END_GAP: f32 = 10.;
const BAND_INSET: f32 = 8.;
const LABEL_GAP: f32 = 6.;
const TAG_DOTS_GAP: f32 = 6.;

const CLOUD_ART: f32 = 13.;

const PILL_INSET: f32 = 10.;
const PILL_PAD_X: f32 = 8.;
const PILL_PAD_Y: f32 = 3.;
const PILL_FONT: f32 = 11.;
const PILL_RADIUS: f32 = 6.;
const WIDTHS_HELD: usize = 4096;

thread_local! {
    static DATES: Font = tokens::system_font_tabular(FontWeight::NORMAL);
}

#[derive(PartialEq)]
struct Look {
    icon: f32,
    row_h: f32,
    name_w: f32,
    scale: f32,
    label: Hsla,
    secondary: Hsla,
    stripe: Hsla,
    selection: Hsla,
    selection_text: Hsla,
    selection_detail: Hsla,
    selection_join: Hsla,
    focused: bool,
    dates: Font,
    date_style: usize,
    searching: bool,
    columns: Vec<ColumnSpec>,
    bleed: bool,
    day: f64,
}

struct Marks {
    selected: Arc<HashSet<SharedString>>,
    cut: Arc<HashSet<SharedString>>,
    expanded: Arc<HashSet<SharedString>>,
    tags: Tags,
    renaming: Option<SharedString>,
    menu_row: Option<SharedString>,
    cursor: Option<SharedString>,
    ql_icon: Anchor,
    anchor_held: bool,
    dropping: Option<SharedString>,
    springing: Option<SharedString>,
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Run {
    above: bool,
    join: Option<Hsla>,
}

/// Everything a row paints and handles: an unchanged row replays its last frame.
struct Drawn {
    line: Line,
    at: usize,
    run: Run,
    look: Rc<Look>,
    selected: bool,
    cut: bool,
    ring: bool,
    dropping: Option<Hsla>,
    tags: Option<Vec<usize>>,
    art: Art,
    anchor: Option<Anchor>,
    open: bool,
    this: WeakEntity<FinderDemo>,
}

impl PartialEq for Drawn {
    fn eq(&self, other: &Self) -> bool {
        self.line.key == other.line.key
            && Arc::ptr_eq(&self.line.row, &other.line.row)
            && self.line.depth == other.line.depth
            && self.at == other.at
            && (self.run, self.selected, self.cut, self.ring, self.open)
                == (other.run, other.selected, other.cut, other.ring, other.open)
            && self.anchor.is_some() == other.anchor.is_some()
            && self.dropping == other.dropping
            && self.tags == other.tags
            && self.art == other.art
            && self.look == other.look
    }
}

struct RowPaint(Drawn);

impl Render for RowPaint {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        shell(&self.0, painted(&self.0, None), window)
    }
}

#[derive(Default)]
pub(crate) struct Faces {
    look: Option<Rc<Look>>,
    held: HashMap<SharedString, (Entity<RowPaint>, bool)>,
    touched: HashSet<SharedString>,
    dirty: HashSet<SharedString>,
}

impl Faces {
    fn frame(&mut self, look: Look) -> Rc<Look> {
        self.held.retain(|_, (_, used)| mem::take(used));
        self.dirty = mem::take(&mut self.touched);
        match &self.look {
            Some(held) if **held == look => held.clone(),
            _ => self.look.insert(Rc::new(look)).clone(),
        }
    }

    /// Notifies, ahead of the frame, each row whose selection changed.
    pub(crate) fn touch(&mut self, selected: &HashSet<SharedString>, cx: &mut App) {
        for (key, (paint, _)) in &self.held {
            if paint.read(cx).0.selected != selected.contains(key)
                && self.touched.insert(key.clone())
            {
                paint.update(cx, |_, cx| cx.notify());
            }
        }
    }

    /// A changed row that was not notified before the frame draws uncached: a notify made during a
    /// draw lands a frame late, and a new view would drop the press or drag its row is part of.
    fn view(&mut self, drawn: Drawn, cx: &mut App) -> AnyElement {
        let style = StyleRefinement::default().w_full().h(px(drawn.look.row_h));
        let Some((paint, used)) = self.held.get_mut(&drawn.line.key) else {
            let key = drawn.line.key.clone();
            let paint = cx.new(|_| RowPaint(drawn));
            self.held.insert(key, (paint.clone(), true));
            return AnyView::from(paint).cached(style).into_any_element();
        };
        *used = true;
        let paint = paint.clone();
        if paint.read(cx).0 == drawn {
            return AnyView::from(paint).cached(style).into_any_element();
        }
        let notified = self.dirty.contains(&drawn.line.key);
        paint.update(cx, |paint, _| paint.0 = drawn);
        match notified {
            true => AnyView::from(paint).cached(style).into_any_element(),
            false => AnyView::from(paint).into_any_element(),
        }
    }
}

pub(crate) fn name_prefix(line: &Line, icon: f32) -> f32 {
    ICON_X + line.depth as f32 * INDENT + icon + LABEL_GAP
}

/// Finder's list: the taller of the icon and the list font's line, and 4 points.
pub(crate) fn row_h(icon: f32) -> f32 {
    static LINE: std::sync::LazyLock<f32> = std::sync::LazyLock::new(|| {
        let font = tokens::system_font_metrics(px(row_font()), FontWeight::NORMAL);
        (font.ascent + font.descent.abs()).ceil()
    });
    icon.max(*LINE) + 4.
}

impl FinderDemo {
    pub(crate) fn list_inset(&self) -> (usize, f32) {
        // Blank rows, not `uniform_list` padding, whose visible-range arithmetic under-counts it.
        let scope_h = if self.in_search() {
            self.search_bars_h()
        } else {
            0.
        };
        let inset = f32::from(self.metrics().top) + scope_h + HEADER_H + LIST_TOP_GAP;
        let row_h = self.row_h();
        let rows = (inset / row_h).ceil() as usize;
        (rows, rows as f32 * row_h - inset)
    }

    pub(crate) fn list_tail(&self) -> (usize, f32) {
        let foot = LIST_END_GAP;
        let row_h = self.row_h();
        let rows = (foot / row_h).ceil() as usize;
        (rows, rows as f32 * row_h - foot)
    }

    pub(crate) fn list_body(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let lines = self.lines(true);
        let this = cx.entity().downgrade();
        let icon = self.list_icon_now();
        let faces = self.faces.clone();
        let look = faces.borrow_mut().frame(Look {
            icon,
            row_h: row_h(icon),
            name_w: self.name_w,
            scale: self.scale,
            label: palette.label,
            secondary: palette.secondary,
            stripe: palette.stripe,
            selection: palette.selection,
            selection_text: palette.selection_text,
            selection_detail: palette.selection_text.opacity(SELECTION_DETAIL_ALPHA),
            selection_join: match palette.focused {
                true => tokens::system_color(SystemColor::AlternateSelectedControlText)
                    .opacity(SELECTION_JOIN_ALPHA),
                false => tokens::system_color(SystemColor::Shadow).opacity(SELECTION_JOIN_SHADE),
            },
            focused: palette.focused,
            dates: DATES.with(Font::clone),
            date_style: self
                .list_columns
                .iter()
                .find(|spec| spec.column == Column::Modified)
                .map_or(3, |spec| date_style(spec.width, row_font())),
            // Finder's search results show a path where the Kind column sits.
            searching: self.results(),
            columns: self.list_columns.clone(),
            bleed: self.list_bleeds(),
            day: today(),
        });
        let marks = Marks {
            selected: self.selected.snapshot(),
            cut: self.cut.snapshot(),
            expanded: self.expanded.snapshot(),
            tags: self.tags.clone(),
            renaming: self.renaming.clone(),
            menu_row: self.menu_row.clone(),
            cursor: self.cursor.clone(),
            ql_icon: self.ql_icon.clone(),
            anchor_held: self.tag_editor.is_some(),
            dropping: self.drop_row().cloned(),
            springing: self.spring_flash().cloned(),
        };

        let (pads, lift) = self.list_inset();
        let slots = self.slots(&lines);
        let hit = self.row_at(pads, look.row_h);
        let count = slots.as_ref().map_or(lines.len(), |slots| slots.len());
        let rows_end = px((pads + count) as f32 * look.row_h);

        // The count and the rows it indexes come from one read: a second read can be shorter.
        let (tails, trim) = self.list_tail();
        let items = pads + count + tails;
        let below = stripes(self.scroll.clone(), pads + (count | 1), &look);
        let list = uniform_list("ng.finder.list", items, move |range, window, cx| {
            let selected = |index: usize| {
                index.checked_sub(pads).is_some_and(|at| {
                    let at = match slots.as_ref().map(|slots| slots.get(at)) {
                        Some(Some(Slot::Row(at))) => *at,
                        Some(_) => return false,
                        None => at,
                    };
                    lines
                        .get(at)
                        .is_some_and(|line| marks.selected.contains(&line.key))
                })
            };
            range
                .map(|index| {
                    let Some(at) = index.checked_sub(pads).filter(|at| *at < count) else {
                        return div()
                            .id(("ng.finder.pad", index))
                            .h(px(look.row_h))
                            .w_full()
                            .into_any_element();
                    };
                    let at = match slots.as_ref().map(|slots| &slots[at]) {
                        Some(Slot::Header(title)) => {
                            return band(title.clone(), look.secondary, look.name_w, look.row_h)
                                .into_any_element();
                        }
                        Some(Slot::Row(at)) => *at,
                        None => at,
                    };
                    let run = Run {
                        above: index.checked_sub(1).is_some_and(&selected),
                        join: selected(index + 1).then_some(look.selection_join),
                    };
                    row(
                        &lines[at],
                        at,
                        run,
                        &look,
                        &marks,
                        &faces,
                        window,
                        cx,
                        this.clone(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .flex_1()
        .track_scroll(&self.scroll);
        let scroll = self.scroll.clone();

        div()
            .flex_1()
            .flex()
            .flex_col()
            .relative()
            .mt(px(-lift))
            .mb(px(-trim))
            .child(below)
            .child(rows_menu(
                "ng.finder.list.menu",
                cx.entity().downgrade(),
                hit,
                axis_locked(list),
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    let state = scroll.0.borrow();
                    let list = &state.base_handle;
                    let y = event.position.y - list.bounds().origin.y - list.offset().y;
                    if y >= rows_end && !event.modifiers.shift && !event.modifiers.platform {
                        this.deselect();
                        cx.notify();
                    }
                }),
            )
            .into_any_element()
    }

    fn row_at(
        &self,
        pads: usize,
        row_h: f32,
    ) -> impl Fn(&FinderDemo, Point<Pixels>) -> Option<Line> + use<> {
        let scroll = self.scroll.clone();
        move |demo, at| {
            let top = f32::from(scroll.0.borrow().base_handle.offset().y);
            let index = ((f32::from(at.y) - top) / row_h).floor();
            let at = (index.max(0.) as usize).checked_sub(pads)?;
            let lines = demo.lines(true);
            match demo.slots(&lines).as_ref().map(|slots| slots.get(at)) {
                Some(Some(Slot::Row(row))) => lines.get(*row).cloned(),
                Some(_) => None,
                None => lines.get(at).cloned(),
            }
        }
    }

    pub(crate) fn type_filter_pill(&self, palette: &Palette, bottom: f32) -> Option<AnyElement> {
        let text = self.type_filter.clone()?;
        Some(
            div()
                .absolute()
                .right(px(PILL_INSET))
                .bottom(px(bottom + PILL_INSET))
                .px(px(PILL_PAD_X))
                .py(px(PILL_PAD_Y))
                .rounded(px(PILL_RADIUS))
                .bg(tokens::system_color(SystemColor::QuaternarySystemFill))
                .font_family(tokens::system_font_family())
                .text_size(px(PILL_FONT))
                .text_color(palette.secondary)
                .child(text)
                .into_any_element(),
        )
    }
}

/// Finder's rows are inset, rounded pills until its columns overflow the list, then run edge to
/// edge, square.
fn edge(bleed: bool) -> (f32, f32) {
    match bleed {
        true => (0., 0.),
        false => (PANE_INSET, ROW_PILL_RADIUS),
    }
}

/// Finder's selection pill: a run of adjacent selected rows is one block, only its ends rounded,
/// with a hairline at each join, which stays put when the rows run edge to edge.
fn pill(fill: Hsla, run: Run, bleed: bool) -> Div {
    let (inset, corner) = edge(bleed);
    let radius = |end: bool| px(if end { corner } else { 0. });
    div()
        .absolute()
        .left(px(inset))
        .right(px(inset))
        .top_0()
        .bottom_0()
        .rounded_t(radius(!run.above))
        .rounded_b(radius(run.join.is_none()))
        .bg(fill)
        .when_some(run.join, |pill, colour| {
            pill.child(
                div()
                    .absolute()
                    .left(px(PANE_INSET - inset + SELECTION_JOIN_LEAD))
                    .right(px(PANE_INSET - inset + SELECTION_JOIN_TRAIL))
                    .bottom_0()
                    .h(px(1.))
                    .bg(colour),
            )
        })
}

/// Finder stripes the empty rows below the last one, into the overscroll, and nothing above the
/// first. Placed at paint from the bounds and offset the list's prepaint just laid its rows out at.
fn stripes(list: UniformListScrollHandle, first: usize, look: &Look) -> impl IntoElement {
    let (colour, pitch) = (look.stripe, look.row_h);
    let (inset, radius) = edge(look.bleed);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let state = list.0.borrow();
            let (rows, offset) = (state.base_handle.bounds(), state.base_handle.offset());
            let pitch = window.pixel_snap(px(pitch));
            for at in (first..).step_by(2) {
                let top = rows.top() + offset.y + pitch * at as f32;
                if top >= bounds.bottom() {
                    break;
                }
                window.paint_quad(
                    gpui::fill(
                        Bounds::from_corners(
                            point(rows.left() + px(inset), top),
                            point(rows.right() - px(inset), top + pitch),
                        ),
                        colour,
                    )
                    .corner_radii(px(radius)),
                );
            }
        },
    )
    .absolute()
    .size_full()
}

fn twist(
    line: &Line,
    at: usize,
    open: bool,
    lit: bool,
    this: WeakEntity<FinderDemo>,
) -> Stateful<Div> {
    let mut slot = div()
        .id(("ng.finder.twist", at))
        .w(px(TWIST_W))
        .h_full()
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    let arrow = if open {
        "chevron.down"
    } else {
        "chevron.right"
    };
    let color = if lit {
        SystemColor::AlternateSelectedControlText
    } else {
        SystemColor::SecondaryLabel
    };
    if let Some(glyph) = small_glyph(arrow, 9., FontWeight::BOLD, color) {
        slot = slot.child(glyph);
    }
    let into = line.row.opens;
    let key = line.key.clone();
    slot.on_click(move |event: &ClickEvent, _, cx| {
        this.update(cx, |this: &mut FinderDemo, cx| {
            this.disclose_at(at, &key, into, !open, event.modifiers().alt);
            cx.notify();
        })
        .ok();
    })
}

fn name_cell(drawn: &Drawn, rename: Option<AnyElement>) -> Div {
    let (line, look) = (&drawn.line, &drawn.look);
    let mut name = div()
        .w(px(look.name_w))
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .whitespace_nowrap()
        .child(
            div()
                .w(px(ICON_X + line.depth as f32 * INDENT))
                .h_full()
                .flex_none(),
        )
        .child(art_at(
            drawn.art.element(look.icon),
            look.icon,
            drawn.anchor.clone(),
        ))
        .child(match rename {
            Some(field) => div().ml(px(LABEL_GAP)).flex_1().child(field),
            None => div()
                .ml(px(LABEL_GAP))
                .overflow_hidden()
                .text_ellipsis_middle()
                .child(line.row.display.clone()),
        });
    if let Some(dots) = drawn.tags.as_deref().and_then(tag_dots) {
        name = name.child(div().ml(px(TAG_DOTS_GAP)).child(dots));
    }
    if line.row.dataless {
        name = name.child(div().flex_1());
        if let Some(cloud) = small_glyph(
            "icloud",
            CLOUD_ART,
            FontWeight::NORMAL,
            SystemColor::SecondaryLabel,
        ) {
            name = name.child(div().pr(px(CELL_TRAIL)).child(cloud));
        }
    }
    name
}

fn detail_cell(spec: &ColumnSpec, line: &Line, look: &Look, detail: Hsla) -> Div {
    let cell = div().w(px(spec.width)).flex_none().text_color(detail);
    match spec.column {
        Column::Size => cell
            .pr(px(CELL_TRAIL))
            .flex()
            .justify_end()
            .font(look.dates.clone())
            .child(line.row.size.clone()),
        Column::Modified => cell
            .pl(px(CELL_PAD))
            .when(look.date_style >= 2, |cell| cell.font(look.dates.clone()))
            .whitespace_nowrap()
            .overflow_hidden()
            .child(list_date(line.row.modified_at, look.date_style)),
        Column::Kind => cell
            .pl(px(CELL_PAD))
            .whitespace_nowrap()
            .overflow_hidden()
            .map(|cell| match look.searching {
                true => cell.text_ellipsis().child(enclosing_name(&line.key)),
                false => cell.text_ellipsis_middle().child(kind_text(
                    &line.row.kind,
                    spec.width - 20.,
                    row_font(),
                )),
            }),
        // `list_columns` never holds Name; this only keeps the match exhaustive.
        Column::Name => cell,
    }
}

/// Finder takes the disclosure triangle, the icon and the name it labels, and no more of the row.
fn drop_band(line: &Line, look: &Look, window: &Window) -> (f32, f32) {
    thread_local! {
        static WIDTHS: RefCell<HashMap<SharedString, f32>> = RefCell::new(HashMap::new());
    }
    let display = &line.row.display;
    let known = WIDTHS.with_borrow(|widths| widths.get(display).copied());
    let text = known.unwrap_or_else(|| {
        let width = text_width(window, display.clone(), px(row_font()));
        WIDTHS.with_borrow_mut(|widths| {
            if widths.len() >= WIDTHS_HELD {
                widths.clear();
            }
            widths.insert(display.clone(), width);
        });
        width
    });
    let twist = ICON_X + line.depth as f32 * INDENT;
    let name = twist + look.icon + LABEL_GAP + text + LABEL_GAP;
    (twist, name.min(look.name_w))
}

fn painted(drawn: &Drawn, rename: Option<AnyElement>) -> Div {
    let look = &drawn.look;
    let over = drawn.selected && look.focused;
    let detail = match drawn.dropping.is_some() || over {
        true => look.selection_detail,
        false => look.secondary,
    };
    let fill = match (drawn.selected, drawn.at % 2 == 1) {
        (true, _) => Some((look.selection, drawn.run)),
        (false, true) => Some((look.stripe, Run::default())),
        (false, false) => None,
    };
    let mut row = div()
        .size_full()
        .relative()
        .flex()
        .items_center()
        .children(fill.map(|(fill, run)| pill(fill, run, look.bleed)))
        .children(
            drawn
                .dropping
                .map(|fill| pill(fill, Run::default(), look.bleed)),
        )
        .children(drawn.ring.then(|| {
            let colour = if over {
                look.selection_text
            } else {
                look.selection
            };
            // Finder's edge-to-edge ring stands a point outside its row.
            let (inset, radius) = edge(look.bleed);
            let outset = if look.bleed { -1. } else { 0. };
            menu_ring(
                colour,
                (inset + outset, outset),
                radius,
                (drawn.run.above, drawn.run.join.is_some()),
                over,
            )
        }))
        .child(name_cell(drawn, rename));
    for spec in &look.columns {
        row = row.child(detail_cell(spec, &drawn.line, look, detail));
    }
    row.child(div().w(px(PANE_INSET)).flex_none())
}

#[allow(clippy::too_many_arguments)]
fn row(
    line: &Line,
    at: usize,
    run: Run,
    look: &Rc<Look>,
    marks: &Marks,
    faces: &RefCell<Faces>,
    window: &Window,
    cx: &mut App,
    this: WeakEntity<FinderDemo>,
) -> AnyElement {
    let selected = marks.selected.contains(&line.key);
    let dropping = (marks.dropping.as_ref() == Some(&line.key)).then(|| {
        match marks.springing.as_ref() == Some(&line.key) {
            true => flashed(look.selection),
            false => look.selection,
        }
    });
    let ring = marks
        .menu_row
        .as_ref()
        .is_some_and(|row| *row == line.key || (selected && marks.selected.contains(row)));
    let cursor = marks.cursor.as_ref() == Some(&line.key);
    let drawn = Drawn {
        line: line.clone(),
        at,
        run: if selected || ring {
            run
        } else {
            Run::default()
        },
        look: look.clone(),
        selected,
        cut: marks.cut.contains(&line.key),
        ring,
        dropping,
        tags: marks.tags.get(&line.key).cloned(),
        art: list_art(&line.row, look.icon, look.scale),
        anchor: cursor.then(|| marks.ql_icon.clone()),
        open: marks.expanded.contains(&line.key),
        this,
    };
    let renaming = marks.renaming.as_ref() == Some(&line.key);
    // A popover's anchor and a rename field are AppKit views, placed only by a row that paints.
    match renaming || (cursor && marks.anchor_held) {
        true => {
            let rename =
                renaming.then(|| rename_field(line, at, drawn.this.clone()).into_any_element());
            shell(&drawn, painted(&drawn, rename), window)
        }
        false => faces.borrow_mut().view(drawn, cx),
    }
}

fn shell(drawn: &Drawn, pixels: Div, window: &Window) -> AnyElement {
    let (line, look, at, this) = (&drawn.line, &drawn.look, drawn.at, &drawn.this);
    let marked = drawn.selected || drawn.dropping.is_some();
    let twist = (line.row.discloses && line.row.readable).then(|| {
        twist(
            line,
            at,
            drawn.open,
            drawn.dropping.is_some() || (drawn.selected && look.focused),
            this.clone(),
        )
        .absolute()
        .top_0()
        .left(px(ICON_X + line.depth as f32 * INDENT - TWIST_W))
    });
    let row = div()
        .id(("ng.finder.row", at))
        .w_full()
        .h(px(look.row_h))
        .relative()
        .font_family(tokens::system_font_family())
        .text_size(px(row_font()))
        .text_color(if marked {
            look.selection_text
        } else {
            look.label
        })
        .when(drawn.cut, |row| row.opacity(0.5))
        .child(pixels)
        .children(twist)
        .on_mouse_down(MouseButton::Left, pick_on_press(line, this.clone()))
        .on_click(pick_on_click(line, this.clone()))
        .on_drag(
            RowDrag {
                key: line.key.clone(),
            },
            move |_, _, _, cx| cx.new(|_| DragGhost),
        );
    let Some(into) = line.row.opens else {
        return row.into_any_element();
    };
    let band = Some(drop_band(line, look, window));
    let key = line.key.clone();
    drop_target::<ExternalPaths>(
        drop_target::<RowDrag>(row, key.clone(), into, band, Lit::Drawn, this.clone()),
        key,
        into,
        band,
        Lit::Drawn,
        this.clone(),
    )
    .into_any_element()
}

fn band(title: SharedString, colour: Hsla, name_w: f32, row_h: f32) -> Div {
    div()
        .h(px(row_h))
        .w(px(name_w))
        .flex()
        .items_end()
        .pb(px(2.))
        .pl(px(BAND_INSET))
        .child(
            div()
                .text_color(colour)
                .text_size(tokens::small_system_font_size())
                .font_weight(FontWeight::BOLD)
                .child(title),
        )
}
