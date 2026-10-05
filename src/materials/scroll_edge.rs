use std::cell::Cell;

use gpui::{App, ElementId, Pixels, Size, Style, Window, px};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSColor, NSControl, NSResponder, NSScrollView, NSTableHeaderView, NSTableView, NSView,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSObject, NSPoint, NSRect, NSSize};

use crate::bridge::host::{Layer, Slot, hosted_element};

/// AppKit's scroll edge effect over GPUI content scrolled beneath the titlebar; size it to that area.
pub fn scroll_edge_effect(id: impl Into<ElementId>) -> ScrollEdgeEffect {
    ScrollEdgeEffect {
        id: id.into(),
        header: px(0.),
    }
}

pub struct ScrollEdgeEffect {
    id: ElementId,
    header: Pixels,
}

impl ScrollEdgeEffect {
    /// A column header this tall under the titlebar: the effect runs on over it, to AppKit's rule.
    pub fn header(mut self, height: Pixels) -> Self {
        self.header = height;
        self
    }

    fn layout(&self) -> Style {
        Style {
            size: Size::full(),
            ..Default::default()
        }
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let size = NSSize::new(slot.bounds.size.width.into(), self.header.into());
        slot.place(Layer::Above, |mtm, _| Edge::new(mtm))
            .set_header(size);
    }
}

hosted_element!(ScrollEdgeEffect);

struct Ivars {
    header: Retained<NSTableHeaderView>,
    written: Cell<NSSize>,
}

define_class!(
    #[unsafe(super(NSScrollView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGScrollEdge"]
    #[ivars = Ivars]
    struct Edge;

    impl Edge {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }

        #[unsafe(method(isAccessibilityElement))]
        fn is_accessibility_element(&self) -> bool {
            false
        }

        #[unsafe(method(accessibilityChildren))]
        fn accessibility_children(&self) -> *mut NSArray {
            std::ptr::null_mut()
        }
    }
);

define_class!(
    #[unsafe(super(NSTableView, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGScrollEdgeDocument"]
    struct Document;

    impl Document {
        /// AppKit tints the edge with an opaque document's background colour; this one paints none.
        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }

        /// Overriding this also keeps AppKit from laying its content background under the document.
        #[unsafe(method(drawBackgroundInClipRect:))]
        fn draw_background(&self, _clip: NSRect) {}
    }
);

impl Edge {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let header = NSTableHeaderView::initWithFrame(NSTableHeaderView::alloc(mtm), NSRect::ZERO);
        let this = Self::alloc(mtm).set_ivars(Ivars {
            header: header.clone(),
            written: Cell::new(NSSize::new(-1., -1.)),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        this.setDrawsBackground(false);
        let document: Retained<Document> =
            unsafe { msg_send![Document::alloc(mtm), initWithFrame: NSRect::ZERO] };
        document.setBackgroundColor(&NSColor::controlBackgroundColor());
        document.setCornerView(None);
        document.setHeaderView(Some(&header));
        this.setDocumentView(Some(&document));
        this
    }

    fn set_header(&self, size: NSSize) {
        if self.ivars().written.get() == size {
            return;
        }
        self.ivars().written.set(size);
        if let Some(document) = self.documentView() {
            document.setFrameSize(NSSize::new(size.width, document.frame().size.height));
        }
        self.ivars().header.setFrameSize(size);
        self.tile();
    }
}
