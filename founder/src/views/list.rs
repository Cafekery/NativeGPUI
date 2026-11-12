use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::mem;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, AnyView, App, ClickEvent, Context, Div, Entity, Font, FontWeight, Hsla,
    MouseButton, MouseDownEvent, Pixels, Point, SharedString, Stateful, StyleRefinement,
    UniformListScrollHandle, WeakEntity, Window, div, prelude::*, px, uniform_list,
};
use native_gpui::native_tooltip;
use native_gpui::tokens::{self, SystemColor};

use crate::app::FinderDemo;
use crate::art::{Anchor, Art, art_at, list_art, marked, small_glyph};
use crate::data::{Column, Line, Row, date_style, intern, kind_text, list_date, today};
use crate::dragdrop::{RowDrag, RowGhost, flashed};
use crate::elastic::{shifted, stretching};
use crate::listing::Slot;
use crate::menus::rows_menu;
use crate::palette::Palette;
use crate::prefs;
use crate::tags::{RowTag, row_tags, tag_dots, tag_dots_w};
use crate::ui::{axis_locked, menu_ring, pick_on_click, pick_on_press, rename_field, text_width};
use crate::view::LIST_VIEW;
use crate::views::header::{ColumnSpec, HEADER_H, PANE_INSET};
use crate::views::{chevron, folder_drop, run_pill};

pub(crate) const ICON_SIZE: f32 = 32.;
pub(crate) const ICON_SMALL: f32 = 16.;
pub(crate) const ICON_X: f32 = 26.;
const INDENT: f32 = 15.;
pub(crate) const TWIST_W: f32 = 15.;
const ROW_PILL_RADIUS: f32 = 8.;
const SELECTION_JOIN_ALPHA: f32 = 0.1;
const SELECTION_JOIN_SHADE: f32 = 0.05;
const SELECTION_JOIN_LEAD: f32 = 15.;
const SELECTION_JOIN_TRAIL: f32 = 6.;
const SELECTION_DETAIL_ALPHA: f32 = 0.7;
const LIST_TOP_GAP: f32 = 5.;
pub(crate) const LIST_FOOT_GAP: f32 = 5.;
const LIST_END_GAP: f32 = 10.;
const BAND_INSET: f32 = 8.;
const LABEL_GAP: f32 = 6.;
const TAG_DOTS_GAP: f32 = 6.;
const CELL_PAD: f32 = 7.;
const CELL_TRAIL: f32 = 6.;
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

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct ListLook {
    pub(crate) text: f32,
    pub(crate) icon: f32,
    pub(crate) relative: bool,
    pub(crate) sizes: bool,
}

impl ListLook {
    pub(crate) fn of(folder: &str) -> Self {
        let finder = prefs::finder();
        let number = |key| prefs::look_number(LIST_VIEW, folder, key).map(|value| value as f32);
        let on = |key| prefs::look_on(LIST_VIEW, folder, key);
        Self {
            text: number("textSize").unwrap_or(finder.text_size),
            icon: number("iconSize").unwrap_or(finder.icon_size),
            relative: on("useRelativeDates").unwrap_or(finder.relative_dates),
            sizes: on("calculateAllSizes").unwrap_or(finder.calculate_all_sizes),
        }
        .clamped()
    }

    fn clamped(self) -> Self {
        Self {
            text: self.text.round().clamp(10., 16.),
            icon: if self.icon > ICON_SMALL {
                ICON_SIZE
            } else {
                ICON_SMALL
            },
            ..self
        }
    }

    fn keep(self, folder: &str, was: Self) {
        for (key, value, before) in [
            ("textSize", self.text, was.text),
            ("iconSize", self.icon, was.icon),
        ] {
            if value != before {
                prefs::set_look_number(LIST_VIEW, folder, key, f64::from(value));
            }
        }
        for (key, on, before) in [
            ("useRelativeDates", self.relative, was.relative),
            ("calculateAllSizes", self.sizes, was.sizes),
        ] {
            if on != before {
                prefs::set_look_on(LIST_VIEW, folder, key, on);
            }
        }
    }
}

#[derive(PartialEq)]
struct Look {
    icon: f32,
    text: f32,
    relative: bool,
    sizes: bool,
    row_h: f32,
    name_w: f32,
    scale: f32,
    previews: bool,
    label: Hsla,
    secondary: Hsla,
    stripe: Hsla,
    selection: Hsla,
    selection_text: Hsla,
    selection_detail: Hsla,
    dim: Hsla,
    selection_dim: Hsla,
    selection_join: Hsla,
    focused: bool,
    dates: Font,
    searching: bool,
    columns: Vec<ColumnSpec>,
    bleed: bool,
    day: f64,
}

