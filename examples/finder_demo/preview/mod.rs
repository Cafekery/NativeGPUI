//! Finder's preview pane: a glass frosting the rows, and a child window carrying the card.

mod card;
mod stats;

use gpui::{
    App, Bounds, Context, DispatchPhase, Entity, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Render, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, canvas,
    div, point, prelude::*, px, size,
};
use native_gpui::tokens;
use native_gpui::{
    Cursor, chrome_host, cursor_region, native_root, native_screen_rect, tab_bar_height,
    window_options,
};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSEvent, NSWindow, NSWindowOrderingMode, NSWindowStyleMask};
use objc2_foundation::NSRect;

pub(crate) use self::stats::{Stats, summarize};
use crate::app::FinderDemo;
use crate::frame::ns_window;
use crate::palette::Palette;

pub(crate) const PREVIEW_W: f32 = 366.;

const MIN_W: f32 = 240.;
const MAX_W: f32 = 480.;
pub(crate) const LIST_MIN_W: f32 = 316.;

const GRIP_W: f32 = 6.;

pub(crate) const INSET: f32 = 10.;
/// The parent's resize edge, left uncovered by the card window.
const RESIZE_EDGE: f32 = 6.;

/// The card's own window: one Metal layer cannot both blur GPUI content and paint over the blur.
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
        // The pointer's screen position: GPUI re-sends the last window-local one while dragging.
        let pointer = NSEvent::mouseLocation().x as f32;
        let room = self.finder.read(cx).list_room;
        let width = (right - pointer).clamp(MIN_W, MAX_W.min(room - LIST_MIN_W).max(MIN_W));
        self.finder.update(cx, |this, cx| {
            if width != this.preview_w {
                this.preview_w = width;
                crate::prefs::set_pane_width(width);
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
        crate::icons::draw_wanted(&finder, cx);
        crate::meta::fetch_wanted(&finder, cx);
        let pane = cx.entity();
        native_root(
            div()
                .size_full()
                .font_family(family)
                .text_size(tokens::system_font_size())
                // AppKit sends the drag to the window the grip was pressed in, which the pointer
                // leaves as soon as the pane grows.
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
                    // The parent paints the pane's material; this carries what draws above it.
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
    pub(crate) fn sync_preview_window(&mut self, window: &Window, cx: &mut Context<Self>) {
        let metrics = self.metrics();
        let viewport = window.viewport_size();
        self.list_focused = self.focus.is_focused(window) && window.is_window_active();
        self.list_room =
            f32::from(viewport.width) - f32::from(metrics.left) - f32::from(metrics.right);
        let top = metrics.top - px(tab_bar_height(window));
        self.pane_h = f32::from(viewport.height - top - metrics.bottom) - RESIZE_EDGE;
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

        // The card window stops short of the parent's resize edges: a child window over them
        // would take the drag.
        let bounds = Bounds {
            origin: point(
                px(f32::from(viewport.width) - f32::from(metrics.right) - self.preview_w),
                top,
            ),
            size: size(px(self.preview_w - RESIZE_EDGE), px(self.pane_h)),
        };
        let frame = native_screen_rect(&chrome_host(window), bounds);
        match &mut self.preview_window {
            Some(pane) if !same_rect(pane.frame, frame) => {
                pane.frame = frame;
                // Off this draw: resizing from inside the parent's frame loses the size change.
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
}
