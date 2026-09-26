mod card;
mod stats;

use std::cell::Cell;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::StackBlock;
use gpui::{
    App, Bounds, Context, DispatchPhase, Entity, MouseDownEvent, Render, WeakEntity, Window,
    WindowBounds, WindowHandle, WindowKind, WindowOptions, canvas, div, point, prelude::*, px,
    size,
};
use native_gpui::tokens;
use native_gpui::{chrome_host, native_root, native_screen_rect, tab_bar_height, window_options};
use objc2::msg_send;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{
    NSColor, NSWindow, NSWindowDidResizeNotification, NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSPoint, NSRect, NSSize};

pub(crate) use self::stats::{Stats, summarize};
use crate::app::FinderDemo;
use crate::frame::ns_window;
use crate::palette::Palette;

pub(crate) const PREVIEW_W: f32 = 250.;

const MIN_W: f32 = 240.;
const MAX_W: f32 = 480.;
pub(crate) const LIST_MIN_W: f32 = 316.;

pub(crate) const INSET: f32 = 10.;
const RESIZE_EDGE: f32 = 6.;
/// The split view takes a click within 2pt of its divider for itself.
const DIVIDER_GRAB: f32 = 3.;

pub(crate) struct PreviewWindow {
    handle: WindowHandle<PreviewPane>,
    window: Retained<NSWindow>,
    anchor: Rc<Cell<Anchor>>,
    follow: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    key: Cell<bool>,
}

impl Drop for PreviewWindow {
    fn drop(&mut self) {
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(self.follow.as_ref()) };
    }
}

/// The pane's gaps to its parent's edges, which resizing the parent keeps.
#[derive(Clone, Copy)]
struct Anchor {
    right: f64,
    top: f64,
    bottom: f64,
    width: f64,
}

impl Anchor {
    fn new(parent: NSRect, pane: NSRect) -> Self {
        Self {
            right: parent.origin.x + parent.size.width - pane.origin.x - pane.size.width,
            top: parent.origin.y + parent.size.height - pane.origin.y - pane.size.height,
            bottom: pane.origin.y - parent.origin.y,
            width: pane.size.width,
        }
    }

    fn place(self, parent: NSRect) -> NSRect {
        NSRect::new(
            NSPoint::new(
                parent.origin.x + parent.size.width - self.right - self.width,
                parent.origin.y + self.bottom,
            ),
            NSSize::new(self.width, parent.size.height - self.top - self.bottom),
        )
    }
}

/// Moves the pane within the parent's own resize: placed from the next frame, it trails the edge.
fn follow_parent(
    parent: &NSWindow,
    pane: &Retained<NSWindow>,
    anchor: &Rc<Cell<Anchor>>,
) -> Retained<ProtocolObject<dyn NSObjectProtocol>> {
    let (host, pane, anchor) = (Weak::new(parent), pane.clone(), anchor.clone());
    let block = StackBlock::new(move |_: NonNull<NSNotification>| {
        let Some(host) = host.load() else {
            return;
        };
        let frame = anchor.get().place(host.frame());
        if !near(pane.frame(), frame) {
            pane.setFrame_display(frame, true);
        }
    })
    .copy();
    unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSWindowDidResizeNotification),
            Some(parent),
            None,
            &block,
        )
    }
}

fn near(a: NSRect, b: NSRect) -> bool {
    (a.origin.x - b.origin.x).abs() < 0.5
        && (a.origin.y - b.origin.y).abs() < 0.5
        && (a.size.width - b.size.width).abs() < 0.5
        && (a.size.height - b.size.height).abs() < 0.5
}

/// Holds its Finder weakly: a strong handle here keeps a closed window's whole state alive.
pub(crate) struct PreviewPane {
    finder: WeakEntity<FinderDemo>,
}

impl PreviewPane {
    fn new(finder: Entity<FinderDemo>, cx: &mut Context<Self>) -> Self {
        cx.observe(&finder, |_, _, cx| cx.notify()).detach();
        Self {
            finder: finder.downgrade(),
        }
    }
}

