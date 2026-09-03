use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, Context, DispatchPhase, Entity, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Render, RenderImage, SharedString, Window,
    WindowBounds, WindowHandle, WindowKind, WindowOptions, canvas, div, img, point, prelude::*, px,
    size,
};
use image::{Frame, RgbaImage};
use native_gpui::tokens::{
    self, Symbol, SymbolColor, SystemColor, sf_symbol_colored, system_color,
};
use native_gpui::{
    Cursor, chrome_host, cursor_region, native_root, native_screen_rect, window_options,
};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSEvent, NSWindow, NSWindowOrderingMode, NSWindowStyleMask};
use objc2_foundation::NSRect;

use crate::data::{Face, Line, Row, byte_string, day_string};
use crate::header::NAME_MIN_W;
use crate::palette::Palette;
use crate::toolbar::BAR_H;
use crate::ui::{TAGS, artwork, tag_dot, tag_pill};
use crate::{FinderDemo, LIST_VIEW, ns_window};

pub(crate) const PREVIEW_W: f32 = 366.;

const MIN_W: f32 = 240.;
const MAX_W: f32 = 480.;
pub(crate) const LIST_MIN_W: f32 = 316.;

const GRIP_W: f32 = 6.;

pub(crate) const INSET: f32 = 10.;
/// The parent's resize edge, left uncovered by the card window.
const RESIZE_EDGE: f32 = 6.;

const NAME_SIZE: f32 = 15.;

const NAME_H: f32 = 18.;
const LINE_H: f32 = 15.;
const SMALL_H: f32 = 13.;
const GAP_NAME_KIND: f32 = 4.;
const GAP_KIND_HEAD: f32 = 9.;
const GAP_HEAD_ROWS: f32 = 8.5;
const ROW_GAP: f32 = 8.;
const GAP_ROWS_TAGS: f32 = 17.;
const GAP_TAGS_ADD: f32 = 6.5;
const ICON_H: f32 = 16.;
const GAP_ICON_LABEL: f32 = 10.5;
const BOTTOM_INSET: f32 = 16.;
const GAP_TOP: f32 = 45.;
const GAP_SQUARE_TEXT: f32 = 62.;

const ACTION_SLOT_W: f32 = 75.5;
const ACTION_ICON_SIZE: f32 = 15.;

/// The pane is a borderless child window laid over the list. A material in its own window samples
/// the rows behind it, and GPUI still paints the card over that material; in one window a blur of
/// GPUI content and GPUI content over the blur cannot both come out of the one Metal layer.
pub(crate) struct PreviewWindow {
    handle: WindowHandle<PreviewPane>,
    window: Retained<NSWindow>,
    frame: NSRect,
}

pub(crate) struct PreviewPane {
    finder: Entity<FinderDemo>,
    /// The pane's right edge in screen points while the grip is held.
    drag: Option<f32>,
}

impl PreviewPane {
    fn new(finder: Entity<FinderDemo>, cx: &mut Context<Self>) -> Self {
        cx.observe(&finder, |_, _, cx| cx.notify()).detach();
        Self { finder, drag: None }
    }

    fn resize_start(&mut self, _: &MouseDownEvent, window: &mut Window, _: &mut Context<Self>) {
        self.drag = ns_window(window).map(|window| {
            let frame = window.frame();
            (frame.origin.x + frame.size.width) as f32
        });
    }

    fn resize(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(right) = self.drag else {
            return;
        };
        // A mouse-up outside the window never arrives, so the next move back inside ends the drag.
        if !event.dragging() {
            self.drag = None;
            return;
        }
        // The pointer's screen position, not the event's: GPUI re-sends the last window-local
        // position while a drag holds, and this window moves under the pointer as it resizes.
        let pointer = NSEvent::mouseLocation().x as f32;
        let room = self.finder.read(cx).list_room;
        let width = (right - pointer).clamp(MIN_W, MAX_W.min(room - LIST_MIN_W).max(MIN_W));
        self.finder.update(cx, |this, cx| {
            if width != this.preview_w {
                this.preview_w = width;
                cx.notify();
            }
        });
    }
}

