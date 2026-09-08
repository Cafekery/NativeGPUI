//! Finder's Space: the system Quick Look panel over the app's own files, with its zoom from the
//! item's icon. The app hands over paths and, for the current item, where its icon is.

use std::cell::RefCell;
use std::path::PathBuf;

use block2::{DynBlock, RcBlock};
use gpui::{Bounds, Pixels, Window};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSEvent, NSEventType, NSWindow, NSWindowDelegate};
use objc2_foundation::{
    MainThreadMarker, NSInteger, NSObject, NSPoint, NSRect, NSSize, NSString, NSURL,
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
    /// Screen frame of the current item's icon; the panel zooms out of and back into it.
    source: Option<NSRect>,
    on_key: Option<Box<dyn Fn(QuickLookKey)>>,
    sink: Option<Retained<QuickLook>>,
    /// The panel was asked to open and the deferred job has not reached it yet.
    opening: bool,
    /// A panel job is running, so far inside its own nested run loop.
    busy: bool,
    pending: Option<Job>,
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
            match showing(item) {
                Some(_) => STATE
                    .with_borrow(|state| state.source)
                    .unwrap_or(NSRect::ZERO),
                None => NSRect::ZERO,
            }
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
);

fn item_at(index: NSInteger) -> Option<Retained<ProtocolObject<dyn QLPreviewItem>>> {
    let path = STATE.with_borrow(|state| state.items.get(index as usize).cloned())?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?));
    Some(ProtocolObject::from_retained(url))
}

/// The item the app says the panel is showing, so only that one zooms out of a row.
fn showing(item: Option<&ProtocolObject<dyn QLPreviewItem>>) -> Option<PathBuf> {
    let path = PathBuf::from(unsafe { item?.previewItemURL()?.path()? }.to_string());
    STATE
        .with_borrow(|state| state.items.get(state.current) == Some(&path))
        .then_some(path)
}

/// The raster the row drew, worn at the size it is drawn at: the picture zooms, not a type icon.
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
        unsafe { *content_rect = NSRect::new(NSPoint::new(0., 0.), size) };
    }
    Some(Retained::into_super(Retained::into_super(image)))
}

/// Space closes like Finder's; with a single item the arrows move the app's selection.
fn handle_key(panel: Option<&QLPreviewPanel>, event: Option<&NSEvent>) -> bool {
    let Some(event) = event else {
        return false;
    };
    if event.r#type() != NSEventType::KeyDown {
        return false;
    }
    let key = match event.keyCode() {
        49 => {
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
    let handler = STATE.with_borrow_mut(|state| state.on_key.take());
    if let Some(handler) = handler {
        handler(key);
        STATE.with_borrow_mut(|state| {
            if state.on_key.is_none() {
                state.on_key = Some(handler);
            }
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

/// Opens the panel on `items` (Finder previews the whole selection) showing `current`, zooming out
/// of `source` when the app knows where that item's icon is. With one item, the panel's arrow
/// keys reach `on_key` so the app can move its selection and call [`quick_look_update`].
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
        if state.sink.is_none() {
            let sink: Retained<QuickLook> = unsafe { msg_send![QuickLook::alloc(mtm), init] };
            state.sink = Some(sink);
        }
    });
    answer_panel_control(ns_window.class());
    defer(panel(mtm), Job::Open);
}

/// The selection moved while the panel is up: show the new items without closing.
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

/// The panel loads its item synchronously, in a nested run loop that drains the main queue and so
/// runs GPUI's foreground tasks: every call that can load or lay it out is enqueued instead, to run
/// at the run loop's top level where GPUI holds no borrow of its `App`.
fn defer(panel: Retained<QLPreviewPanel>, job: Job) {
    let block = RcBlock::new(move || run(&panel, job));
    unsafe { dispatch_async(&_dispatch_main_q, &block) };
}

#[repr(C)]
struct DispatchQueue {
    _opaque: [u8; 0],
}

extern "C" {
    static _dispatch_main_q: DispatchQueue;
    fn dispatch_async(queue: &DispatchQueue, block: &DynBlock<dyn Fn()>);
}

/// A job that lands inside another's nested loop waits for it rather than re-entering the panel.
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

/// The panel finds its controller on the key window's responder chain; GPUI's window learns the
/// three methods once.
fn answer_panel_control(class: &AnyClass) {
    if class
        .instance_method(sel!(acceptsPreviewPanelControl:))
        .is_some()
    {
        return;
    }
    let accepts: extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> objc2::runtime::Bool =
        accepts_control;
    let begin: extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) = begin_control;
    let end: extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) = end_control;
    unsafe {
        let class = class as *const AnyClass as *mut AnyClass;
        objc2::ffi::class_addMethod(
            class,
            sel!(acceptsPreviewPanelControl:),
            std::mem::transmute::<
                extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> objc2::runtime::Bool,
                Imp,
            >(accepts),
            c"B@:@".as_ptr(),
        );
        objc2::ffi::class_addMethod(
            class,
            sel!(beginPreviewPanelControl:),
            std::mem::transmute::<extern "C" fn(*mut AnyObject, Sel, *mut AnyObject), Imp>(begin),
            c"v@:@".as_ptr(),
        );
        objc2::ffi::class_addMethod(
            class,
            sel!(endPreviewPanelControl:),
            std::mem::transmute::<extern "C" fn(*mut AnyObject, Sel, *mut AnyObject), Imp>(end),
            c"v@:@".as_ptr(),
        );
    }
}

extern "C" fn accepts_control(
    _: *mut AnyObject,
    _: Sel,
    _: *mut AnyObject,
) -> objc2::runtime::Bool {
    objc2::runtime::Bool::new(STATE.with_borrow(|state| !state.items.is_empty()))
}

extern "C" fn begin_control(_: *mut AnyObject, _: Sel, panel: *mut AnyObject) {
    if panel.is_null() {
        return;
    }
    let panel: &QLPreviewPanel = unsafe { &*panel.cast() };
    attach(panel);
}

extern "C" fn end_control(_: *mut AnyObject, _: Sel, panel: *mut AnyObject) {
    if panel.is_null() {
        return;
    }
    let panel: &QLPreviewPanel = unsafe { &*panel.cast() };
    unsafe {
        panel.setDataSource(None);
        panel.setDelegate(None);
    }
}
