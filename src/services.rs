use std::cell::RefCell;
use std::path::PathBuf;

use gpui::Window;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, Bool, ProtocolObject, Sel};
use objc2::{ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardWriting, NSWindow,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSObject, NSString, NSURL};

use crate::bridge::appkit::gpui_window;

struct RequestorIvars {
    files: RefCell<Vec<PathBuf>>,
}

define_class!(
    // AppKit ignores `writeSelectionToPasteboard:types:` added to GPUI's window itself.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGServicesRequestor"]
    #[ivars = RequestorIvars]
    struct Requestor;

    impl Requestor {
        #[unsafe(method(writeSelectionToPasteboard:types:))]
        fn write_selection(&self, board: &NSPasteboard, _types: *mut AnyObject) -> Bool {
            let urls: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = self
                .ivars()
                .files
                .borrow()
                .iter()
                .map(|file| {
                    let path = NSString::from_str(&file.to_string_lossy());
                    ProtocolObject::from_retained(NSURL::fileURLWithPath(&path))
                })
                .collect();
            if urls.is_empty() {
                return Bool::NO;
            }
            board.clearContents();
            Bool::new(board.writeObjects(&NSArray::from_retained_slice(&urls)))
        }
    }
);

thread_local! {
    static REQUESTORS: RefCell<Vec<(Weak<NSWindow>, Retained<Requestor>)>> = const { RefCell::new(Vec::new()) };
}

/// The files the Services menu acts on while `window` is key.
pub fn services_selection(window: &Window, files: Vec<PathBuf>) {
    let ns_window = gpui_window(window);
    offer_files(&ns_window);
    REQUESTORS.with_borrow_mut(|all| {
        all.retain(|(held, _)| held.load().is_some());
        let mine = all
            .iter()
            .find(|(held, _)| held.load().is_some_and(|held| held == ns_window));
        if let Some((_, requestor)) = mine {
            *requestor.ivars().files.borrow_mut() = files;
            return;
        }
        let requestor =
            Requestor::alloc(MainThreadMarker::from(&*ns_window)).set_ivars(RequestorIvars {
                files: RefCell::new(files),
            });
        let requestor: Retained<Requestor> = unsafe { msg_send![super(requestor), init] };
        all.push((Weak::from_retained(&ns_window), requestor));
    });
}

fn requestor_of(window: &AnyObject) -> Option<*mut AnyObject> {
    REQUESTORS.with_borrow(|all| {
        all.iter()
            .find(|(held, _)| {
                held.load()
                    .is_some_and(|held| std::ptr::eq(Retained::as_ptr(&held).cast(), window))
            })
            .filter(|(_, requestor)| !requestor.ivars().files.borrow().is_empty())
            .map(|(_, requestor)| Retained::as_ptr(requestor) as *mut AnyObject)
    })
}

/// Succeeds only the first time: NSWindow's own answer is inherited, not this class's.
fn offer_files(window: &NSWindow) {
    let class: &AnyClass = unsafe { msg_send![window, class] };
    let valid: extern "C" fn(&AnyObject, Sel, *mut NSString, *mut NSString) -> *mut AnyObject =
        valid_requestor;
    let added = unsafe {
        objc2::ffi::class_addMethod(
            class as *const AnyClass as *mut AnyClass,
            sel!(validRequestorForSendType:returnType:),
            std::mem::transmute(valid),
            c"@@:@@".as_ptr(),
        )
    };
    if added.as_bool() {
        let send = NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]);
        NSApplication::sharedApplication(MainThreadMarker::from(window))
            .registerServicesMenuSendTypes_returnTypes(&send, &NSArray::new());
    }
}

extern "C" fn valid_requestor(
    this: &AnyObject,
    _: Sel,
    send: *mut NSString,
    back: *mut NSString,
) -> *mut AnyObject {
    let file_urls = !send.is_null() && unsafe { &*send } == unsafe { NSPasteboardTypeFileURL };
    if file_urls && back.is_null() {
        if let Some(requestor) = requestor_of(this) {
            return requestor;
        }
    }
    unsafe {
        msg_send![super(this, NSWindow::class()), validRequestorForSendType: send, returnType: back]
    }
}