impl Render for PreviewPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let family = tokens::text_font_family(cx).unwrap_or_else(tokens::system_font_family);
        let finder = self.finder.clone();
        let demo = finder.read(cx);
        let palette = Palette::new(demo.list_focused);
        let content = demo.preview_w - 1. - 2. * INSET;
        let card = demo.info_card(demo.cursor_line().as_ref(), &palette, content, &finder);
        let pane = cx.entity();
        native_root(
            div()
                .size_full()
                .font_family(family)
                .text_size(tokens::system_font_size())
                // AppKit sends the drag to the window the grip was pressed in, and the pointer
                // leaves that window as soon as the pane grows, so no hitbox is ever under it.
                .child(canvas(|_, _, _| (), move |_, _, window, _| {
                    // A non-activating panel: a click here must still bring the app forward.
                    window.on_mouse_event(|_: &MouseDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            cx.activate(true);
                        }
                    });
                    let moved = pane.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble {
                            moved.update(cx, |pane, cx| pane.resize(event, window, cx));
                        }
                    });
                    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            pane.update(cx, |pane, _| pane.drag = None);
                        }
                    });
                }))
                .child(
                    // The parent paints the pane's material and edges; this window, never key,
                    // carries only what must draw above that material.
                    div()
                        .size_full()
                        .relative()
                        .child(card)
                        .child(cursor_region(
                            "finder.preview.grip",
                            Cursor::ColumnResize,
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(px(GRIP_W))
                                .on_mouse_down(MouseButton::Left, cx.listener(Self::resize_start)),
                        )),
                ),
        )
    }
}

fn same_rect(a: NSRect, b: NSRect) -> bool {
    a.origin.x == b.origin.x
        && a.origin.y == b.origin.y
        && a.size.width == b.size.width
        && a.size.height == b.size.height
}

fn open_preview_window(
    finder: Entity<FinderDemo>,
    parent: Option<Retained<NSWindow>>,
    frame: NSRect,
    cx: &mut App,
) {
    let pane_finder = finder.clone();
    let opened = parent.and_then(|parent| {
        let handle = cx
            .open_window(
                WindowOptions {
                    kind: WindowKind::PopUp,
                    titlebar: None,
                    focus: false,
                    is_movable: false,
                    is_resizable: false,
                    is_minimizable: false,
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(frame.size.width as f32), px(frame.size.height as f32)),
                    })),
                    ..window_options()
                },
                |_, cx| cx.new(|cx| PreviewPane::new(pane_finder, cx)),
            )
            .ok()?;
        let window = handle
            .update(cx, |_, window, _| ns_window(window))
            .ok()
            .flatten()?;
        window.setStyleMask(NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel);
        window.setHasShadow(false);
        // Changing the style mask hands back an opaque window.
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        // An NSPanel: a click in the pane must not take key from the list.
        unsafe {
            let _: () = msg_send![&*window, setBecomesKeyOnlyIfNeeded: true];
        }
        window.setFrame_display(frame, true);
        unsafe { parent.addChildWindow_ordered(&window, NSWindowOrderingMode::Above) };
        window.setLevel(parent.level());
        Some(PreviewWindow {
            handle,
            window,
            frame,
        })
    });
    finder.update(cx, |this, _| {
        this.preview_opening = false;
        this.preview_window = opened;
    });
}