impl Render for PreviewPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(finder) = self.finder.upgrade() else {
            return div().into_any_element();
        };
        let family = tokens::text_font_family(cx).unwrap_or_else(tokens::system_font_family);
        let demo = finder.read(cx);
        let palette = Palette::new(demo.list_focused);
        let pane_w = demo.preview_w;
        let card = demo.info_card(
            demo.cursor_line().as_ref(),
            &palette,
            pane_w - 2. * INSET,
            &finder,
        );
        crate::icons::draw_wanted(&finder, cx);
        crate::meta::fetch_wanted(&finder, cx);
        native_root(
            div()
                .size_full()
                .relative()
                .font_family(family)
                .text_size(tokens::system_font_size())
                .child(canvas(
                    |_, _, _| (),
                    |_, _, window, _| {
                        // A non-activating panel: a click here must still bring the app forward.
                        window.on_mouse_event(|_: &MouseDownEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                cx.activate(true);
                            }
                        });
                    },
                ))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(-DIVIDER_GRAB))
                        .w(px(pane_w))
                        .child(card),
                ),
        )
        .into_any_element()
    }
}

pub(crate) fn close_with_finder(cx: &mut Context<FinderDemo>) {
    cx.on_release(|this, cx| {
        if let Some(pane) = this.preview_window.take() {
            pane.handle
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        }
    })
    .detach();
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
        // gpui takes drops on every window; this one has nowhere to put them.
        window.unregisterDraggedTypes();
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
        let anchor = Rc::new(Cell::new(Anchor::new(parent.frame(), frame)));
        let follow = follow_parent(&parent, &window, &anchor);
        Some(PreviewWindow {
            handle,
            window,
            anchor,
            follow,
            key: Cell::new(parent.isKeyWindow()),
        })
    });
    finder.update(cx, |this, _| {
        this.preview_opening = false;
        this.preview_window = opened;
    });
}

impl FinderDemo {
    pub(crate) fn show_preview(&mut self, shown: bool) {
        self.preview = shown;
        crate::prefs::set_preview(self.view, shown);
    }

    pub(crate) fn sync_preview_window(&mut self, window: &Window, cx: &mut Context<Self>) {
        if let Some(sidebar) = &self.sidebar {
            // Whole points: a window resize can hand the pane back half a point short.
            match sidebar
                .take_inspector_resize()
                .map(|width| f32::from(width).round())
            {
                // Finder hides the preview when its divider is dragged shut.
                Some(width) if width <= 0. => {
                    self.preview = false;
                    crate::prefs::set_preview(self.view, false);
                    cx.notify();
                }
                Some(width) if width != self.preview_w => {
                    self.preview_w = width;
                    crate::prefs::set_pane_width(width);
                    cx.notify();
                }
                _ => {}
            }
            sidebar.set_inspector(
                self.preview_shown().then(|| px(self.preview_w)),
                px(MIN_W),
                px(MAX_W),
            );
            let card = self.selected.len() > 1 || self.cursor_line().is_some();
            sidebar.set_inspector_footer(px(if card { card::ACTIONS_H } else { 0. }));
        }
        let metrics = self.metrics();
        let viewport = window.viewport_size();
        let key = window.is_window_active();
        self.list_focused = self.focus.is_focused(window) && key;
        if self
            .preview_window
            .as_ref()
            .is_some_and(|pane| pane.key.replace(key) != key)
        {
            cx.notify();
        }
        self.list_room =
            f32::from(viewport.width) - f32::from(metrics.left) - f32::from(metrics.right);
        let top = metrics.top - px(tab_bar_height(window));
        self.pane_h = f32::from(viewport.height - top - metrics.bottom) - RESIZE_EDGE;
        self.fit_name_column();

        // The card rides the pane while it slides in or out, and goes once it is shut.
        if metrics.right <= px(DIVIDER_GRAB + RESIZE_EDGE) {
            if let Some(pane) = self.preview_window.take() {
                cx.defer(move |cx| {
                    pane.handle
                        .update(cx, |_, window, _| window.remove_window())
                        .ok();
                });
            }
            return;
        }

        // Clear of the divider's grab and the parent's resize edges, which a child window takes.
        let bounds = Bounds {
            origin: point(viewport.width - metrics.right + px(DIVIDER_GRAB), top),
            size: size(
                metrics.right - px(DIVIDER_GRAB + RESIZE_EDGE),
                px(self.pane_h),
            ),
        };
        let frame = native_screen_rect(&chrome_host(window), bounds);
        match &self.preview_window {
            Some(pane) => {
                if let Some(parent) = &self.host {
                    pane.anchor.set(Anchor::new(parent.frame(), frame));
                }
                if !near(pane.window.frame(), frame) {
                    // Off this draw: resizing from inside the parent's frame loses the size change.
                    let pane_window = pane.window.clone();
                    cx.spawn(async move |_, _| pane_window.setFrame_display(frame, true))
                        .detach();
                }
            }
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
