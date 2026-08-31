//! The whole ObjC surface this crate needs: how to reach GPUI's view, and one container class.

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
use objc2_quartz_core::CALayer;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

define_class!(
    /// A flipped, event-transparent container.
    ///
    /// Flipped so an AppKit subview frame is GPUI's coordinate space verbatim — logical points from
    /// the top-left of the content view — which is why the crate holds no coordinate math.
    ///
    /// `hitTest:` declines its own area so mouse events no real control claimed reach GPUI. An
    /// optimisation, not a requirement: the responder chain would carry them there anyway.
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

/// The `NSView` GPUI renders its Metal content into. Its superview is the window's content view,
/// which is where [`crate::Layer::Below`] views are inserted.
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

/// The `CAMetalLayer` GPUI presents its scene through.
///
/// Identified by asking which layer answers `maximumDrawableCount` rather than by assuming a
/// class name or a position in the tree: GPUI builds its view class at runtime and the property is
/// the one thing only a Metal layer has.
pub fn metal_layer(view: &NSView) -> Option<Retained<CALayer>> {
    let layer = view.layer()?;
    let responds: bool =
        unsafe { msg_send![&*layer, respondsToSelector: sel!(maximumDrawableCount)] };
    responds.then_some(layer)
}

/// How many frames GPUI's *presented* Metal content trails the `CATransaction` that carried the
/// same frame's layout, as a count of frames.
///
/// `presentDrawable:` hands the surface to Core Animation asynchronously, so a frame is displayed
/// only once the drawables already queued ahead of it have been, and `CAMetalLayer` lets
/// `maximumDrawableCount` of them be in flight. GPUI draws one frame per display refresh and
/// blocks in `nextDrawable`, so the queue sits full and the trail is `maximumDrawableCount - 1`
/// refreshes — measured as exactly 2 at the default count of 3 and exactly 1 at 2, the same
/// integer at every scroll velocity.
///
/// A layer that is not GPUI's Metal layer trails by nothing, which is also the honest answer for a
/// window whose scene is not presented through one.
pub fn presentation_lag(view: &NSView) -> usize {
    let Some(layer) = metal_layer(view) else {
        return 0;
    };
    let count: usize = unsafe { msg_send![&*layer, maximumDrawableCount] };
    count.saturating_sub(1)
}

/// Give keyboard focus back to GPUI.
///
/// AppKit's first responder and GPUI's focus system are independent: a hosted `NSTextField` keeps
/// first responder after a click elsewhere, so keystrokes stay in its field editor while GPUI's
/// `FocusHandle` reports focused. [`crate::native_root`] calls this on any mouse-down GPUI sees,
/// which by construction is one no hosted view claimed.
pub fn focus_gpui(window: &Window) {
    let view = gpui_view(window);
    if let Some(ns_window) = view.window() {
        ns_window.makeFirstResponder(Some(&**view));
    }
}

/// The runtime subclass [`adopt_key_path`] installs. `ClassBuilder` rather than `define_class!`
/// because GPUI's view class is itself built at runtime: no superclass to name at compile time.
const KEY_PATH_CLASS: &CStr = c"NGKeyPathView";

/// Stop GPUI's view from swallowing keys that belong to a hosted AppKit control.
///
/// GPUI overrides `performKeyEquivalent:` and `keyDown:`; both have to behave like a plain
/// `NSView`'s whenever GPUI is not the first responder.
///
/// * AppKit walks `performKeyEquivalent:` **before** the first responder is sent `keyDown:`. GPUI
///   answers it for every keystroke it has no character for — every true function key — by handing
///   the event to the *active* input context, which is the hosted field editor's, so the editor is
///   sent `insertText:` with the function key's legacy ASCII spelling (← inserts U+001C,
///   forward-delete U+007F, Home U+0001, End U+0004) and the walk reports handled, so `moveLeft:`
///   never runs. GPUI's override also never calls `super`, so the walk never descends into hosted
///   views and a hosted default button never sees its Return.
/// * Space-activates-the-focused-control and arrow-adjusts-it are `NSWindow`'s `keyDown:` fallback,
///   reached only once the focused control declines the key and the chain carries it to the window.
///   GPUI's `keyDown:` is a dead end, so the event dies one link short of the responder that would
///   have acted. The same `NSSwitch` toggles on Space in the content view and does nothing inside
///   GPUI's view, with `acceptsFirstResponder`, `canBecomeKeyView` and its rect in the window
///   identical either way.
///
/// Installed on GPUI's own view instance per window, so GPUI's class is untouched and a window
/// with no hosted controls keeps GPUI's behaviour exactly. Idempotent.
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
    // Adds no instance variables, so GPUI's own ivar still resolves at the same offset.
    unsafe { AnyObject::set_class(&*(view as *const NSView).cast::<AnyObject>(), subclass) };
}

/// GPUI's own implementation of `selector`, reached via the subclass's superclass because
/// `msg_send![super(..)]` needs a superclass known at compile time.
fn gpui_imp(selector: Sel) -> Imp {
    AnyClass::get(KEY_PATH_CLASS)
        .and_then(AnyClass::superclass)
        .and_then(|base| base.instance_method(selector))
        .expect("NGKeyPathView is only ever registered as a subclass of GPUI's view class")
        .implementation()
}

/// What a view that overrode none of this would do: descend into the subviews for a key
/// equivalent, and pass a key down to the next responder.
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

/// A control's own preferred size, asked of AppKit once and cached.
///
/// A returned axis of `0.0` is AppKit reporting that axis **free** — a progress bar reports
/// `(0.0, 20.0)`, an `NSLevelIndicator` in the rating style `(60.0, 12.0)`. Branch on
/// `size.width > 0.` rather than deciding per class which axis GPUI drives, so a future macOS
/// changing its mind stays handled.
pub fn fitting_size(
    class: &'static str,
    measure: impl FnOnce(MainThreadMarker) -> NSSize,
) -> NSSize {
    fitting_size_keyed(class, 0, measure)
}

/// [`fitting_size`] for controls whose preferred size depends on their own properties — an
/// `NSLevelIndicator`'s width is a function of its range, so a per-class cache would freeze the
/// first range's answer. `key` is any hash of the inputs the measurement depends on.
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
        // Rounding here would misalign a hosted view against the GPUI row it belongs to.
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