impl FinderDemo {
    /// Keep the pane window over the list column, or close it when the pane is hidden.
    pub(crate) fn sync_preview_window(&mut self, window: &Window, cx: &mut Context<Self>) {
        let metrics = self.metrics();
        let viewport = window.viewport_size();
        self.list_focused = self.focus.is_focused(window);
        self.list_room =
            f32::from(viewport.width) - f32::from(metrics.left) - f32::from(metrics.right);
        self.fit_name_column();

        if !self.preview_shown() {
            if let Some(pane) = self.preview_window.take() {
                cx.defer(move |cx| {
                    pane.handle
                        .update(cx, |_, window, _| window.remove_window())
                        .ok();
                });
            }
            return;
        }

        let bars = [self.path_bar_shown, self.status_bar_shown]
            .iter()
            .filter(|shown| **shown)
            .count() as f32
            * BAR_H;
        // The card window stops short of the window's right and bottom edges: those are the
        // parent's resize edges, and a child window over them would take the drag.
        let bounds = Bounds {
            origin: point(
                px(f32::from(viewport.width) - f32::from(metrics.right) - self.preview_w),
                metrics.top,
            ),
            size: size(
                px(self.preview_w - RESIZE_EDGE),
                viewport.height - metrics.top - metrics.bottom - px(bars + RESIZE_EDGE),
            ),
        };
        let frame = native_screen_rect(&chrome_host(window), bounds);
        match &mut self.preview_window {
            Some(pane) if !same_rect(pane.frame, frame) => {
                pane.frame = frame;
                // Moved off this draw: resizing the pane's window from inside the parent's frame
                // leaves the pane's own gpui window without its size change.
                let pane_window = pane.window.clone();
                cx.spawn(async move |_, _| pane_window.setFrame_display(frame, true))
                    .detach();
            }
            Some(_) => {}
            None if self.preview_opening => {}
            None => {
                self.preview_opening = true;
                let finder = cx.entity();
                let parent = ns_window(window);
                cx.defer(move |cx| open_preview_window(finder, parent, frame, cx));
            }
        }
    }

    /// Finder's Name column takes up whatever the window gives or takes: it fills the list at
    /// first, shrinks to its minimum as the list narrows, and only grows again once the columns
    /// fit beside the pane once more.
    fn fit_name_column(&mut self) {
        let span = self.list_room
            - if self.preview_shown() {
                self.preview_w
            } else {
                0.
            };
        let fixed = self.list_content_w() - self.name_w;
        if self.list_span.is_nan() {
            self.name_w = (span - fixed).max(NAME_MIN_W);
        } else if span != self.list_span {
            let delta = span - self.list_span;
            let overflow = (self.list_content_w() - self.list_span).max(0.);
            let change = if delta < 0. {
                delta
            } else {
                (delta - overflow).max(0.)
            };
            self.name_w = (self.name_w + change).max(NAME_MIN_W);
        }
        self.list_span = span;
    }

    pub(crate) fn info_card(
        &self,
        line: Option<&Line>,
        palette: &Palette,
        content: f32,
        finder: &Entity<FinderDemo>,
    ) -> AnyElement {
        let card = if self.selected.len() > 1 {
            self.many_card(content)
        } else {
            let Some(line) = line else {
                return div().into_any_element();
            };
            let row = &line.row;
            Card {
                hero: hero(row, content),
                title: row.name.clone(),
                subtitle: kind_line(row),
                rows: info_rows(row, self.preview_expanded),
                show_more: row.dimensions.is_some() || row.duration.is_some(),
                tags: self.tags.get(&line.key).copied().into_iter().collect(),
                many: false,
                actions: actions(row.face),
            }
        };
        self.card_element(card, palette, content, finder)
    }

    /// Finder's card for a selection of several: their icons in a stack, a count, the kinds and
    /// the size together, each date as the span from the earliest to the latest, and every tag.
    fn many_card(&self, content: f32) -> Card {
        let lines: Vec<Line> = self
            .lines(self.view == LIST_VIEW)
            .into_iter()
            .filter(|line| self.selected.contains(&line.key))
            .collect();
        let folders = lines
            .iter()
            .filter(|line| line.row.face == Face::Folder)
            .count();
        let files = lines.len() - folders;
        let bytes: u64 = lines.iter().map(|line| self.total_bytes(&line.row)).sum();
        let kinds = match (files, folders) {
            (0, n) => format!("{n} folders"),
            (n, 0) => format!("{n} documents"),
            (f, d) => format!("{f} documents, {d} folders"),
        };
        let span = |at: fn(&Row) -> f64| {
            let lo = lines
                .iter()
                .map(|line| at(&line.row))
                .fold(f64::MAX, f64::min);
            let hi = lines
                .iter()
                .map(|line| at(&line.row))
                .fold(f64::MIN, f64::max);
            let (lo, hi) = (day_string(lo), day_string(hi));
            SharedString::from(if lo == hi {
                lo
            } else {
                format!("{lo} – {hi}")
            })
        };
        let mut tags: Vec<usize> = lines
            .iter()
            .filter_map(|line| self.tags.get(&line.key).copied())
            .collect();
        tags.sort_unstable();
        tags.dedup();
        Card {
            hero: self.stack_hero(&lines, content),
            title: format!("{} items", lines.len()).into(),
            subtitle: format!("{kinds} - {}", byte_string(bytes)).into(),
            rows: vec![
                ("Created", span(|row| row.created_at)),
                ("Modified", span(|row| row.modified_at)),
            ],
            show_more: false,
            tags,
            many: true,
            actions: &[("ellipsis.circle", "More…")],
        }
    }

