use std::path::PathBuf;
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::Window;
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSDragOperation, NSDraggingInfo, NSPasteboard, NSPasteboardNameDrag,
    NSPasteboardTypeFileURL, NSView, NSWindow, NSWindowButton, NSWindowOrderingMode,
    NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSPoint, NSRect, NSSize, NSURL};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::open::url_path;

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static INSTALLED: AtomicBool = AtomicBool::new(false);

pub fn take_dropped() -> Vec<PathBuf> {
    std::mem::take(&mut *PENDING.lock().unwrap())
}

pub fn has_dropped() -> bool {
    !PENDING.lock().unwrap().is_empty()
}

pub fn hide_traffic_lights(window: &Window) {
    let Some(ns_window) = ns_window(window) else {
        return;
    };
    for button in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(light) = ns_window.standardWindowButton(button) {
            light.setHidden(true);
        }
    }
    ns_window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
}

pub fn enable_resize(window: &Window) {
    let Some(ns_window) = ns_window(window) else {
        return;
    };
    let mut style = ns_window.styleMask();
    if !style.contains(NSWindowStyleMask::Resizable) {
        style.insert(NSWindowStyleMask::Resizable);
        ns_window.setStyleMask(style);
    }
    let min = NSSize::new(420., 520.);
    if ns_window.contentMinSize() != min {
        ns_window.setContentMinSize(min);
    }
}

pub fn install(window: &Window) {
    if INSTALLED.load(Ordering::Relaxed) {
        return;
    }
    let view = gpui_view(window);
    let Some(parent) = (unsafe { view.superview() }) else {
        return;
    };
    let mtm = MainThreadMarker::from(&*view);
    let drop = DropView::new(mtm, parent.bounds());
    drop.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    drop.registerForDraggedTypes(&NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]));
    parent.addSubview_positioned_relativeTo(&drop, NSWindowOrderingMode::Above, Some(&view));
    INSTALLED.store(true, Ordering::Relaxed);
}

fn gpui_view(window: &Window) -> Retained<NSView> {
    let handle = HasWindowHandle::window_handle(window)
        .expect("gpui's macOS window always exposes an AppKit window handle");
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        unreachable!("macOS-only crate saw a non-AppKit window handle");
    };
    unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }
        .expect("gpui's AppKit handle always carries the live NSView it renders into")
}

fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    gpui_view(window).window()
}

fn drag_has_files() -> bool {
    let pb = NSPasteboard::pasteboardWithName(unsafe { NSPasteboardNameDrag });
    pb.availableTypeFromArray(&NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]))
        .is_some()
}

fn paths_from(info: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<PathBuf> {
    let Some(items) = info.draggingPasteboard().pasteboardItems() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let text = item.stringForType(unsafe { NSPasteboardTypeFileURL })?;
            let url = NSURL::URLWithString(&text)?;
            url_path(&url)
        })
        .collect()
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGConvertDrop"]
    struct DropView;

    impl DropView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            if drag_has_files() {
                ptr::from_ref(self).cast_mut().cast()
            } else {
                ptr::null_mut()
            }
        }

        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            if paths_from(sender).is_empty() {
                NSDragOperation::empty()
            } else {
                NSDragOperation::Copy
            }
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag_operation(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> Bool {
            let paths = paths_from(sender);
            if paths.is_empty() {
                return false.into();
            }
            PENDING.lock().unwrap().extend(paths);
            true.into()
        }
    }
);

impl DropView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), initWithFrame: frame] }
    }
}
