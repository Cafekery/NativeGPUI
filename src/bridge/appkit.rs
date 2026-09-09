use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CStr;
use std::ptr;

use gpui::{Bounds, Pixels, Window};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Imp, Sel};
use objc2::{ClassType, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

define_class!(
    /// A flipped container, so subview frames are GPUI's top-left coordinates verbatim.
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

    pub fn autoresize(&self) {
        self.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
    }
}

/// The `NSView` GPUI renders into. Its superview is the window's content view.
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

/// AppKit's first responder and GPUI's focus are independent: a hosted field keeps first
/// responder after a click elsewhere.
pub fn focus_gpui(window: &Window) {
    let view = gpui_view(window);
    if let Some(ns_window) = view.window() {
        ns_window.makeFirstResponder(Some(&**view));
    }
}

const KEY_PATH_CLASS: &CStr = c"NGKeyPathView";

/// GPUI's overrides swallow function keys into the field editor as legacy ASCII and dead-end the
/// keys `NSWindow` would have used to activate the focused control.
pub fn adopt_key_path(view: &NSView) {
    let current = view.class();
    if current.name() == KEY_PATH_CLASS {
        return;
    }
    let subclass = match AnyClass::get(KEY_PATH_CLASS) {
        Some(existing) => existing,
        None => {
            let mut builder = ClassBuilder::new(KEY_PATH_CLASS, current)
                .expect("no other class in this process is named NGKeyPathView");
            unsafe {
                builder.add_method(
                    sel!(performKeyEquivalent:),
                    key_equivalent as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> Bool,
                );
                builder.add_method(
                    sel!(keyDown:),
                    key_down as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
                );
            }
            builder.register()
        }
    };
    unsafe { AnyObject::set_class(&*(view as *const NSView).cast::<AnyObject>(), subclass) };
}

fn gpui_imp(selector: Sel) -> Imp {
    AnyClass::get(KEY_PATH_CLASS)
        .and_then(AnyClass::superclass)
        .and_then(|base| base.instance_method(selector))
        .expect("NGKeyPathView is only ever registered as a subclass of GPUI's view class")
        .implementation()
}

fn plain_imp(selector: Sel) -> Imp {
    NSView::class()
        .instance_method(selector)
        .expect("NSView inherits both performKeyEquivalent: and keyDown: from NSResponder")
        .implementation()
}

fn owns_keyboard(view: *mut AnyObject) -> bool {
    let window: *mut AnyObject = unsafe { msg_send![view, window] };
    if window.is_null() {
        return true;
    }
    let responder: *mut AnyObject = unsafe { msg_send![window, firstResponder] };
    ptr::eq(responder, view)
}

extern "C" fn key_equivalent(view: *mut AnyObject, _: Sel, event: *mut AnyObject) -> Bool {
    let selector = sel!(performKeyEquivalent:);
    let walk: extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> Bool =
        unsafe { std::mem::transmute(plain_imp(selector)) };
    if walk(view, selector, event).as_bool() {
        return Bool::YES;
    }
    if !owns_keyboard(view) {
        return Bool::NO;
    }
    let gpui: extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> Bool =
        unsafe { std::mem::transmute(gpui_imp(selector)) };
    gpui(view, selector, event)
}

extern "C" fn key_down(view: *mut AnyObject, _: Sel, event: *mut AnyObject) {
    let selector = sel!(keyDown:);
    let imp = if owns_keyboard(view) {
        gpui_imp(selector)
    } else {
        plain_imp(selector)
    };
    let call: extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) =
        unsafe { std::mem::transmute(imp) };
    call(view, selector, event);
}

pub fn rects_equal(a: NSRect, b: NSRect) -> bool {
    a.origin.x == b.origin.x
        && a.origin.y == b.origin.y
        && a.size.width == b.size.width
        && a.size.height == b.size.height
}

/// A `0.0` axis is AppKit declaring that axis free.
pub fn fitting_size(
    class: &'static str,
    measure: impl FnOnce(MainThreadMarker) -> NSSize,
) -> NSSize {
    fitting_size_keyed(class, 0, measure)
}

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
    fn to_rect_preserves_fractions() {
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