    /// A folder weighs what it holds.
    fn total_bytes(&self, row: &Row) -> u64 {
        match row.opens.and_then(|id| self.store.get(id)) {
            Some(folder) => folder.rows.iter().map(|row| self.total_bytes(row)).sum(),
            None => row.bytes,
        }
    }

    /// The first three selected items' icons, fanned as Finder stacks them, drawn once per
    /// selection and kept.
    fn stack_hero(&self, lines: &[Line], content: f32) -> AnyElement {
        let icons: Vec<Symbol> = lines
            .iter()
            .take(3)
            .filter_map(|line| line.row.artwork)
            .filter_map(|kind| tokens::file_icon(kind, px(content * STACK_ICON)))
            .collect();
        let keys: Vec<SharedString> = lines.iter().take(3).map(|line| line.key.clone()).collect();
        let side = icons
            .first()
            .map(|icon| (content * icon.scale).round() as u32)
            .unwrap_or_default();
        let cached = self
            .stack
            .borrow()
            .as_ref()
            .filter(|(k, s, _)| *k == keys && *s == side)
            .map(|(_, _, image)| image.clone());
        let image = cached.or_else(|| {
            let image = stack_image(&icons, side)?;
            *self.stack.borrow_mut() = Some((keys, side, image.clone()));
            Some(image)
        });
        let art = match image {
            Some(image) => img(image).w(px(content)).h(px(content)).into_any_element(),
            None => artwork(&lines[0].row, content),
        };
        div()
            .w(px(content))
            .h(px(content))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(art)
            .into_any_element()
    }

    fn card_element(
        &self,
        card: Card,
        palette: &Palette,
        content: f32,
        finder: &Entity<FinderDemo>,
    ) -> AnyElement {
        let body = tokens::system_font_size();
        let small = tokens::small_system_font_size();
        let hairline = system_color(SystemColor::QuaternarySystemFill);

        let top = div()
            .size_full()
            .flex()
            .flex_col()
            .px(px(INSET))
            .pb(px(BOTTOM_INSET))
            .child(div().h(px(GAP_TOP)).flex_none())
            .child(
                div()
                    .w(px(content))
                    .h(px(content))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .child(card.hero),
            );

        let mut lower = div()
            .flex_none()
            .flex()
            .flex_col()
            .mt(px(GAP_SQUARE_TEXT))
            .child(
                div()
                    .line_height(px(NAME_H))
                    .text_size(px(NAME_SIZE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.label)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(card.title),
            )
            .child(
                div()
                    .mt(px(GAP_NAME_KIND))
                    .line_height(px(LINE_H))
                    .text_size(body)
                    .text_color(palette.secondary)
                    .child(card.subtitle),
            )
            .child(
                div()
                    .mt(px(GAP_KIND_HEAD))
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .line_height(px(LINE_H))
                    .text_size(body)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.label)
                    .child("Information")
                    .when(card.show_more, |heading| {
                        heading.child(
                            div()
                                .line_height(px(SMALL_H))
                                .text_size(small)
                                .font_weight(FontWeight::NORMAL)
                                .text_color(system_color(SystemColor::Link))
                                .on_mouse_down(MouseButton::Left, {
                                    let finder = finder.clone();
                                    move |_: &MouseDownEvent, _, cx| {
                                        finder.update(cx, |this, cx| {
                                            this.preview_expanded = !this.preview_expanded;
                                            cx.notify();
                                        })
                                    }
                                })
                                .child(if self.preview_expanded {
                                    "Show Less"
                                } else {
                                    "Show More"
                                }),
                        )
                    }),
            );

        let mut block = div().mt(px(GAP_HEAD_ROWS)).flex().flex_col();
        for (index, (label, value)) in card.rows.iter().enumerate() {
            if index > 0 {
                block = block.child(
                    div()
                        .h(px(ROW_GAP))
                        .flex()
                        .flex_col()
                        .justify_center()
                        .child(div().h(px(1.)).bg(hairline)),
                );
            }
            block = block.child(
                div()
                    .flex()
                    .justify_between()
                    .line_height(px(LINE_H))
                    .text_size(body)
                    .child(div().text_color(palette.secondary).child(*label))
                    .child(div().text_color(palette.label).child(value.clone())),
            );
        }
        lower = lower.child(block).child(
            div()
                .mt(px(GAP_ROWS_TAGS))
                .line_height(px(LINE_H))
                .text_size(body)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.label)
                .child("Tags"),
        );