struct Marks {
    selected: Arc<HashSet<SharedString>>,
    expanded: Arc<HashSet<SharedString>>,
    renaming: Option<SharedString>,
    typed_ahead: Option<String>,
    rename_draft: Option<(SharedString, String)>,
    menu_row: Option<SharedString>,
    cursor: Option<SharedString>,
    ql_icon: Anchor,
    anchor_held: bool,
    dropping: Option<SharedString>,
    springing: Option<SharedString>,
    faces: Rc<RefCell<Faces>>,
    this: WeakEntity<FinderDemo>,
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Run {
    above: bool,
    join: Option<Hsla>,
}

struct Drawn {
    line: Line,
    at: usize,
    run: Run,
    look: Rc<Look>,
    selected: bool,
    ring: bool,
    dropping: Option<Hsla>,
    tags: Vec<RowTag>,
    art: Art,
    anchor: Option<Anchor>,
    open: bool,
    clipped: bool,
    this: WeakEntity<FinderDemo>,
}

impl Drawn {
    fn lit(&self) -> bool {
        self.dropping.is_some() || (self.selected && self.look.focused)
    }

    fn dim(&self) -> Option<Hsla> {
        let look = &self.look;
        self.line.row.hidden.then_some(if self.lit() {
            look.selection_dim
        } else {
            look.dim
        })
    }
}

impl PartialEq for Drawn {
    fn eq(&self, other: &Self) -> bool {
        self.line.key == other.line.key
            && Arc::ptr_eq(&self.line.row, &other.line.row)
            && self.line.depth == other.line.depth
            && self.at == other.at
            && (self.run, self.selected, self.ring, self.open, self.clipped)
                == (
                    other.run,
                    other.selected,
                    other.ring,
                    other.open,
                    other.clipped,
                )
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

    pub(crate) fn touch(&mut self, selected: &HashSet<SharedString>, cx: &mut App) {
        for (key, (paint, _)) in &self.held {
            if paint.read(cx).0.selected != selected.contains(key)
                && self.touched.insert(key.clone())
            {
                paint.update(cx, |_, cx| cx.notify());
            }
        }
    }

    /// A changed row not notified before the frame draws uncached: a notify mid-draw lands late.
    fn view(&mut self, drawn: Drawn, cx: &mut App) -> AnyElement {
        let row_h = drawn.look.row_h;
        let (paint, cached) = match self.held.get_mut(&drawn.line.key) {
            None => {
                let key = drawn.line.key.clone();
                let paint = cx.new(|_| RowPaint(drawn));
                self.held.insert(key, (paint.clone(), true));
                (paint, true)
            }
            Some((paint, used)) => {
                *used = true;
                let paint = paint.clone();
                if paint.read(cx).0 == drawn {
                    (paint, true)
                } else {
                    let notified = self.dirty.contains(&drawn.line.key);
                    paint.update(cx, |paint, _| paint.0 = drawn);
                    (paint, notified)
                }
            }
        };
        let view = AnyView::from(paint);
        if cached {
            view.cached(StyleRefinement::default().w_full().h(px(row_h)))
                .into_any_element()
        } else {
            view.into_any_element()
        }
    }
}

fn indent(line: &Line) -> f32 {
    ICON_X + line.depth as f32 * INDENT
}

pub(crate) fn name_prefix(line: &Line, icon: f32) -> f32 {
    indent(line) + icon + label_gap(icon)
}

fn label_gap(icon: f32) -> f32 {
    LABEL_GAP + (icon - ICON_SMALL) / 8.
}

pub(crate) fn line_h(text: f32) -> f32 {
    thread_local! {
        static LINE: Cell<(u32, f32)> = const { Cell::new((0, 0.)) };
    }
    let (size, mut line) = LINE.get();
    if size != text.to_bits() {
        let font = tokens::system_font_metrics(px(text), FontWeight::NORMAL);
        line = (font.ascent + font.descent.abs()).ceil();
        LINE.set((text.to_bits(), line));
    }
    line
}

pub(crate) fn row_h(icon: f32, text: f32) -> f32 {
    icon.max(line_h(text)) + 4.
}

/// Blank rows, not `uniform_list` padding, whose visible-range arithmetic under-counts it.
fn pad_rows(gap: f32, row_h: f32) -> (usize, f32) {
    let rows = (gap / row_h).ceil() as usize;
    (rows, rows as f32 * row_h - gap)
}

fn slot_line(slots: &Option<Arc<Vec<Slot>>>, at: usize) -> Option<usize> {
    match slots.as_deref().map(|slots| slots.get(at)) {
        Some(Some(Slot::Row(row))) => Some(*row),
        Some(_) => None,
        None => Some(at),
    }
}

impl FinderDemo {
    pub(crate) fn change_list_look(
        &mut self,
        change: impl FnOnce(&mut ListLook),
        cx: &mut Context<Self>,
    ) {
        let was = self.list_look;
        let mut look = was;
        change(&mut look);
        let look = look.clamped();
        if look == was {
            return;
        }
        look.keep(self.here(), was);
        if look.icon != was.icon {
            self.list_zoom = Some((self.list_icon_now(), look.icon, Instant::now()));
        }
        self.list_look = look;
        self.name_w = self.name_w.max(self.name_min_w());
        if look.sizes != was.sizes {
            if self.sort == Column::Size {
                self.sort_held(self.order(), cx);
            }
            let shown: Vec<&'static str> = (self.expanded.iter().map(|key| intern(key)))
                .chain([self.here()])
                .collect();
            for id in shown {
                self.weigh_folders(id, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn list_inset(&self) -> (usize, f32) {
        let scope_h = if self.in_search() {
            self.search_bars_h()
        } else {
            0.
        };
        let inset = f32::from(self.metrics().top) + scope_h + HEADER_H + LIST_TOP_GAP;
        pad_rows(inset, self.row_h())
    }

    pub(crate) fn list_body(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let lines = self.lines(true);
        let icon = self.list_icon_now();
        let faces = self.faces.clone();
        let look = faces.borrow_mut().frame(Look {
            icon,
            text: self.list_look.text,
            relative: self.list_look.relative,
            sizes: self.list_look.sizes,
            row_h: row_h(icon, self.list_look.text),
            name_w: self.name_w,
            scale: self.scale,
            previews: self.previews.list,
            label: palette.label,
            secondary: palette.secondary,
            stripe: palette.stripe,
            selection: palette.selection,
            selection_text: palette.selection_text,
            selection_detail: palette.selection_text.opacity(SELECTION_DETAIL_ALPHA),
            dim: palette.dim(false),
            selection_dim: palette.dim(true),
            selection_join: if palette.focused {
                tokens::system_color(SystemColor::AlternateSelectedControlText)
                    .opacity(SELECTION_JOIN_ALPHA)
            } else {
                tokens::system_color(SystemColor::Shadow).opacity(SELECTION_JOIN_SHADE)
            },
            focused: palette.focused,
            dates: DATES.with(Font::clone),
            searching: self.flat_results(),
            columns: self.shown_columns(),
            bleed: self.list_bleeds(),
            day: today(),
        });
        let marks = Marks {
            selected: self.selected.snapshot(),
            expanded: self.expanded.snapshot(),
            renaming: self.renaming.clone(),
            typed_ahead: self.typed_ahead.clone(),
            rename_draft: self.rename_draft.clone(),
            menu_row: self.menu_row.clone(),
            cursor: self.cursor.clone(),
            ql_icon: self.ql_icon.clone(),
            anchor_held: self.tag_editor.is_some() || self.customizer.is_some(),
            dropping: self.drop_row().cloned(),
            springing: self.spring_flash().cloned(),
            faces,
            this: cx.entity().downgrade(),
        };

        let (pads, lift) = self.list_inset();
        let slots = self.slots(&lines);
        let hit = self.row_at(pads, look.row_h);
        let count = slots.as_ref().map_or(lines.len(), |slots| slots.len());
        let rows_end = px((pads + count) as f32 * look.row_h);
        let stretch = f32::from(self.bands.rows.shown().y);

        // The count and the rows it indexes come from one read: a second read can be shorter.
        let (tails, trim) = pad_rows(LIST_END_GAP, look.row_h);
        let items = pads + count + tails;
        let seen = (self.seen_items(&self.scroll, look.row_h, items))
            .filter_map(|index| index.checked_sub(pads).filter(|at| *at < count))
            .filter_map(|at| slot_line(&slots, at));
        self.report_scene(cx, self.here(), &lines, seen, icon, self.previews.list);
        let list = uniform_list("ng.finder.list", items, move |range, window, cx| {
            let selected = |index: usize| {
                index
                    .checked_sub(pads)
                    .and_then(|at| slot_line(&slots, at))
                    .and_then(|at| lines.get(at))
                    .is_some_and(|line| marks.selected.contains(&line.key))
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
                            return group_title(title.clone(), &look).into_any_element();
                        }
                        Some(Slot::Row(at)) => *at,
                        None => at,
                    };
                    let run = Run {
                        above: index.checked_sub(1).is_some_and(&selected),
                        join: selected(index + 1).then_some(look.selection_join),
                    };
                    row(&lines[at], at, run, &look, &marks, window, cx)
                })
                .map(|row| shifted(row, px(stretch)))
                .collect::<Vec<_>>()
        })
        .flex_1()
        .track_scroll(&self.scroll)
        .on_scroll_wheel(stretching(
            |this| {
                (
                    &mut this.bands.rows,
                    this.scroll.0.borrow().base_handle.clone(),
                )
            },
            cx,
        ));
        let scroll = self.scroll.clone();

        div()
            .flex_1()
            .flex()
            .flex_col()
            .relative()
            .mt(px(-lift))
            .mb(px(-trim))
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
                    let y = event.position.y
                        - list.bounds().origin.y
                        - list.offset().y
                        - this.bands.rows.shown().y;
                    if y >= rows_end && !event.modifiers.shift && !event.modifiers.platform {
                        this.deselect();
                        cx.notify();
                    }
                }),
            )
            .into_any_element()
    }

