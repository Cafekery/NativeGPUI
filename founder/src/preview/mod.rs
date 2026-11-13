mod card;
pub(crate) mod options;
mod stats;

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use block2::{RcBlock, StackBlock};
use gpui::{
    App, Bounds, Context, DispatchPhase, Entity, EntityId, MouseDownEvent, Pixels, Render,
    RenderImage, WeakEntity, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, canvas,
    div, point, prelude::*, px, size,
};
use native_gpui::sidebar::{NativeSidebar, SidebarMetrics};
use native_gpui::tokens;
use native_gpui::{native_root, window_options};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::{ClassType, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationDidUnhideNotification, NSColor, NSPanel, NSWindow,
    NSWindowDidMoveNotification, NSWindowDidResizeNotification, NSWindowOrderingMode,
    NSWindowStyleMask, NSWindowWillCloseNotification,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSPoint, NSRect, NSRunLoop, NSSize,
};

pub(crate) use self::card::{duration, follow_stacks};
pub(crate) use self::stats::{Newest, Stats, sized_by_index, summarize};
use crate::app::FinderDemo;
use crate::frame::ns_window;
use crate::palette::Palette;
use crate::view::{COLUMN_VIEW, GALLERY_VIEW};

pub(crate) const PREVIEW_W: f32 = 288.;
pub(crate) const LIST_MIN_W: f32 = 316.;
pub(crate) const INSET: f32 = 10.;
const MIN_W: f32 = 240.;
const MAX_W: f32 = 480.;
const RESIZE_EDGE: f32 = 6.;
const HOLD: Duration = Duration::from_millis(120);
/// The split view takes a click within 2pt of its divider for itself.
const DIVIDER_GRAB: f32 = 3.;

pub(crate) struct PreviewWindow {
    handle: WindowHandle<PreviewPane>,
    window: Retained<NSWindow>,
    follow: [Retained<ProtocolObject<dyn NSObjectProtocol>>; 3],
    pane: EntityId,
    handed: Rc<Handed>,
}

#[derive(Default)]
struct Handed {
    key: RefCell<Option<card::CardKey>>,
    shown: RefCell<Option<card::Shown>>,
    stand_in: Cell<Option<u64>>,
    held: Cell<Option<Instant>>,
    stack: RefCell<Option<Arc<RenderImage>>>,
}

impl PreviewWindow {
    fn close(self, cx: &mut App) {
        self.handle
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
}

impl Drop for PreviewWindow {
    fn drop(&mut self) {
        let center = NSNotificationCenter::defaultCenter();
        for observer in &self.follow {
            unsafe { center.removeObserver(observer.as_ref()) };
        }
    }
}

/// From AppKit's layout: gpui's viewport catches up with a window's resize only at its next draw.
fn pane_frame(parent: &NSWindow, metrics: SidebarMetrics) -> NSRect {
    let pt = |value: Pixels| f64::from(f32::from(value));
    let content = parent.contentRectForFrameRect(parent.frame());
    // Clear of the divider's grab and the parent's resize edges, which a child window takes.
    let width = pt(metrics.right) - f64::from(DIVIDER_GRAB + RESIZE_EDGE);
    let bottom = pt(metrics.bottom) + f64::from(RESIZE_EDGE);
    NSRect::new(
        NSPoint::new(
            content.origin.x + content.size.width - f64::from(RESIZE_EDGE) - width,
            content.origin.y + bottom,
        ),
        NSSize::new(width, content.size.height - pt(metrics.top) - bottom),
    )
}

fn stray(parent: &NSWindow, pane: &NSWindow) -> bool {
    parent.isVisible()
        && (!pane.isVisible()
            || pane
                .parentWindow()
                .is_none_or(|owner| !std::ptr::eq(&*owner, parent)))
}

fn off(parent: &NSWindow, pane: &NSWindow, metrics: SidebarMetrics) -> bool {
    stray(parent, pane) || !near(pane.frame(), pane_frame(parent, metrics))
}

/// Read when applied: a window manager moves the window between a draw and a deferred placement.
fn place(parent: &NSWindow, pane: &NSWindow, sidebar: &NativeSidebar) {
    let frame = pane_frame(parent, sidebar.metrics());
    if stray(parent, pane) {
        parent.removeChildWindow(pane);
        attach(parent, pane);
    }
    if !near(pane.frame(), frame) {
        pane.setFrame_display(frame, true);
    }
}

fn attach(parent: &NSWindow, pane: &NSWindow) {
    unsafe { parent.addChildWindow_ordered(pane, NSWindowOrderingMode::Above) };
    pane.setLevel(parent.level());
}

/// Moves the pane within the parent's own resize: placed from the next frame, it trails the edge.
fn follow_parent(
    parent: &NSWindow,
    pane: &Retained<NSWindow>,
    sidebar: &NativeSidebar,
) -> [Retained<ProtocolObject<dyn NSObjectProtocol>>; 3] {
    let (host, card, sidebar) = (Weak::new(parent), pane.clone(), sidebar.clone());
    let resized = StackBlock::new(move |_: NonNull<NSNotification>| {
        if let Some(host) = host.load() {
            place(&host, &card, &sidebar);
        }
    })
    .copy();
    let (host, card) = (Weak::new(parent), pane.clone());
    // Unhiding the app adds a child to its parent's movement group again, and it moves once per copy.
    let unhidden = StackBlock::new(move |_: NonNull<NSNotification>| {
        if let Some(host) = host.load().filter(|host| host.isVisible()) {
            host.removeChildWindow(&card);
            attach(&host, &card);
        }
    })
    .copy();
    let center = NSNotificationCenter::defaultCenter();
    unsafe {
        [
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWindowDidResizeNotification),
                Some(parent),
                None,
                &resized,
            ),
            center.addObserverForName_object_queue_usingBlock(
                Some(NSApplicationDidUnhideNotification),
                None,
                None,
                &unhidden,
            ),
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWindowDidMoveNotification),
                Some(pane),
                None,
                &resized,
            ),
        ]
    }
}

