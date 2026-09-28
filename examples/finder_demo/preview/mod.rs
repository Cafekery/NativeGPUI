mod card;
mod stats;

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use block2::{RcBlock, StackBlock};
use gpui::{
    App, Bounds, Context, DispatchPhase, Entity, EntityId, MouseDownEvent, Render, RenderImage,
    WeakEntity, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, canvas, div, point,
    prelude::*, px, size,
};
use native_gpui::tokens;
use native_gpui::{chrome_host, native_root, native_screen_rect, tab_bar_height, window_options};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::{ClassType, msg_send};
use objc2_app_kit::{
    NSApplication, NSColor, NSPanel, NSWindow, NSWindowDidResizeNotification, NSWindowOrderingMode,
    NSWindowStyleMask, NSWindowWillCloseNotification,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSPoint, NSRect, NSRunLoop, NSSize,
};

pub(crate) use self::card::follow_stacks;
pub(crate) use self::stats::{Stats, sized_by_index, summarize};
use crate::app::FinderDemo;
use crate::frame::ns_window;
use crate::palette::Palette;

pub(crate) const PREVIEW_W: f32 = 250.;

const MIN_W: f32 = 240.;
const MAX_W: f32 = 480.;
pub(crate) const LIST_MIN_W: f32 = 316.;

pub(crate) const INSET: f32 = 10.;
const RESIZE_EDGE: f32 = 6.;
/// Past the time a first picture usually takes; a slower one shows the type icon meanwhile.
const HOLD: Duration = Duration::from_millis(120);
/// The split view takes a click within 2pt of its divider for itself.
const DIVIDER_GRAB: f32 = 3.;

pub(crate) struct PreviewWindow {
    handle: WindowHandle<PreviewPane>,
    window: Retained<NSWindow>,
    anchor: Rc<Cell<Anchor>>,
    follow: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    pane: EntityId,
    handed: Rc<Handed>,
}

/// What a window hands its pane to draw.
#[derive(Default)]
struct Handed {
    /// What the card was drawn from; it is taken and drawn again only when this changes.
    key: RefCell<Option<card::CardKey>>,
    shown: RefCell<Option<card::Shown>>,
    /// The icon landings seen when the pane last drew a stand-in for an icon.
    waiting: Cell<Option<u64>>,
    /// Since when the card has been kept up while the next one's picture is made.
    held: Cell<Option<Instant>>,
    stack: RefCell<Option<Arc<RenderImage>>>,
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

/// AppKit offers key to the frontmost window when one closes; this pane's panel declines it and
/// leaves the app with no key window, so the frontmost ordinary window takes it instead.
pub(crate) fn keep_a_key_window() {
    let block = StackBlock::new(|_: NonNull<NSNotification>| {
        let later = RcBlock::new(|| {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let app = NSApplication::sharedApplication(mtm);
            if app.keyWindow().is_some() || !app.isActive() {
                return;
            }
            let windows = app.orderedWindows();
            if let Some(window) = windows.iter().find(|window| {
                window.isVisible()
                    && !window.isKindOfClass(NSPanel::class())
                    && window.canBecomeKeyWindow()
            }) {
                window.makeKeyWindow();
            }
        });
        unsafe { NSRunLoop::mainRunLoop().performBlock(&later) };
    })
    .copy();
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSWindowWillCloseNotification),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(observer);
}

/// Holds its Finder weakly: a strong handle here keeps a closed window's whole state alive.
pub(crate) struct PreviewPane {
    finder: WeakEntity<FinderDemo>,
    handed: Rc<Handed>,
}

impl Render for PreviewPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(finder) = self.finder.upgrade() else {
            return div().into_any_element();
        };
        let family = tokens::text_font_family(cx).unwrap_or_else(tokens::system_font_family);
        let shown = self.handed.shown.borrow();
        let asked = crate::icons::asks();
        let (card, pane_w) = match shown.as_ref() {
            Some(shown) => (
                shown.element(
                    &Palette::new(shown.list_focused),
                    &finder,
                    &self.handed.stack,
                ),
                shown.width(),
            ),
            None => (div().into_any_element(), 0.),
        };
        self.handed
            .waiting
            .set((crate::icons::asks() != asked).then(crate::icons::landed));
        crate::icons::start_wanted(&finder, cx);
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
    let handed = Rc::new(Handed::default());
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
                |_, cx| {
                    cx.new(|_| PreviewPane {
                        finder: pane_finder.downgrade(),
                        handed: handed.clone(),
                    })
                },
            )
            .ok()?;
        let (window, pane) = handle
            .update(cx, |_, window, cx| (ns_window(window), cx.entity_id()))
            .ok()?;
        let window = window?;
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
            pane,
            handed,
        })
    });
    finder.update(cx, |this, _| {
        this.preview_opening = false;
        if let Some(pane) = &opened {
            let content = this.preview_w - 2. * INSET;
            *pane.handed.shown.borrow_mut() = this.shown(this.cursor_line().as_ref(), content);
        }
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
        let Some(pane) = &self.preview_window else {
            return;
        };
        let content = self.preview_w - 2. * INSET;
        let card = Some(self.card_key(content, key));
        let handed = &pane.handed;
        let landed = handed
            .waiting
            .get()
            .is_some_and(|at| crate::icons::landed() != at);
        if *handed.key.borrow() != card || landed {
            // A type icon would flash up before the picture; the card that is up waits for it.
            if handed.shown.borrow().is_some() && self.hero_awaited(content) {
                let since = handed.held.get().unwrap_or_else(|| {
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(HOLD).await;
                        this.update(cx, |_, cx| cx.notify()).ok();
                    })
                    .detach();
                    Instant::now()
                });
                handed.held.set(Some(since));
                if since.elapsed() < HOLD {
                    return;
                }
            }
            handed.held.set(None);
            *handed.shown.borrow_mut() = self.shown(self.cursor_line().as_ref(), content);
            *handed.key.borrow_mut() = card;
            handed.waiting.set(None);
            App::notify(cx, pane.pane);
        }
    }
}