    /// From the scroll offset, not the list's own range: it also asks for row 0 to measure it.
    pub(crate) fn seen_items(
        &self,
        handle: &UniformListScrollHandle,
        row_h: f32,
        items: usize,
    ) -> Range<usize> {
        let state = handle.0.borrow();
        let top = -f32::from(state.base_handle.offset().y);
        let tall = f32::from(state.base_handle.bounds().size.height);
        let tall = if tall > 0. {
            tall
        } else {
            f32::from(self.viewport.height)
        };
        let first = (top / row_h).floor().max(0.) as usize;
        let end = ((top + tall) / row_h).ceil().max(0.) as usize;
        first.min(items)..end.min(items)
    }

    fn row_at(
        &self,
        pads: usize,
        row_h: f32,
    ) -> impl Fn(&FinderDemo, Point<Pixels>) -> Option<Line> + use<> {
        let scroll = self.scroll.clone();
        move |demo, at| {
            let top =
                f32::from(scroll.0.borrow().base_handle.offset().y + demo.bands.rows.shown().y);
            let index = ((f32::from(at.y) - top) / row_h).floor();
            let at = (index.max(0.) as usize).checked_sub(pads)?;
            let lines = demo.lines(true);
            lines.get(slot_line(&demo.slots(&lines), at)?).cloned()
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

fn edge(bleed: bool) -> (f32, f32) {
    if bleed {
        (0., 0.)
    } else {
        (PANE_INSET, ROW_PILL_RADIUS)
    }
}

fn pill(fill: Hsla, run: Run, bleed: bool) -> Div {
    let (inset, corner) = edge(bleed);
    run_pill(fill, inset, corner, (run.above, run.join.is_some())).when_some(
        run.join,
        |pill, colour| {
            pill.child(
                div()
                    .absolute()
                    .left(px(PANE_INSET - inset + SELECTION_JOIN_LEAD))
                    .right(px(PANE_INSET - inset + SELECTION_JOIN_TRAIL))
                    .bottom_0()
                    .h(px(1.))
                    .bg(colour),
            )
        },
    )
}

fn twist(drawn: &Drawn) -> Stateful<Div> {
    let (line, at, open) = (&drawn.line, drawn.at, drawn.open);
    let arrow = if open {
        "chevron.down"
    } else {
        "chevron.right"
    };
    let into = line.row.opens;
    let key = line.key.clone();
    let this = drawn.this.clone();
    div()
        .id(("ng.finder.twist", at))
        .absolute()
        .top_0()
        .left(px(indent(line) - TWIST_W))
        .w(px(TWIST_W))
        .h_full()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .children(chevron(arrow, 9., FontWeight::BOLD, drawn.lit()))
        .on_click(move |event: &ClickEvent, _, cx| {
            this.update(cx, |this: &mut FinderDemo, cx| {
                this.disclose_at(at, &key, into, !open, event.modifiers().alt);
                cx.notify();
            })
            .ok();
        })
}

fn name_cell(drawn: &Drawn, rename: Option<AnyElement>) -> Div {
    let (line, look) = (&drawn.line, &drawn.look);
    let label = div().ml(px(label_gap(look.icon)));
    div()
        .w(px(look.name_w))
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .whitespace_nowrap()
        .child(div().w(px(indent(line))).h_full().flex_none())
        .child(art_at(
            marked(&line.row, look.icon, drawn.art.element(look.icon)),
            look.icon,
            drawn.anchor.clone(),
        ))
        .child(match rename {
            Some(field) => label.flex_1().child(field).into_any_element(),
            None => {
                let label = label
                    .overflow_hidden()
                    .text_ellipsis_middle()
                    .when_some(drawn.dim(), |label, dim| label.text_color(dim))
                    .child(line.row.display.clone());
                if drawn.clipped {
                    native_tooltip("ng.finder.name.tip", line.row.display.clone(), label)
                        .into_any_element()
                } else {
                    label.into_any_element()
                }
            }
        })
        .children(tag_dots(&drawn.tags).map(|dots| div().ml(px(TAG_DOTS_GAP)).child(dots)))
        .when(line.row.dataless, |name| {
            name.child(div().flex_1()).children(
                small_glyph(
                    "icloud",
                    CLOUD_ART,
                    FontWeight::NORMAL,
                    SystemColor::SecondaryLabel,
                )
                .map(|cloud| div().pr(px(CELL_TRAIL)).child(cloud)),
            )
        })
}

pub(crate) fn detail_text(row: &Row, column: Column) -> SharedString {
    let Some(extra) = row.extra.as_deref() else {
        return SharedString::default();
    };
    match column {
        Column::Tags => {
            let names: Vec<&str> = extra.tags.iter().map(|(name, _)| &**name).collect();
            names.join(", ").into()
        }
        Column::Version => extra.version.clone().unwrap_or_default(),
        Column::Comments => extra.comment.clone(),
        Column::SharedBy => extra.sharer_cell(),
        _ => SharedString::default(),
    }
}

fn detail_cell(spec: &ColumnSpec, line: &Line, look: &Look, detail: Hsla) -> Div {
    let cell = div().w(px(spec.width)).flex_none().text_color(detail);
    match spec.column {
        Column::Name => cell,
        Column::Size => cell
            .pr(px(CELL_TRAIL))
            .flex()
            .justify_end()
            .font(look.dates.clone())
            .child(line.row.size_shown(look.sizes)),
        column => {
            let cell = cell.pl(px(CELL_PAD)).whitespace_nowrap().overflow_hidden();
            match column {
                column if column.dated() => {
                    let style = date_style(spec.width, look.text, look.relative);
                    cell.when(style >= 2, |cell| cell.font(look.dates.clone()))
                        .child(
                            line.row
                                .date(column)
                                .map_or(SharedString::new_static("--"), |at| {
                                    list_date(at, style, look.relative)
                                }),
                        )
                }
                Column::Kind => cell.text_ellipsis_middle().child(kind_text(
                    &line.row.kind,
                    spec.width - 20.,
                    look.text,
                )),
                column => cell.text_ellipsis().child(detail_text(&line.row, column)),
            }
        }
    }
}

fn name_text_w(line: &Line, look: &Look, window: &Window) -> f32 {
    thread_local! {
        static WIDTHS: RefCell<HashMap<(SharedString, u32), f32>> = RefCell::new(HashMap::new());
    }
    let key = (line.row.display.clone(), look.text.to_bits());
    let known = WIDTHS.with_borrow(|widths| widths.get(&key).copied());
    known.unwrap_or_else(|| {
        let width = text_width(window, key.0.clone(), px(look.text));
        WIDTHS.with_borrow_mut(|widths| {
            if widths.len() >= WIDTHS_HELD {
                widths.clear();
            }
            widths.insert(key, width);
        });
        width
    })
}

fn drop_band(line: &Line, look: &Look, window: &Window) -> (f32, f32) {
    let name = name_prefix(line, look.icon) + name_text_w(line, look, window) + LABEL_GAP;
    (indent(line), name.min(look.name_w))
}

fn painted(drawn: &Drawn, rename: Option<AnyElement>) -> Div {
    let look = &drawn.look;
    let over = drawn.selected && look.focused;
    let detail = drawn.dim().unwrap_or(if drawn.lit() {
        look.selection_detail
    } else {
        look.secondary
    });
    let fill = match (drawn.selected, drawn.at % 2 == 1) {
        (true, _) => Some((look.selection, drawn.run)),
        (false, true) => Some((look.stripe, Run::default())),
        (false, false) => None,
    };
    div()
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
        .child(name_cell(drawn, rename))
        .children(
            look.columns
                .iter()
                .map(|spec| detail_cell(spec, &drawn.line, look, detail)),
        )
        .child(div().w(px(PANE_INSET)).flex_none())
}

fn row(
    line: &Line,
    at: usize,
    run: Run,
    look: &Rc<Look>,
    marks: &Marks,
    window: &Window,
    cx: &mut App,
) -> AnyElement {
    let selected = marks.selected.contains(&line.key);
    let dropping = (marks.dropping.as_ref() == Some(&line.key)).then(|| {
        if marks.springing.as_ref() == Some(&line.key) {
            flashed(look.selection)
        } else {
            look.selection
        }
    });
    let ring = marks
        .menu_row
        .as_ref()
        .is_some_and(|row| *row == line.key || (selected && marks.selected.contains(row)));
    let cursor = marks.cursor.as_ref() == Some(&line.key);
    let tags = row_tags(&line.key);
    let dots = if tags.is_empty() {
        0.
    } else {
        tag_dots_w(&tags) + TAG_DOTS_GAP
    };
    let room = look.name_w - name_prefix(line, look.icon) - dots;
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
        ring,
        dropping,
        tags,
        art: list_art(&line.row, look.icon, look.scale, look.previews),
        anchor: cursor.then(|| marks.ql_icon.clone()),
        open: marks.expanded.contains(&line.key),
        clipped: name_text_w(line, look, window) > room,
        this: marks.this.clone(),
    };
    let renaming = marks.renaming.as_ref() == Some(&line.key);
    // A popover's anchor and a rename field are AppKit views, placed only by a row that paints.
    if renaming || (cursor && marks.anchor_held) {
        let rename = renaming.then(|| {
            rename_field(
                line,
                at,
                marks.typed_ahead.as_deref(),
                marks.rename_draft.as_ref(),
                marks.this.clone(),
            )
            .into_any_element()
        });
        return shell(&drawn, painted(&drawn, rename), window);
    }
    marks.faces.borrow_mut().view(drawn, cx)
}

fn shell(drawn: &Drawn, pixels: Div, window: &Window) -> AnyElement {
    let (line, look, this) = (&drawn.line, &drawn.look, &drawn.this);
    let row = div()
        .id(("ng.finder.row", drawn.at))
        .w_full()
        .h(px(look.row_h))
        .relative()
        .font_family(tokens::system_font_family())
        .text_size(px(look.text))
        .text_color(if drawn.selected || drawn.dropping.is_some() {
            look.selection_text
        } else {
            look.label
        })
        .child(pixels)
        .children(
            (line.row.discloses && line.row.readable && !look.searching).then(|| twist(drawn)),
        )
        .on_mouse_down(MouseButton::Left, pick_on_press(line, this.clone()))
        .on_click(pick_on_click(line, this.clone()))
        .on_drag(
            RowDrag {
                key: line.key.clone(),
            },
            {
                let (line, look) = (line.clone(), look.clone());
                move |_, _, _, cx| {
                    cx.new(|_| RowGhost {
                        line: line.clone(),
                        art: |row, size, scale, previews| {
                            list_art(row, size, scale, previews).element(size)
                        },
                        left: indent(&line),
                        icon: look.icon,
                        gap: label_gap(look.icon),
                        width: look.name_w - name_prefix(&line, look.icon),
                        text: look.text,
                        row_h: look.row_h,
                        scale: look.scale,
                        previews: look.previews,
                        fill: look.selection,
                        ink: look.selection_text,
                    })
                }
            },
        );
    match line.row.opens {
        Some(into) => {
            let band = Some(drop_band(line, look, window));
            folder_drop(row, &line.key, into, band, this.clone()).into_any_element()
        }
        None => row.into_any_element(),
    }
}

fn group_title(title: SharedString, look: &Look) -> Div {
    div()
        .h(px(look.row_h))
        .w(px(look.name_w))
        .flex()
        .items_end()
        .pb(px(2.))
        .pl(px(BAND_INSET))
        .child(
            div()
                .text_color(look.secondary)
                .text_size(tokens::small_system_font_size())
                .font_weight(FontWeight::BOLD)
                .child(title),
        )
}
