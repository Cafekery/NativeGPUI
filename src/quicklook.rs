use std::cell::RefCell;
use std::path::PathBuf;

use block2::RcBlock;
use gpui::{Bounds, Pixels, Window};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSEvent, NSEventType, NSWindow, NSWindowDelegate};
use objc2_foundation::{
    MainThreadMarker, NSInteger, NSObject, NSPoint, NSRect, NSRunLoop, NSSize, NSString, NSURL,
};
use objc2_quick_look_ui::{
    QLPreviewItem, QLPreviewPanel, QLPreviewPanelDataSource, QLPreviewPanelDelegate,
};

use crate::bridge::appkit::gpui_view;
use crate::system::thumbnails;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuickLookKey {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Default)]
struct State {
    items: Vec<PathBuf>,
    current: usize,
    source: Option<NSRect>,
    on_key: Option<Box<dyn Fn(QuickLookKey)>>,
    sink: Option<Retained<QuickLook>>,
    opening: bool,
    full_screen: bool,
    busy: bool,
    pending: Option<Job>,
    #[cfg(feature = "private-api")]
    markup: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGQuickLook"]
    struct QuickLook;

    unsafe impl NSObjectProtocol for QuickLook {}

    unsafe impl NSWindowDelegate for QuickLook {}

    unsafe impl QLPreviewPanelDataSource for QuickLook {
        #[unsafe(method(numberOfPreviewItemsInPreviewPanel:))]
        fn count(&self, _panel: Option<&QLPreviewPanel>) -> NSInteger {
            STATE.with_borrow(|state| state.items.len() as NSInteger)
        }

        #[unsafe(method_id(previewPanel:previewItemAtIndex:))]
        fn item(
            &self,
            _panel: Option<&QLPreviewPanel>,
            index: NSInteger,
        ) -> Option<Retained<ProtocolObject<dyn QLPreviewItem>>> {
            item_at(index)
        }
    }

    unsafe impl QLPreviewPanelDelegate for QuickLook {
        #[unsafe(method(previewPanel:handleEvent:))]
        fn handle_event(&self, panel: Option<&QLPreviewPanel>, event: Option<&NSEvent>) -> bool {
            handle_key(panel, event)
        }

        #[unsafe(method(previewPanel:sourceFrameOnScreenForPreviewItem:))]
        fn source_frame(
            &self,
            _panel: Option<&QLPreviewPanel>,
            item: Option<&ProtocolObject<dyn QLPreviewItem>>,
        ) -> NSRect {
            showing(item)
                .and_then(|_| STATE.with_borrow(|state| state.source))
                .unwrap_or(NSRect::ZERO)
        }

        #[unsafe(method_id(previewPanel:transitionImageForPreviewItem:contentRect:))]
        fn transition_image(
            &self,
            _panel: Option<&QLPreviewPanel>,
            item: Option<&ProtocolObject<dyn QLPreviewItem>>,
            content_rect: *mut NSRect,
        ) -> Option<Retained<AnyObject>> {
            thumbnail_of(item, content_rect)
        }
    }

    #[cfg(feature = "private-api")]
    impl QuickLook {
        #[unsafe(method(previewPanel:canEditItem:))]
        fn can_edit(
            &self,
            _panel: Option<&QLPreviewPanel>,
            item: Option<&ProtocolObject<dyn QLPreviewItem>>,
        ) -> bool {
            marking_up(item)
        }

        #[unsafe(method(previewPanel:initialEditModeForItem:))]
        fn initial_edit_mode(
            &self,
            _panel: Option<&QLPreviewPanel>,
            item: Option<&ProtocolObject<dyn QLPreviewItem>>,
        ) -> NSInteger {
            const MARKUP: NSInteger = 1;
            if marking_up(item) { MARKUP } else { 0 }
        }
    }
);

fn item_at(index: NSInteger) -> Option<Retained<ProtocolObject<dyn QLPreviewItem>>> {
    let path = STATE.with_borrow(|state| state.items.get(index as usize).cloned())?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?));
    Some(ProtocolObject::from_retained(url))
}

fn showing(item: Option<&ProtocolObject<dyn QLPreviewItem>>) -> Option<PathBuf> {
    let path = PathBuf::from(unsafe { item?.previewItemURL()?.path()? }.to_string());
    STATE
        .with_borrow(|state| state.items.get(state.current) == Some(&path))
        .then_some(path)
}