        let tags = div()
            .mt(px(GAP_TAGS_ADD))
            .flex()
            .items_center()
            .gap(px(5.))
            .line_height(px(LINE_H))
            .text_size(body);
        lower = lower.child(match (card.many, card.tags.first()) {
            (false, Some(tag)) => tags
                .child(tag_dot(*tag))
                .child(div().text_color(palette.label).child(TAGS[*tag].0)),
            (false, None) => tags.text_color(palette.tertiary).child("Add Tags…"),
            (true, _) => tags
                .children(card.tags.iter().map(|tag| tag_pill(*tag, palette.label)))
                .child(
                    div()
                        .ml(px(3.))
                        .text_color(system_color(SystemColor::Link))
                        .child("Add Tag"),
                ),
        });

        let actions = card.actions;
        let slot = ACTION_SLOT_W.min(content / actions.len() as f32);
        top.child(lower)
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .flex()
                    .justify_center()
                    .children(actions.iter().map(|(symbol, label)| {
                        div()
                            .w(px(slot))
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(
                                div().h(px(ICON_H)).flex().items_center().children(
                                    sf_symbol_colored(
                                        symbol,
                                        px(ACTION_ICON_SIZE),
                                        FontWeight::NORMAL,
                                        &SymbolColor::Hierarchical(SystemColor::SecondaryLabel),
                                    )
                                    .map(|glyph| glyph.img()),
                                ),
                            )
                            .child(
                                div()
                                    .mt(px(GAP_ICON_LABEL))
                                    .line_height(px(SMALL_H))
                                    .text_size(small)
                                    .text_color(palette.secondary)
                                    .child(*label),
                            )
                    })),
            )
            .into_any_element()
    }
}

