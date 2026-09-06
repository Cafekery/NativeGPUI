//! AppKit window tabs: the bar under the toolbar, `New Tab`, and the actions AppKit routes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CStr;

use gpui::{Action, AnyWindowHandle, App, AsyncApp, Window};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Imp, Sel};
use objc2::{ClassType, msg_send, sel};
use objc2_app_kit::{NSApplication, NSView, NSWindow, NSWindowOrderingMode, NSWindowTabbingMode};
use objc2_foundation::{MainThreadMarker, NSSize};

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
    teach_new_tab(mtm);
}

/// Stop the tab bar `points` short of the window's trailing edge, so it ends where a full-height
/// pane begins and carries its New Tab button there, as Finder's bar ends at the preview pane.
/// Cheap to call every frame; it takes effect once the bar exists.
pub fn set_trailing_inset(window: &Window, points: f32) {
    let Some(accessory) = tab_bar_accessory(&ns_window_of(window)) else {
        return;
    };
    adopt_inset(&accessory);
    let inset = f64::from(points.max(0.));
    let key = Retained::as_ptr(&accessory) as usize;
    let changed = INSET.with_borrow_mut(|insets| insets.insert(key, inset) != Some(inset));
    if changed {
        accessory.setNeedsLayout(true);
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

/// AppKit decides once, when it first builds a window's tab bar, whether that bar carries the
/// New Tab button, and gpui builds the bar while it creates the window: nothing the responder
/// chain learns afterwards brings the button back, so the app delegate answers from launch.
pub(crate) fn teach_new_tab(mtm: MainThreadMarker) {
    if let Some(delegate) = NSApplication::sharedApplication(mtm).delegate() {
        let delegate: &AnyObject = delegate.as_ref();
        answer_new_tab(delegate.class());
    }
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

thread_local! {
    static INSET: RefCell<HashMap<usize, f64>> = RefCell::new(HashMap::new());
}

const INSET_CLASS: &CStr = c"NGTabBarInsetView";

/// The titlebar accessory AppKit gives the tab bar, if this window is showing one.
fn tab_bar_accessory(window: &NSWindow) -> Option<Retained<NSView>> {
    window
        .titlebarAccessoryViewControllers()
        .iter()
        .map(|controller| controller.view())
        .find(|view| tab_bar(view).is_some())
}

fn tab_bar(accessory: &NSView) -> Option<Retained<NSView>> {
    accessory
        .subviews()
        .iter()
        .find(|view| view.class().name() == c"NSTabBar")
        .map(Retained::from)
}

/// Subclass the accessory instance so the inset is re-applied after every layout AppKit runs:
/// AppKit sizes the bar to the whole titlebar again on each resize and each tab change.
fn adopt_inset(accessory: &NSView) {
    if accessory.class().name() == INSET_CLASS {
        return;
    }
    let subclass = match AnyClass::get(INSET_CLASS) {
        Some(existing) => existing,
        None => {
            let mut builder = ClassBuilder::new(INSET_CLASS, accessory.class())
                .expect("no other class in this process is named NGTabBarInsetView");
            unsafe {
                builder.add_method(
                    sel!(layout),
                    inset_layout as extern "C" fn(*mut AnyObject, Sel),
                );
                builder.add_method(
                    sel!(resizeSubviewsWithOldSize:),
                    inset_resize as extern "C" fn(*mut AnyObject, Sel, NSSize),
                );
            }
            builder.register()
        }
    };
    unsafe { AnyObject::set_class(&*(accessory as *const NSView).cast::<AnyObject>(), subclass) };
}

fn accessory_imp(selector: Sel) -> Imp {
    AnyClass::get(INSET_CLASS)
        .and_then(AnyClass::superclass)
        .and_then(|base| base.instance_method(selector))
        .expect("NGTabBarInsetView is only ever registered over the accessory's own class")
        .implementation()
}

extern "C" fn inset_layout(view: *mut AnyObject, selector: Sel) {
    let base: extern "C" fn(*mut AnyObject, Sel) =
        unsafe { std::mem::transmute(accessory_imp(selector)) };
    base(view, selector);
    apply_inset(view);
}

extern "C" fn inset_resize(view: *mut AnyObject, selector: Sel, old: NSSize) {
    let base: extern "C" fn(*mut AnyObject, Sel, NSSize) =
        unsafe { std::mem::transmute(accessory_imp(selector)) };
    base(view, selector, old);
    apply_inset(view);
}

fn apply_inset(view: *mut AnyObject) {
    let inset = INSET.with_borrow(|insets| insets.get(&(view as usize)).copied());
    let Some(inset) = inset else {
        return;
    };
    let accessory = unsafe { &*view.cast::<NSView>() };
    let Some(bar) = tab_bar(accessory) else {
        return;
    };
    let mut frame = bar.frame();
    let width = (accessory.bounds().size.width - inset).max(0.);
    if frame.origin.x == 0. && frame.size.width == width {
        return;
    }
    frame.origin.x = 0.;
    frame.size.width = width;
    bar.setFrame(frame);
}