#[cfg(feature = "private-api")]
fn marking_up(item: Option<&ProtocolObject<dyn QLPreviewItem>>) -> bool {
    STATE.with_borrow(|state| state.markup) && showing(item).is_some()
}

fn thumbnail_of(
    item: Option<&ProtocolObject<dyn QLPreviewItem>>,
    content_rect: *mut NSRect,
) -> Option<Retained<AnyObject>> {
    let path = showing(item)?;
    let source = STATE.with_borrow(|state| state.source)?;
    let image = thumbnails::cached_image(&path)?;
    let raster = image.size();
    let zoom =
        (source.size.width / raster.width.max(1.)).min(source.size.height / raster.height.max(1.));
    let size = NSSize::new(raster.width * zoom, raster.height * zoom);
    image.setSize(size);
    if !content_rect.is_null() {
        unsafe { *content_rect = NSRect::new(NSPoint::ZERO, size) };
    }
    Some(Retained::into_super(Retained::into_super(image)))
}

fn handle_key(panel: Option<&QLPreviewPanel>, event: Option<&NSEvent>) -> bool {
    const SPACE: u16 = 49;
    let Some(event) = event.filter(|event| event.r#type() == NSEventType::KeyDown) else {
        return false;
    };
    let key = match event.keyCode() {
        SPACE => {
            if let Some(panel) = panel {
                panel.close();
            }
            return true;
        }
        126 => QuickLookKey::Up,
        125 => QuickLookKey::Down,
        123 => QuickLookKey::Left,
        124 => QuickLookKey::Right,
        _ => return false,
    };
    if STATE.with_borrow(|state| state.items.len() > 1) {
        return false;
    }
    if let Some(handler) = STATE.with_borrow_mut(|state| state.on_key.take()) {
        handler(key);
        STATE.with_borrow_mut(|state| {
            state.on_key.get_or_insert(handler);
        });
    }
    true
}

fn ns_window_of(window: &Window) -> Retained<NSWindow> {
    gpui_view(window)
        .window()
        .expect("GPUI's view is in its window")
}

fn screen_rect(window: &Window, bounds: Bounds<Pixels>) -> NSRect {
    let ns_window = ns_window_of(window);
    let frame = ns_window.frame();
    let height = ns_window
        .contentView()
        .map_or(frame.size.height, |view| view.frame().size.height);
    let (x, y) = (f64::from(bounds.origin.x), f64::from(bounds.origin.y));
    let (w, h) = (f64::from(bounds.size.width), f64::from(bounds.size.height));
    NSRect::new(
        NSPoint::new(frame.origin.x + x, frame.origin.y + height - y - h),
        NSSize::new(w, h),
    )
}

fn panel(mtm: MainThreadMarker) -> Retained<QLPreviewPanel> {
    unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) }
        .expect("QuickLookUI always has its shared panel")
}

/// With a single item, the panel's arrow keys go to `on_key`.
pub fn quick_look(
    window: &Window,
    items: Vec<PathBuf>,
    current: usize,
    source: Option<Bounds<Pixels>>,
    on_key: impl Fn(QuickLookKey) + 'static,
) {
    let ns_window = ns_window_of(window);
    let mtm = MainThreadMarker::from(&*ns_window);
    let source = source.map(|bounds| screen_rect(window, bounds));
    STATE.with_borrow_mut(|state| {
        state.items = items;
        state.current = current;
        state.source = source;
        state.on_key = Some(Box::new(on_key));
        state.opening = true;
        state.full_screen = false;
        #[cfg(feature = "private-api")]
        {
            state.markup = false;
        }
        state
            .sink
            .get_or_insert_with(|| unsafe { msg_send![QuickLook::alloc(mtm), init] });
    });
    answer_panel_control(ns_window.class());
    defer(panel(mtm), Job::Open);
}

pub fn quick_look_full_screen(
    window: &Window,
    items: Vec<PathBuf>,
    current: usize,
    on_key: impl Fn(QuickLookKey) + 'static,
) {
    quick_look(window, items, current, None, on_key);
    STATE.with_borrow_mut(|state| state.full_screen = true);
}

/// Opens `item` in Quick Look's Markup, which saves into the file.
#[cfg(feature = "private-api")]
pub fn quick_look_markup(window: &Window, item: PathBuf) {
    quick_look(window, vec![item], 0, None, |_| {});
    STATE.with_borrow_mut(|state| state.markup = true);
}