/// What the pane's card shows, whichever way it was gathered.
struct Card {
    hero: AnyElement,
    title: SharedString,
    subtitle: SharedString,
    rows: Vec<(&'static str, SharedString)>,
    show_more: bool,
    tags: Vec<usize>,
    /// Several items: tags as pills and `Add Tag`, not one tag's dot and `Add Tags…`.
    many: bool,
    actions: &'static [(&'static str, &'static str)],
}

/// Each icon's size as a share of the hero square.
const STACK_ICON: f32 = 0.62;

/// Back to front: degrees of tilt, offset in shares of the square, scale. Finder fans a
/// selection's icons this way.
const STACK_LAYERS: [(f32, f32, f32, f32); 3] = [
    (-10., 0.03, -0.025, 0.9),
    (7., -0.025, 0., 0.95),
    (-3., 0., 0.02, 1.),
];

/// The icons composed into one square image, `side` device pixels across, the first icon in
/// front. Straight-alpha BGRA in and out, which is what the workspace icons come as.
fn stack_image(icons: &[Symbol], side: u32) -> Option<Arc<RenderImage>> {
    if icons.is_empty() {
        return None;
    }
    let layers = &STACK_LAYERS[3 - icons.len().min(3)..];
    let mut canvas = vec![0f32; (side * side * 4) as usize];
    let centre = side as f32 / 2.;
    for (icon, (degrees, dx, dy, zoom)) in icons.iter().take(3).rev().zip(layers) {
        let (w, h) = {
            let size = icon.image.size(0);
            (size.width.0 as usize, size.height.0 as usize)
        };
        let src = icon.image.as_bytes(0)?;
        let (sin, cos) = degrees.to_radians().sin_cos();
        let (cx, cy) = (centre + dx * side as f32, centre + dy * side as f32);
        for y in 0..side {
            for x in 0..side {
                let (px_, py) = (x as f32 - cx, y as f32 - cy);
                let u = (px_ * cos + py * sin) / zoom + w as f32 / 2.;
                let v = (-px_ * sin + py * cos) / zoom + h as f32 / 2.;
                if u < 0. || v < 0. || u >= (w - 1) as f32 || v >= (h - 1) as f32 {
                    continue;
                }
                let (x0, y0) = (u as usize, v as usize);
                let (fx, fy) = (u - x0 as f32, v - y0 as f32);
                let at = |x: usize, y: usize, c: usize| src[(y * w + x) * 4 + c] as f32;
                let mut s = [0f32; 4];
                for (c, slot) in s.iter_mut().enumerate() {
                    *slot = at(x0, y0, c) * (1. - fx) * (1. - fy)
                        + at(x0 + 1, y0, c) * fx * (1. - fy)
                        + at(x0, y0 + 1, c) * (1. - fx) * fy
                        + at(x0 + 1, y0 + 1, c) * fx * fy;
                }
                let a = s[3] / 255.;
                if a <= 0. {
                    continue;
                }
                let d = &mut canvas[((y * side + x) * 4) as usize..][..4];
                let da = d[3] / 255.;
                let out = a + da * (1. - a);
                for c in 0..3 {
                    d[c] = (s[c] * a + d[c] * da * (1. - a)) / out;
                }
                d[3] = out * 255.;
            }
        }
    }
    let bytes: Vec<u8> = canvas
        .iter()
        .map(|v| v.round().clamp(0., 255.) as u8)
        .collect();
    let raster = RgbaImage::from_raw(side, side, bytes)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(raster)])))
}

fn hero(row: &Row, content: f32) -> AnyElement {
    if matches!(row.face, Face::Movie | Face::Image) {
        return div()
            .w(px(content))
            .h(px(content * 9. / 16.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .bg(system_color(SystemColor::TertiarySystemFill))
            .child(artwork(row, (content / 3.).min(96.)))
            .into_any_element();
    }
    artwork(row, content)
}

fn kind_line(row: &Row) -> SharedString {
    match row.size.as_ref() {
        "--" => row.face.kind().into(),
        size => format!("{} - {size}", row.face.kind()).into(),
    }
}

fn info_rows(row: &Row, expanded: bool) -> Vec<(&'static str, SharedString)> {
    let media = row.dimensions.is_some() || row.duration.is_some();
    let mut rows = Vec::with_capacity(6);
    if media {
        rows.push(("Created", row.created.clone()));
    }
    rows.push(("Modified", row.modified.clone()));
    if media {
        rows.extend(row.dimensions.clone().map(|value| ("Dimensions", value)));
        rows.extend(row.duration.clone().map(|value| ("Duration", value)));
        if expanded {
            rows.push(("Kind", row.face.kind().into()));
            rows.push(("Size", row.size.clone()));
        }
    } else {
        rows.push(("Size", row.size.clone()));
    }
    rows
}

fn actions(face: Face) -> &'static [(&'static str, &'static str)] {
    match face {
        Face::Movie => &[
            ("rotate.left", "Rotate Left"),
            ("timeline.selection", "Trim"),
            ("ellipsis.circle", "More…"),
        ],
        Face::Image => &[("rotate.left", "Rotate Left"), ("ellipsis.circle", "More…")],
        _ => &[("ellipsis.circle", "More…")],
    }
}
