use std::cell::Cell;

use gpui::{App, ElementId, Pixels, Style, Window, px};
use objc2::rc::Retained;
#[cfg(feature = "private-api")]
use objc2::runtime::{AnyClass, NSObjectProtocol};
#[cfg(feature = "private-api")]
use objc2::sel;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSColor, NSControl, NSResponder, NSScrollView, NSTableHeaderView, NSTableView, NSView,
};
#[cfg(feature = "private-api")]
use objc2_foundation::ns_string;
use objc2_foundation::{MainThreadMarker, NSArray, NSObject, NSPoint, NSRect, NSSize};

#[cfg(feature = "private-api")]
use super::private_api;
use crate::bridge::host::{Layer, Slot, full_size, hosted_element};

/// AppKit's scroll edge effect over GPUI content scrolled under the titlebar; size it to that area.
pub fn scroll_edge_effect(id: impl Into<ElementId>) -> ScrollEdgeEffect {
    ScrollEdgeEffect {
        id: id.into(),
        header: px(0.),
        scrolled: px(0.),
    }
}

pub struct ScrollEdgeEffect {
    id: ElementId,
    header: Pixels,
    scrolled: Pixels,
}

impl ScrollEdgeEffect {
    /// A column header this tall under the titlebar: the effect runs on over it, to AppKit's rule.
    pub fn header(mut self, height: Pixels) -> Self {
        self.header = height;
        self
    }

    /// How far the content has scrolled up past the edge: AppKit's separator shows once it has.
    pub fn scrolled(mut self, distance: Pixels) -> Self {
        self.scrolled = distance;
        self
    }

    fn layout(&self) -> Style {
        full_size()
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let size = NSSize::new(slot.bounds.size.width.into(), self.header.into());
        let edge = slot.place(Layer::Above, |mtm, _| Edge::new(mtm));
        edge.set_header(size);
        edge.set_scrolled(f64::from(f32::from(self.scrolled).max(0.)));
    }
}

hosted_element!(ScrollEdgeEffect);

struct Ivars {
    header: Retained<Header>,
    document: Retained<Document>,
    header_size: Cell<NSSize>,
    rest: Cell<Option<(f64, f64)>>,
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

    #[cfg(feature = "private-api")]
    impl Edge {
        /// Once layout ties a pocket to a transparent titlebar it hides outside semantic context 7.
        #[unsafe(method(layout))]
        fn layout_pocket(&self) {
            let _: () = unsafe { msg_send![super(self), layout] };
            let Some(pocket) = AnyClass::get(c"NSScrollPocket") else {
                return;
            };
            for view in self.subviews().iter() {
                if view.isKindOfClass(pocket)
                    && private_api::read_bool(&*view, sel!(_hidesTitlebarElements)) == Some(true)
                {
                    private_api::send_isize(&*view, sel!(_setSemanticContext:), 7);
                    private_api::send_string(
                        &*view,
                        sel!(_invalidateDependenciesOnValueForKey:),
                        Some(ns_string!("_titlebarBackgroundStyle")),
                    );
                }
            }
        }

        /// A transparent titlebar's sections set style 1, which empties the pocket in any context.
        #[unsafe(method(setTitlebarBackgroundStyle:))]
        fn set_titlebar_background_style(&self, _style: isize) {
            let _: () = unsafe { msg_send![super(self), setTitlebarBackgroundStyle: 0isize] };
        }
    }
);

define_class!(
    #[unsafe(super(NSTableView, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGScrollEdgeDocument"]
    #[ivars = Cell<f64>]
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

        /// A table fits its rows; this keeps it tall enough for the clip to rest scrolled.
        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            let size = NSSize::new(size.width, size.height.max(self.ivars().get()));
            unsafe { msg_send![super(self), setFrameSize: size] }
        }
    }
);

define_class!(
    #[unsafe(super(NSTableHeaderView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGScrollEdgeHeader"]
    struct Header;

    impl Header {
        /// The edge's own separator rules the header off; AppKit's 1 pt rule would double it.
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {}
    }
);

impl Edge {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let header: Retained<Header> =
            unsafe { msg_send![Header::alloc(mtm), initWithFrame: NSRect::ZERO] };
        let document = Document::alloc(mtm).set_ivars(Cell::new(0.));
        let document: Retained<Document> =
            unsafe { msg_send![super(document), initWithFrame: NSRect::ZERO] };
        document.setBackgroundColor(&NSColor::controlBackgroundColor());
        document.setCornerView(None);
        document.setHeaderView(Some(&header));
        let this = Self::alloc(mtm).set_ivars(Ivars {
            header,
            document,
            header_size: Cell::new(NSSize::new(-1., -1.)),
            rest: Cell::new(None),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        this.setDrawsBackground(false);
        this.setDocumentView(Some(&this.ivars().document));
        this
    }

    fn set_header(&self, size: NSSize) {
        if self.ivars().header_size.replace(size) == size {
            return;
        }
        let document = &self.ivars().document;
        document.setFrameSize(NSSize::new(size.width, document.frame().size.height));
        self.ivars().header.setFrameSize(size);
        self.tile();
    }

    fn set_scrolled(&self, distance: f64) {
        let rest = &self.ivars().rest;
        if distance == 0. && rest.get().is_none() {
            return;
        }
        let clip = self.contentView();
        let origin = distance - self.contentInsets().top;
        let reach = clip.frame().size.height + distance;
        if rest.replace(Some((origin, reach))) == Some((origin, reach)) {
            return;
        }
        let document = &self.ivars().document;
        document.ivars().set(reach);
        document.setFrameSize(document.frame().size);
        clip.scrollToPoint(NSPoint::new(0., origin));
        self.reflectScrolledClipView(&clip);
    }
}
