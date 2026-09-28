use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::ptr;

use gpui::{Bounds, Pixels, Window};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Imp, NSObjectProtocol, Sel};
use objc2::{ClassType, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{NSAccessibilityUnignoredChildren, NSAutoresizingMaskOptions, NSView};
use objc2_foundation::{MainThreadMarker, NSArray, NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

define_class!(
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

pub fn gpui_view(window: &Window) -> Retained<NSView> {
    let handle = HasWindowHandle::window_handle(window)
        .expect("gpui's macOS window always exposes an AppKit window handle");
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        unreachable!("macOS-only crate saw a non-AppKit window handle");
    };
    unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }
        .expect("gpui's AppKit handle always carries the live NSView it renders into")
}

/// gpui draws and presents this frame now; never call it while gpui holds its `App`.
pub(crate) fn draw_now(gpui: &NSView) {
    if let Some(layer) = gpui.layer() {
        layer.setNeedsDisplay();
        layer.displayIfNeeded();
    }
}

pub fn to_rect(bounds: Bounds<Pixels>) -> NSRect {
    NSRect::new(
        NSPoint::new(bounds.origin.x.to_f64(), bounds.origin.y.to_f64()),
        NSSize::new(bounds.size.width.to_f64(), bounds.size.height.to_f64()),
    )
}

pub fn focus_gpui(window: &Window) {
    let view = gpui_view(window);
    if let Some(ns_window) = view.window() {
        ns_window.makeFirstResponder(Some(&**view));
    }
}

const KEY_PATH_CLASS: &CStr = c"NGKeyPathView";

/// GPUI's key overrides eat hosted controls' keys, so they run only while GPUI is first responder.
pub fn adopt_key_path(view: &NSView) {
    let current = view.class();
    if current.name() == KEY_PATH_CLASS {
        return;
    }
    let subclass = AnyClass::get(KEY_PATH_CLASS).unwrap_or_else(|| {
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
    });
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

const HOSTED_PREFIX: &str = "NGHosted";

/// gpui's accessibility adapter answers the content view's children with gpui's tree alone.
pub fn adopt_hosted_accessibility(content: &NSView) {
    let current = content.class();
    let base = current.name().to_string_lossy();
    if base.starts_with(HOSTED_PREFIX) {
        return;
    }
    let name = CString::new(format!("{HOSTED_PREFIX}{base}")).expect("class names hold no NUL");
    let subclass = AnyClass::get(&name).unwrap_or_else(|| {
        let mut builder = ClassBuilder::new(&name, current)
            .expect("only adopt_hosted_accessibility registers NGHosted classes");
        unsafe {
            builder.add_method(
                sel!(accessibilityChildren),
                hosted_children as extern "C" fn(*mut AnyObject, Sel) -> *mut NSArray,
            );
            builder.add_method(
                sel!(accessibilityHitTest:),
                hosted_hit_test as extern "C" fn(*mut AnyObject, Sel, NSPoint) -> *mut AnyObject,
            );
        }
        builder.register()
    });
    unsafe { AnyObject::set_class(&*(content as *const NSView).cast::<AnyObject>(), subclass) };
}

fn inherited_imp(view: *mut AnyObject, selector: Sel) -> Imp {
    let mut class = unsafe { &*view }.class();
    while !class
        .name()
        .to_bytes()
        .starts_with(HOSTED_PREFIX.as_bytes())
    {
        class = class
            .superclass()
            .expect("an adopted view's class always descends from its NGHosted class");
    }
    class
        .superclass()
        .and_then(|base| base.instance_method(selector))
        .expect("NSView answers every accessibility selector")
        .implementation()
}

extern "C" fn hosted_children(view: *mut AnyObject, selector: Sel) -> *mut NSArray {
    let inherited: extern "C" fn(*mut AnyObject, Sel) -> *mut NSArray =
        unsafe { std::mem::transmute(inherited_imp(view, selector)) };
    let mut children = unsafe { inherited(view, selector).as_ref() }
        .map(NSArray::to_vec)
        .unwrap_or_default();
    let subviews: Retained<NSArray> = unsafe { msg_send![view, subviews] };
    for child in unsafe { NSAccessibilityUnignoredChildren(&subviews) }.iter() {
        if !children
            .iter()
            .any(|known| Retained::as_ptr(known) == Retained::as_ptr(&child))
        {
            children.push(child);
        }
    }
    Retained::autorelease_return(NSArray::from_retained_slice(&children))
}

extern "C" fn hosted_hit_test(
    view: *mut AnyObject,
    selector: Sel,
    point: NSPoint,
) -> *mut AnyObject {
    let content = unsafe { &*view.cast::<NSView>() };
    let hit = content
        .window()
        .zip(unsafe { content.superview() })
        .and_then(|(window, frame)| {
            content.hitTest(frame.convertPoint_fromView(window.convertPointFromScreen(point), None))
        });
    if let Some(hit) = hit.filter(|hit| is_hosted(hit, content)) {
        return unsafe { msg_send![&*hit, accessibilityHitTest: point] };
    }
    let inherited: extern "C" fn(*mut AnyObject, Sel, NSPoint) -> *mut AnyObject =
        unsafe { std::mem::transmute(inherited_imp(view, selector)) };
    inherited(view, selector, point)
}

fn is_hosted(view: &NSView, content: &NSView) -> bool {
    let mut at = Some(view.retain());
    while let Some(view) = at {
        if ptr::eq(&*view, content) {
            return false;
        }
        if view.isKindOfClass(Container::class()) {
            return true;
        }
        at = unsafe { view.superview() };
    }
    false
}

pub fn rects_equal(a: NSRect, b: NSRect) -> bool {
    a == b
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
