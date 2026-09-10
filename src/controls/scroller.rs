use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, ElementId, ScrollHandle, Style, Window, point, px, relative, size};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSEvent, NSResponder, NSScrollView, NSView, NSViewBoundsDidChangeNotification,
};
use objc2_foundation::{MainThreadMarker, NSNotificationCenter, NSObject, NSPoint, NSRect, NSSize};

use crate::bridge::host::{Layer, NativeRead, Slot, Target, hosted_element};

/// AppKit's own scroll bars over GPUI content: a click-through `NSScrollView` the size of the
/// scrolled area, kept at the handles' offsets, whose scrollers move the handles in turn. Place it
/// over the area, sized to it.
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
        let mut style = Style::default();
        style.size = size(relative(1.).into(), relative(1.).into());
        style
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let key = slot.key;
        let bars = slot.place(Layer::Above, |mtm, target| {
            ScrollBars::new(mtm, target, key)
        });
        let (across, down) = (
            self.horizontal
                .as_ref()
                .map_or(0., |h| f64::from(h.max_offset().x)),
            self.vertical
                .as_ref()
                .map_or(0., |v| f64::from(v.max_offset().y)),
        );
        let offset = NSPoint::new(
            self.horizontal
                .as_ref()
                .map_or(0., |h| -f64::from(h.offset().x))
                .clamp(0., across),
            self.vertical
                .as_ref()
                .map_or(0., |v| -f64::from(v.offset().y))
                .clamp(0., down),
        );
        let area = slot.bounds.size;
        let content = NSSize::new(
            f64::from(area.width) + across,
            f64::from(area.height) + down,
        );
        bars.show(content, across >= 1., down >= 1., offset);

        let (vertical, horizontal) = (self.vertical.clone(), self.horizontal.clone());
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

        /// Only the scrollers take the pointer; the GPUI content beneath takes everything else.
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
            match on_a_bar {
                true => hit,
                false => std::ptr::null_mut(),
            }
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
            // GPUI moving the content flashes the scrollers, as a scroll view's own scrolling does.
            if self.ivars().shown.get() {
                self.flashScrollers();
            }
        }
        self.ivars().shown.set(true);
    }
}
