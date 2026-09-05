//! AppKit window tabs: the bar under the toolbar, `New Tab`, and the actions AppKit routes.

use std::cell::RefCell;

use gpui::{Action, AnyWindowHandle, App, AsyncApp, Window};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{ClassType, msg_send, sel};
use objc2_app_kit::{NSApplication, NSWindow, NSWindowOrderingMode, NSWindowTabbingMode};
use objc2_foundation::MainThreadMarker;

use crate::bridge::appkit::gpui_view;

thread_local! {
    static NEW_TAB: RefCell<Option<(AsyncApp, Box<dyn Action>)>> = const { RefCell::new(None) };
}

fn ns_window_of(window: &Window) -> Retained<NSWindow> {
    gpui_view(window)
        .window()
        .expect("GPUI's view is in its window")
}

/// Show AppKit's own tab bar on this window, grouped by `WindowOptions::tabbing_identifier`.
/// `newWindowForTab:` (File > New Tab, ⌘T) dispatches `action` the way a menu row would.
pub fn window_tabs(window: &Window, action: impl Action, cx: &mut App) {
    let ns_window = ns_window_of(window);
    let mtm = MainThreadMarker::from(&*ns_window);
    NSWindow::setAllowsAutomaticWindowTabbing(true, mtm);
    ns_window.setTabbingMode(NSWindowTabbingMode::Automatic);
    NEW_TAB.with_borrow_mut(|slot| *slot = Some((cx.to_async(), action.boxed_clone())));
    restore_appkit_tabs(&ns_window);
    if let Some(delegate) = NSApplication::sharedApplication(mtm).delegate() {
        let delegate: &AnyObject = delegate.as_ref();
        answer_new_tab(delegate.class());
    }
}

/// Open `tab` beside `window` in its tab bar, selected.
pub fn add_tab(window: AnyWindowHandle, tab: &Window, cx: &mut App) {
    let Ok(host) = window.update(cx, |_, window, _| ns_window_of(window)) else {
        return;
    };
    let tab = ns_window_of(tab);
    host.addTabbedWindow_ordered(&tab, NSWindowOrderingMode::Above);
    tab.makeKeyAndOrderFront(None);
}

/// GPUI's window class collapses AppKit's tab bar to draw its own and swallows the tab
/// actions; a native app wants AppKit's, so its class gets `NSWindow`'s implementations back.
fn restore_appkit_tabs(window: &NSWindow) {
    let base = NSWindow::class();
    let mut class = window.class();
    while let Some(sup) = class.superclass() {
        if sup == base {
            break;
        }
        class = sup;
    }
    for selector in [
        sel!(addTitlebarAccessoryViewController:),
        sel!(moveTabToNewWindow:),
        sel!(mergeAllWindows:),
        sel!(selectNextTab:),
        sel!(selectPreviousTab:),
    ] {
        let (Some(method), Some(original)) = (
            class.instance_method(selector),
            base.instance_method(selector),
        ) else {
            continue;
        };
        unsafe {
            objc2::ffi::method_setImplementation(
                method as *const _ as *mut _,
                original.implementation(),
            );
        }
    }
    if let Some(method) = class.instance_method(sel!(toggleTabBar:)) {
        let imp: extern "C" fn(&AnyObject, Sel, *mut AnyObject) = toggle_tab_bar;
        unsafe {
            objc2::ffi::method_setImplementation(
                method as *const _ as *mut _,
                std::mem::transmute::<_, Imp>(imp),
            );
        }
    }
    answer_new_tab(class);
}

/// GPUI turns automatic tabbing off again whenever a window without a tabbing identifier
/// opens (a panel, say), and AppKit will not raise a lone window's bar while it is off.
extern "C" fn toggle_tab_bar(this: &AnyObject, _: Sel, sender: *mut AnyObject) {
    let mtm = MainThreadMarker::new().expect("AppKit sends toggleTabBar: on the main thread");
    NSWindow::setAllowsAutomaticWindowTabbing(true, mtm);
    let _: () = unsafe { msg_send![super(this, NSWindow::class()), toggleTabBar: sender] };
}

/// Teach a responder class `newWindowForTab:`. A second call is a no-op: the class has it.
fn answer_new_tab(class: &AnyClass) {
    let imp: extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) = new_window_for_tab;
    unsafe {
        objc2::ffi::class_addMethod(
            class as *const AnyClass as *mut AnyClass,
            sel!(newWindowForTab:),
            std::mem::transmute::<_, Imp>(imp),
            c"v@:@".as_ptr(),
        );
    }
}

extern "C" fn new_window_for_tab(_: *mut AnyObject, _: Sel, _: *mut AnyObject) {
    NEW_TAB.with_borrow(|slot| {
        if let Some((cx, action)) = slot {
            let action = action.boxed_clone();
            // Deferred: this fires inside AppKit's event dispatch.
            cx.spawn(async move |cx: &mut AsyncApp| {
                cx.update(|cx| cx.dispatch_action(&*action));
            })
            .detach();
        }
    });
}
