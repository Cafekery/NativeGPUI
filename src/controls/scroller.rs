use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{App, ElementId, Pixels, Point, ScrollHandle, Style, Window, point, px};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSEvent, NSResponder, NSScrollView, NSView, NSViewBoundsDidChangeNotification,
};
use objc2_foundation::{MainThreadMarker, NSNotificationCenter, NSObject, NSPoint, NSRect, NSSize};

use crate::bridge::host::{Layer, NativeRead, Slot, Target, full_size, hosted_element};

const FLASH_EVERY: Duration = Duration::from_millis(500);

/// AppKit's scroll bars over GPUI content, driving the handles; size it to the scrolled area.
pub fn native_scroll_bars(id: impl Into<ElementId>) -> NativeScrollBars {
    NativeScrollBars {
        id: id.into(),
        vertical: None,
        horizontal: None,
    }
}

pub struct NativeScrollBars {
    id: ElementId,
    vertical: Option<ScrollHandle>,
    horizontal: Option<ScrollHandle>,
}

impl NativeScrollBars {
    pub fn vertical(mut self, handle: &ScrollHandle) -> Self {
        self.vertical = Some(handle.clone());
        self
    }

    pub fn horizontal(mut self, handle: &ScrollHandle) -> Self {
        self.horizontal = Some(handle.clone());
        self
    }

    fn layout(&self) -> Style {
        full_size()
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let bars = slot.place(Layer::Above, |mtm, target| {
            ScrollBars::new(mtm, target, slot.key)
        });
        let (across, x) = travel(self.horizontal.as_ref(), |p| p.x);
        let (down, y) = travel(self.vertical.as_ref(), |p| p.y);
        let area = slot.bounds.size;
        let content = NSSize::new(
            f64::from(area.width) + across,
            f64::from(area.height) + down,
        );
        bars.show(content, across >= 1., down >= 1., NSPoint::new(x, y));

        let (vertical, horizontal) = (self.vertical.take(), self.horizontal.take());
        slot.on_event(
            read(),
            Some(Rc::new(move |value, window, _| {
                let Ok(origin) = value.downcast::<NSPoint>() else {
                    return;
                };
                if let Some(h) = &horizontal {
                    h.set_offset(point(px(-origin.x as f32), h.offset().y));
                }
                if let Some(v) = &vertical {
                    v.set_offset(point(v.offset().x, px(-origin.y as f32)));
                }
                window.refresh();
            })),
        );
    }
}

hosted_element!(NativeScrollBars);

fn travel(handle: Option<&ScrollHandle>, along: fn(Point<Pixels>) -> Pixels) -> (f64, f64) {
    handle.map_or((0., 0.), |handle| {
        let max = f64::from(along(handle.max_offset()));
        (max, (-f64::from(along(handle.offset()))).clamp(0., max))
    })
}

fn read() -> NativeRead {
    Rc::new(|sender| {
        let bars = unsafe { &*(sender as *const AnyObject).cast::<NSScrollView>() };
        Box::new(bars.contentView().bounds().origin)
    })
}

struct Ivars {
    key: u64,
    target: Retained<Target>,
    syncing: Cell<bool>,
    shown: Cell<bool>,
    flashed: Cell<Option<Instant>>,
}

define_class!(
    #[unsafe(super(NSScrollView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGScrollBars"]
    #[ivars = Ivars]
    struct ScrollBars;

    impl ScrollBars {
        /// The host finds a sender's handler by its tag.
        #[unsafe(method(tag))]
        fn tag(&self) -> isize {
            self.ivars().key as isize
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, at: NSPoint) -> *mut NSView {
            let hit: *mut NSView = unsafe { msg_send![super(self), hitTest: at] };
            let Some(view) = (unsafe { hit.as_ref() }) else {
                return hit;
            };
            let on_a_bar = [self.verticalScroller(), self.horizontalScroller()]
                .into_iter()
                .flatten()
                .any(|bar| view.isDescendantOf(&bar));
            if on_a_bar { hit } else { std::ptr::null_mut() }
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            if let Some(next) = unsafe { self.nextResponder() } {
                next.scrollWheel(event);
            }
        }

        #[unsafe(method(ngClipMoved))]
        fn clip_moved(&self) {
            if !self.ivars().syncing.get() {
                let _: () = unsafe { msg_send![&*self.ivars().target, ngAction: self] };
            }
        }
    }
);

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGScrollDocument"]
    struct Document;

    impl Document {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl ScrollBars {
    fn new(mtm: MainThreadMarker, target: &Target, key: u64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            key,
            target: target.retain(),
            syncing: Cell::new(false),
            shown: Cell::new(false),
            flashed: Cell::new(None),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        this.setDrawsBackground(false);
        this.setAutohidesScrollers(true);
        let document: Retained<Document> =
            unsafe { msg_send![Document::alloc(mtm), initWithFrame: NSRect::ZERO] };
        this.setDocumentView(Some(&document));
        let clip = this.contentView();
        clip.setPostsBoundsChangedNotifications(true);
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &this,
                sel!(ngClipMoved),
                Some(NSViewBoundsDidChangeNotification),
                Some(&clip),
            );
        }
        this
    }

    fn show(&self, content: NSSize, across: bool, down: bool, offset: NSPoint) {
        self.setHasHorizontalScroller(across);
        self.setHasVerticalScroller(down);
        if let Some(document) = self.documentView() {
            if document.frame().size != content {
                document.setFrameSize(content);
            }
        }
        let clip = self.contentView();
        if clip.bounds().origin != offset {
            self.ivars().syncing.set(true);
            clip.scrollToPoint(offset);
            self.reflectScrolledClipView(&clip);
            self.ivars().syncing.set(false);
            // Each flash invalidates the window's cursor rects, so it is rate-limited.
            let due = self
                .ivars()
                .flashed
                .get()
                .is_none_or(|at| at.elapsed() >= FLASH_EVERY);
            if self.ivars().shown.get() && due {
                self.flashScrollers();
                self.ivars().flashed.set(Some(Instant::now()));
            }
        }
        self.ivars().shown.set(true);
    }
}
