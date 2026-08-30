//! The whole ObjC surface this crate needs: how to reach GPUI's view, and one container class.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr;

use gpui::{Bounds, Pixels, Window};
use objc2::rc::Retained;
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

define_class!(
    /// A flipped, event-transparent container.
    ///
    /// Flipped so AppKit subview frames are GPUI's coordinate space verbatim: logical points
    /// from the top-left of the window's content view, no conversion anywhere in the crate.
    ///
    /// `hitTest:` returns null for its own area so GPUI keeps receiving mouse events that no real
    /// control claimed. That is an optimisation rather than a requirement: AppKit forwards
    /// unhandled mouse events up the responder chain through these containers to GPUI's own view,
    /// so GPUI keeps working even when a hosted view claims every hit. Declining early just avoids
    /// the trip.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGContainer"]
    pub struct Container;

    impl Container {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            let hit: *mut NSView = unsafe { msg_send![super(self), hitTest: point] };
            if ptr::eq(hit.cast_const().cast::<Container>(), self) {
                ptr::null_mut()
            } else {
                hit
            }
        }
    }
);

impl Container {
    pub fn new(mtm: MainThreadMarker, frame: NSRect, clips: bool) -> Retained<Self> {
        let this: Retained<Self> = unsafe { msg_send![Self::alloc(mtm), initWithFrame: frame] };
        this.setClipsToBounds(clips);
        this
    }

    /// Track the parent's size, so a window resize needs no bookkeeping from us.
    pub fn autoresize(&self) {
        self.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
    }
}

/// The `NSView` GPUI renders its Metal content into.
///
/// Its superview is the `NSWindow`'s content view, which is where [`crate::Layer::Below`] views
/// are inserted.
pub fn gpui_view(window: &Window) -> Retained<NSView> {
    let handle = HasWindowHandle::window_handle(window)
        .expect("gpui's macOS window always exposes an AppKit window handle");
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        unreachable!("macOS-only crate saw a non-AppKit window handle");
    };
    unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }
        .expect("gpui's AppKit handle always carries the live NSView it renders into")
}

pub fn to_rect(bounds: Bounds<Pixels>) -> NSRect {
    NSRect::new(
        NSPoint::new(bounds.origin.x.to_f64(), bounds.origin.y.to_f64()),
        NSSize::new(bounds.size.width.to_f64(), bounds.size.height.to_f64()),
    )
}

/// Give keyboard focus back to GPUI.
///
/// A hosted `NSTextField` takes first responder on a real mouse-down and keeps it: clicking GPUI
/// content afterwards does **not** hand it back, because AppKit's first responder and GPUI's own
/// focus system are independent and GPUI never sees those key events. Measured: after clicking
/// away from a hosted field, the first responder was still its `NSTextView` and the next three
/// keystrokes went into the field while GPUI's `FocusHandle` reported focused the whole time.
/// [`crate::native_root`] calls this whenever GPUI receives a mouse-down, which by construction
/// means no hosted view claimed it.
pub fn focus_gpui(window: &Window) {
    let view = gpui_view(window);
    if let Some(ns_window) = view.window() {
        ns_window.makeFirstResponder(Some(&**view));
    }
}

pub fn rects_equal(a: NSRect, b: NSRect) -> bool {
    a.origin.x == b.origin.x
        && a.origin.y == b.origin.y
        && a.size.width == b.size.width
        && a.size.height == b.size.height
}

/// A control's own preferred size, asked of AppKit once and cached.
///
/// Sizes are read from the system rather than hardcoded, so a control that changes size in a
/// future macOS lays out correctly here without this crate being edited.
///
/// A returned axis of `0.0` is AppKit telling you that axis is **free** — a progress bar reports
/// `(0.0, 20.0)`, an `NSLevelIndicator` in the rating style reports `(60.0, 12.0)`. Branch on
/// `size.width > 0.` rather than deciding per class which axis GPUI should drive, so the answer
/// stays right if a future macOS changes its mind.
pub fn fitting_size(
    class: &'static str,
    measure: impl FnOnce(MainThreadMarker) -> NSSize,
) -> NSSize {
    fitting_size_keyed(class, 0, measure)
}

/// [`fitting_size`] for controls whose preferred size depends on their own properties — an
/// `NSLevelIndicator`'s width is a function of its range, so a per-class cache would return the
/// first range's answer forever. `key` is any hash of the inputs the measurement depends on.
pub fn fitting_size_keyed(
    class: &'static str,
    key: u64,
    measure: impl FnOnce(MainThreadMarker) -> NSSize,
) -> NSSize {
    thread_local! {
        static CACHE: RefCell<HashMap<(&'static str, u64), NSSize>> = RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        if let Some(size) = cache.borrow().get(&(class, key)) {
            return *size;
        }
        let mtm = MainThreadMarker::new().expect("fitting_size is main-thread only");
        let size = measure(mtm);
        cache.borrow_mut().insert((class, key), size);
        size
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px, size};

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        }
    }

    #[test]
    fn to_rect_is_the_identity_because_the_containers_are_flipped() {
        let r = to_rect(bounds(12., 34., 56., 78.));
        assert_eq!((r.origin.x, r.origin.y), (12., 34.));
        assert_eq!((r.size.width, r.size.height), (56., 78.));
    }

    #[test]
    fn to_rect_preserves_fractions() {
        // Fractional frames are stored faithfully by AppKit; rounding here would misalign a
        // hosted view against the GPUI row it belongs to.
        let r = to_rect(bounds(430.5, 201.25, 120.5, 32.75));
        assert_eq!((r.origin.x, r.origin.y), (430.5, 201.25));
        assert_eq!((r.size.width, r.size.height), (120.5, 32.75));
    }

    #[test]
    fn rects_equal_compares_all_four_fields() {
        let base = to_rect(bounds(1., 2., 3., 4.));
        assert!(rects_equal(base, to_rect(bounds(1., 2., 3., 4.))));
        for other in [
            bounds(9., 2., 3., 4.),
            bounds(1., 9., 3., 4.),
            bounds(1., 2., 9., 4.),
            bounds(1., 2., 3., 9.),
        ] {
            assert!(!rects_equal(base, to_rect(other)));
        }
    }
}