fn near(a: NSRect, b: NSRect) -> bool {
    (a.origin.x - b.origin.x).abs() < 0.5
        && (a.origin.y - b.origin.y).abs() < 0.5
        && (a.size.width - b.size.width).abs() < 0.5
        && (a.size.height - b.size.height).abs() < 0.5
}

/// A closing window offers key to this pane's panel, which declines it and leaves no key window.
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
            if let Some(window) = app.orderedWindows().iter().find(|window| {
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

struct PreviewPane {
    /// Weak: a strong handle keeps a closed window's whole state alive.
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
            .stand_in
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

/// With the window, not only its state: the pane's last frame may be what holds that state.
pub(crate) fn close_with_finder(window: &Window, cx: &mut Context<FinderDemo>) {
    let (id, finder) = (window.window_handle().window_id(), cx.entity().downgrade());
    let closed = cx.on_window_closed(move |cx, closed| {
        let taken = (closed == id).then(|| finder.update(cx, |this, _| this.preview_window.take()));
        if let Some(Ok(Some(pane))) = taken {
            pane.close(cx);
        }
    });
    cx.on_release(move |_, _| drop(closed)).detach();
}

fn open_preview_window(
    finder: Entity<FinderDemo>,
    parent: Option<Retained<NSWindow>>,
    cx: &mut App,
) {
    let pane_finder = finder.downgrade();
    let handed = Rc::new(Handed::default());
    let sidebar = finder.read(cx).sidebar.clone();
    let opened = parent.zip(sidebar).and_then(|(parent, sidebar)| {
        let opening = pane_frame(&parent, sidebar.metrics()).size;
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
                        size: size(px(opening.width as f32), px(opening.height as f32)),
                    })),
                    ..window_options()
                },
                |_, cx| {
                    cx.new(|_| PreviewPane {
                        finder: pane_finder,
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
        // After the style mask: no drag may carry the pane away from its window.
        window.setMovable(false);
        window.setMovableByWindowBackground(false);
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
        place(&parent, &window, &sidebar);
        attach(&parent, &window);
        let follow = follow_parent(&parent, &window, &sidebar);
        Some(PreviewWindow {
            handle,
            window,
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

/// A saved width outside what the divider allows, or none at all, is a bad one.
pub(crate) fn usable(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(MIN_W, MAX_W)
    } else {
        PREVIEW_W
    }
}

impl FinderDemo {
    pub(crate) fn preview_stray(&self) -> bool {
        match (&self.preview_window, &self.host, &self.sidebar) {
            (Some(pane), Some(parent), _) => self.preview_shown() && stray(parent, &pane.window),
            _ => false,
        }
    }

    pub(crate) fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        if self.preview_stray() {
            self.preview_opening = false;
            if let Some(pane) = self.preview_window.take() {
                cx.defer(move |cx| pane.close(cx));
            }
        } else {
            self.show_preview(!self.preview);
        }
        cx.notify();
    }

    pub(crate) fn show_preview(&mut self, shown: bool) {
        if shown && !self.preview && self.view != COLUMN_VIEW {
            self.preview_gallery = self.view == GALLERY_VIEW;
            self.preview_w = PREVIEW_W;
            crate::prefs::set_pane_width(self.preview_gallery, PREVIEW_W);
        }
        self.preview = shown;
        crate::prefs::set_preview(self.view, shown);
    }

    pub(crate) fn sync_preview_window(&mut self, window: &Window, cx: &mut Context<Self>) {
        crate::inspector::follow(self, window, cx);
        options::follow(self, window, cx);
        if let Some(sidebar) = &self.sidebar {
            let gallery = self.view == GALLERY_VIEW;
            if gallery != self.preview_gallery {
                self.preview_gallery = gallery;
                self.preview_w = usable(crate::prefs::pane_width(gallery).unwrap_or(PREVIEW_W));
                sidebar.set_inspector_thickness(px(self.preview_w));
            }
            // AppKit lays the divider on a whole point, half a point off the width asked for.
            match sidebar.take_inspector_resize().map(f32::from) {
                Some(width) if width <= 0. => {
                    self.preview = false;
                    crate::prefs::set_preview(self.view, false);
                    cx.notify();
                }
                Some(width) if (width - self.preview_w).abs() > 0.5 => {
                    self.preview_w = usable(width.round());
                    crate::prefs::set_pane_width(gallery, self.preview_w);
                    cx.notify();
                }
                _ => {}
            }
            sidebar.set_inspector(
                self.preview_shown().then(|| px(self.preview_w)),
                px(MIN_W),
                px(MAX_W),
            );
        }
        let metrics = self.metrics();
        let viewport = window.viewport_size();
        let key = window.is_window_active();
        self.list_focused = self.focus.is_focused(window) && key;
        self.list_room =
            f32::from(viewport.width) - f32::from(metrics.left) - f32::from(metrics.right);
        self.pane_h = f32::from(viewport.height - metrics.top - metrics.bottom) - RESIZE_EDGE;
        self.fit_name_column();

        if metrics.right <= px(DIVIDER_GRAB + RESIZE_EDGE) {
            if let Some(pane) = self.preview_window.take() {
                cx.defer(move |cx| pane.close(cx));
            }
            return;
        }

        match &self.preview_window {
            Some(pane) => {
                if let (Some(parent), Some(sidebar)) = (&self.host, &self.sidebar) {
                    if off(parent, &pane.window, metrics) {
                        // Off this draw: resizing from inside the parent's frame loses the size change.
                        let (parent, window, sidebar) =
                            (parent.clone(), pane.window.clone(), sidebar.clone());
                        cx.spawn(async move |_, _| place(&parent, &window, &sidebar))
                            .detach();
                    }
                }
            }
            None if self.preview_opening => {}
            None => {
                self.preview_opening = true;
                let finder = cx.entity();
                let parent = ns_window(window);
                cx.defer(move |cx| open_preview_window(finder, parent, cx));
            }
        }
        let Some(pane) = &self.preview_window else {
            return;
        };
        let content = self.preview_w - 2. * INSET;
        let card = Some(self.card_key(content, key));
        let handed = &pane.handed;
        let landed = handed
            .stand_in
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
            handed.stand_in.set(None);
            App::notify(cx, pane.pane);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_width_is_held_to_what_the_divider_allows() {
        assert_eq!(usable(0.), MIN_W);
        assert_eq!(usable(3.), MIN_W);
        assert_eq!(usable(-40.), MIN_W);
        assert_eq!(usable(300.), 300.);
        assert_eq!(usable(9000.), MAX_W);
    }

    #[test]
    fn a_width_that_is_not_a_number_falls_back_to_the_default() {
        assert_eq!(usable(f32::NAN), PREVIEW_W);
        assert_eq!(usable(f32::INFINITY), PREVIEW_W);
    }
}