pub fn quick_look_update(
    window: &Window,
    items: Vec<PathBuf>,
    current: usize,
    source: Option<Bounds<Pixels>>,
) {
    if !quick_look_visible() {
        return;
    }
    let source = source.map(|bounds| screen_rect(window, bounds));
    STATE.with_borrow_mut(|state| {
        state.items = items;
        state.current = current;
        state.source = source;
    });
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    defer(panel(mtm), Job::Reload);
}

pub fn quick_look_close() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if !quick_look_visible() {
        return;
    }
    STATE.with_borrow_mut(|state| state.opening = false);
    defer(panel(mtm), Job::Close);
}

pub fn quick_look_visible() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    if STATE.with_borrow(|state| state.opening) {
        return true;
    }
    let exists = unsafe { QLPreviewPanel::sharedPreviewPanelExists(mtm) };
    exists && panel(mtm).isVisible()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Job {
    Open,
    Reload,
    Close,
}

/// Loading the panel spins a nested run loop that runs GPUI's tasks and cannot re-enter the main
/// dispatch queue, so it opens from a top-level run-loop block or waits out a half-second timeout.
fn defer(panel: Retained<QLPreviewPanel>, job: Job) {
    let block = RcBlock::new(move || run(&panel, job));
    unsafe { NSRunLoop::mainRunLoop().performBlock(&block) };
}

fn run(panel: &QLPreviewPanel, job: Job) {
    let queued = STATE.with_borrow_mut(|state| {
        if state.busy {
            state.pending = Some(job);
            return true;
        }
        state.busy = true;
        false
    });
    if queued {
        return;
    }
    let mut next = Some(job);
    while let Some(job) = next {
        match job {
            Job::Close => {
                if panel.isVisible() {
                    panel.close();
                }
            }
            Job::Open | Job::Reload => {
                if job == Job::Open || panel.isVisible() {
                    let current = STATE.with_borrow(|state| state.current);
                    attach(panel);
                    unsafe {
                        panel.reloadData();
                        panel.setCurrentPreviewItemIndex(current as NSInteger);
                    }
                }
                if job == Job::Open {
                    unsafe { panel.updateController() };
                    panel.makeKeyAndOrderFront(None);
                    if STATE.with_borrow(|state| state.full_screen) {
                        unsafe { panel.enterFullScreenMode_withOptions(None, None) };
                    }
                }
            }
        }
        next = STATE.with_borrow_mut(|state| {
            state.opening = false;
            state.pending.take()
        });
    }
    STATE.with_borrow_mut(|state| state.busy = false);
}

fn attach(panel: &QLPreviewPanel) {
    let Some(sink) = STATE.with_borrow(|state| state.sink.clone()) else {
        return;
    };
    let sink: &QuickLook = &sink;
    unsafe {
        panel.setDataSource(Some(ProtocolObject::from_ref(sink)));
        panel.setDelegate(Some(sink.as_ref()));
    }
}

/// The panel looks for its controller along the key window's responder chain.
fn answer_panel_control(class: &AnyClass) {
    if class
        .instance_method(sel!(acceptsPreviewPanelControl:))
        .is_some()
    {
        return;
    }
    let accepts: extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> Bool = accepts_control;
    let begin: extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) = begin_control;
    let end: extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) = end_control;
    unsafe {
        let class = class as *const AnyClass as *mut AnyClass;
        objc2::ffi::class_addMethod(
            class,
            sel!(acceptsPreviewPanelControl:),
            std::mem::transmute(accepts),
            c"B@:@".as_ptr(),
        );
        objc2::ffi::class_addMethod(
            class,
            sel!(beginPreviewPanelControl:),
            std::mem::transmute(begin),
            c"v@:@".as_ptr(),
        );
        objc2::ffi::class_addMethod(
            class,
            sel!(endPreviewPanelControl:),
            std::mem::transmute(end),
            c"v@:@".as_ptr(),
        );
    }
}

extern "C" fn accepts_control(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> Bool {
    Bool::new(STATE.with_borrow(|state| !state.items.is_empty()))
}

extern "C" fn begin_control(_: *mut AnyObject, _: Sel, panel: *mut AnyObject) {
    if let Some(panel) = unsafe { panel.cast::<QLPreviewPanel>().as_ref() } {
        attach(panel);
    }
}

extern "C" fn end_control(_: *mut AnyObject, _: Sel, panel: *mut AnyObject) {
    let Some(panel) = (unsafe { panel.cast::<QLPreviewPanel>().as_ref() }) else {
        return;
    };
    unsafe {
        panel.setDataSource(None);
        panel.setDelegate(None);
    }
}
