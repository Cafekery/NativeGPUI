//! The transient surfaces: popovers, sheets and the system share picker.

use std::cell::{Cell, RefCell};
use std::panic::Location;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use gpui::{
    AnyElement, App, AsyncWindowContext, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Size, Window, px,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSControl, NSModalResponse, NSPopover, NSPopoverBehavior, NSPopoverDelegate, NSSharingService,
    NSSharingServicePicker, NSSharingServicePickerDelegate, NSView, NSViewController, NSWindow,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect,
    NSRectEdge, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSTimer, NSURL,
};

use crate::bridge::appkit::{Container, gpui_view};
use crate::bridge::host::{Layer, NativeHost};
use crate::chrome::window_chrome::{chrome_host, native_rect};

/// The painted bounds of a gpui element, kept for a surface to point at.
#[derive(Clone, Default)]
pub struct SurfaceAnchor(Rc<AnchorState>);

#[derive(Default)]
struct AnchorState {
    bounds: Cell<Option<Bounds<Pixels>>>,
    tracker: RefCell<Option<Retained<Container>>>,
}

impl SurfaceAnchor {
    pub fn new() -> Self {
        Self::default()
    }

    /// `None` until the anchoring element has painted.
    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.0.bounds.get()
    }

    fn tracker(&self) -> Option<Retained<Container>> {
        self.0.tracker.borrow().clone()
    }

    fn key(&self) -> u64 {
        Rc::as_ptr(&self.0) as u64
    }
}

/// Record `child`'s painted bounds into `anchor`, so a surface can point at it.
pub fn native_anchor(anchor: &SurfaceAnchor, child: impl IntoElement) -> NativeAnchor {
    NativeAnchor {
        anchor: anchor.clone(),
        child: child.into_any_element(),
    }
}

pub struct NativeAnchor {
    anchor: SurfaceAnchor,
    child: AnyElement,
}

impl Element for NativeAnchor {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.anchor.0.bounds.set(Some(bounds));

        let key = self.anchor.key();
        let mask = window.content_mask().bounds;
        let tracker =
            NativeHost::get(window, cx).place(key, Layer::Above, bounds, mask, |mtm, _| {
                Container::new(mtm, NSRect::ZERO, false)
            });
        self.anchor.0.tracker.replace(Some(tracker));

        self.child.paint(window, cx);
    }
}

impl IntoElement for NativeAnchor {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Which side of the anchor a surface appears on, named as you see it on screen.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum SurfaceEdge {
    #[default]
    Above,
    Below,
    Left,
    Right,
}

impl SurfaceEdge {
    fn rect_edge(self) -> NSRectEdge {
        // MinX = 0, MinY = 1, MaxX = 2, MaxY = 3, and every positioning view here is flipped.
        NSRectEdge(match self {
            Self::Left => 0,
            Self::Above => 1,
            Self::Right => 2,
            Self::Below => 3,
        })
    }
}

#[doc(hidden)]
pub struct ActionIvars {
    handlers: RefCell<Vec<Rc<dyn Fn(&mut Window, &mut App)>>>,
    cx: AsyncWindowContext,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSurfaceActions"]
    /// The action sink for controls a surface owns; `setTarget:` is weak, so the surface retains it.
    #[ivars = ActionIvars]
    pub struct SurfaceActions;

    impl SurfaceActions {
        #[unsafe(method(ngSurfaceAction:))]
        fn ng_surface_action(&self, sender: *mut AnyObject) {
            let ivars = self.ivars();
            let tag: isize = unsafe { msg_send![sender, tag] };
            let Some(handler) = usize::try_from(tag)
                .ok()
                .and_then(|tag| ivars.handlers.borrow().get(tag).cloned())
            else {
                return;
            };
            // Deferred: an action can fire while gpui holds its `App` borrow.
            ivars
                .cx
                .spawn(async move |cx| {
                    let _ = cx.update(|window, app| handler(window, app));
                })
                .detach();
        }
    }
);

impl SurfaceActions {
    fn new(window: &mut Window, cx: &mut App) -> Retained<Self> {
        let mtm = MainThreadMarker::new().expect("gpui runs its windows on the main thread");
        let this = Self::alloc(mtm).set_ivars(ActionIvars {
            handlers: RefCell::new(Vec::new()),
            cx: window.to_async(cx),
        });
        unsafe { msg_send![super(this), init] }
    }

    /// Fire `handler` when `control` acts; uses the control's `tag`, so do not set your own.
    pub fn on_action(
        &self,
        control: &NSControl,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) {
        let tag = {
            let mut handlers = self.ivars().handlers.borrow_mut();
            handlers.push(Rc::new(handler));
            handlers.len() - 1
        };
        control.setTag(tag as isize);
        unsafe {
            control.setTarget(Some(self.as_ref()));
            control.setAction(Some(sel!(ngSurfaceAction:)));
        }
    }
}

/// How a popover decides to close.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum PopoverBehavior {
    #[default]
    ApplicationDefined,
    Transient,
    Semitransient,
}

impl From<PopoverBehavior> for NSPopoverBehavior {
    fn from(behavior: PopoverBehavior) -> Self {
        match behavior {
            PopoverBehavior::ApplicationDefined => NSPopoverBehavior::ApplicationDefined,
            PopoverBehavior::Transient => NSPopoverBehavior::Transient,
            PopoverBehavior::Semitransient => NSPopoverBehavior::Semitransient,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct PopoverOptions {
    pub size: Size<Pixels>,
    pub edge: SurfaceEdge,
    pub behavior: PopoverBehavior,
    pub animates: bool,
}

impl Default for PopoverOptions {
    fn default() -> Self {
        Self {
            size: gpui::size(px(240.), px(110.)),
            edge: SurfaceEdge::Above,
            behavior: PopoverBehavior::default(),
            animates: true,
        }
    }
}

/// A live popover. Dropping this does **not** close it; call [`NativePopover::close`].
pub struct NativePopover {
    popover: Retained<NSPopover>,
    // `setTarget:` and `setDelegate:` are both weak.
    _actions: Retained<SurfaceActions>,
    _delegate: Option<Retained<PopoverDelegate>>,
}

struct DelegateIvars {
    on_close: Rc<dyn Fn(&mut Window, &mut App)>,
    cx: AsyncWindowContext,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGPopoverDelegate"]
    #[ivars = DelegateIvars]
    struct PopoverDelegate;

    unsafe impl NSObjectProtocol for PopoverDelegate {}

    unsafe impl NSPopoverDelegate for PopoverDelegate {
        #[unsafe(method(popoverDidClose:))]
        fn popover_did_close(&self, _: &NSNotification) {
            let ivars = self.ivars();
            let on_close = ivars.on_close.clone();
            ivars
                .cx
                .spawn(async move |cx| {
                    let _ = cx.update(|window, app| on_close(window, app));
                })
                .detach();
        }
    }
);

/// Show a popover pointing at `anchor`, or `None` if `anchor` has never painted.
pub fn present_popover(
    window: &mut Window,
    cx: &mut App,
    anchor: &SurfaceAnchor,
    spec: PopoverOptions,
    build: impl FnOnce(MainThreadMarker, &SurfaceActions) -> Retained<NSView>,
) -> Option<NativePopover> {
    let positioning = anchor.tracker()?;
    let mtm = MainThreadMarker::new().expect("gpui runs its windows on the main thread");
    let actions = SurfaceActions::new(window, cx);
    let content_size = NSSize::new(spec.size.width.to_f64(), spec.size.height.to_f64());

    let content = build(mtm, &actions);
    content.setFrame(NSRect::new(NSPoint::ZERO, content_size));
    let controller = NSViewController::new(mtm);
    controller.setView(&content);

    let popover = NSPopover::new(mtm);
    popover.setContentViewController(Some(&controller));
    popover.setContentSize(content_size);
    popover.setBehavior(spec.behavior.into());
    popover.setAnimates(spec.animates);
    popover.showRelativeToRect_ofView_preferredEdge(
        positioning.bounds(),
        &positioning,
        spec.edge.rect_edge(),
    );

    Some(NativePopover {
        popover,
        _actions: actions,
        _delegate: None,
    })
}

impl NativePopover {
    /// Run `on_close` when *AppKit* closes the popover; a close you asked for does not call back.
    pub fn on_close(
        mut self,
        window: &mut Window,
        cx: &mut App,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        let mtm = MainThreadMarker::new().expect("gpui runs its windows on the main thread");
        let delegate: Retained<PopoverDelegate> = {
            let this = PopoverDelegate::alloc(mtm).set_ivars(DelegateIvars {
                on_close: Rc::new(on_close),
                cx: window.to_async(cx),
            });
            unsafe { msg_send![super(this), init] }
        };
        self.popover
            .setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        self._delegate = Some(delegate);
        self
    }

    pub fn close(&self) {
        // `popoverDidClose:` arrives a turn late, after the next popover has taken this one's slot.
        self.popover.setDelegate(None);
        self.popover.close();
    }

    pub fn is_shown(&self) -> bool {
        self.popover.isShown()
    }

    pub fn window(&self) -> Option<Retained<NSWindow>> {
        self.popover
            .contentViewController()
            .and_then(|controller| controller.view().window())
    }
}

/// The `NSModalResponse` a sheet ended with; anything but `OK` or `CANCEL` is your own value.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct SheetResponse(pub isize);

impl SheetResponse {
    pub const OK: Self = Self(1);
    pub const CANCEL: Self = Self(0);
}

pub struct NativeSheet {
    sheet: Retained<NSWindow>,
    parent: Retained<NSWindow>,
    _actions: Retained<SurfaceActions>,
}

pub fn present_sheet(
    window: &mut Window,
    cx: &mut App,
    build: impl FnOnce(MainThreadMarker, &SurfaceActions) -> Retained<NSWindow>,
    on_end: impl Fn(SheetResponse, &mut Window, &mut App) + 'static,
) -> NativeSheet {
    let mtm = MainThreadMarker::new().expect("gpui runs its windows on the main thread");
    let parent = gpui_view(window)
        .window()
        .expect("gpui's view is always in a window");
    let actions = SurfaceActions::new(window, cx);
    let sheet = build(mtm, &actions);

    let async_cx = window.to_async(cx);
    let on_end = Rc::new(on_end);
    let completion = RcBlock::new(move |response: NSModalResponse| {
        let on_end = on_end.clone();
        async_cx
            .spawn(async move |cx| {
                let _ = cx.update(|window, app| on_end(SheetResponse(response), window, app));
            })
            .detach();
    });
    parent.beginSheet_completionHandler(&sheet, Some(&completion));

    NativeSheet {
        sheet,
        parent,
        _actions: actions,
    }
}

impl NativeSheet {
    /// End the sheet with `response`, which is what `on_end` receives.
    pub fn dismiss(&self, response: SheetResponse) {
        self.parent.endSheet_returnCode(&self.sheet, response.0);
    }

    pub fn is_attached(&self) -> bool {
        self.parent
            .attachedSheet()
            .is_some_and(|attached| attached == self.sheet)
    }

    pub fn window(&self) -> &NSWindow {
        &self.sheet
    }
}

#[derive(Clone, Debug)]
pub enum ShareItem {
    Url(String),
    Text(String),
}

fn share_array(items: &[ShareItem]) -> Retained<NSArray> {
    let objects: Vec<Retained<NSObject>> = items
        .iter()
        .filter_map(|item| match item {
            ShareItem::Url(url) => {
                NSURL::URLWithString(&NSString::from_str(url)).map(Retained::into_super)
            }
            ShareItem::Text(text) => Some(Retained::into_super(NSString::from_str(text))),
        })
        .collect();
    let refs: Vec<&AnyObject> = objects.iter().map(|object| object.as_ref()).collect();
    NSArray::from_slice(&refs)
}

/// How many share services macOS currently offers for `items`.
#[allow(deprecated)]
pub fn share_service_count(items: &[ShareItem]) -> usize {
    unsafe { NSSharingService::sharingServicesForItems(&share_array(items)) }.len()
}

#[derive(Copy, Clone, Debug)]
pub struct SharePicker {
    pub edge: SurfaceEdge,
    /// How long before the picker is forced shut; nothing else dismisses an ignored one.
    pub watchdog: Option<Duration>,
}

impl Default for SharePicker {
    fn default() -> Self {
        Self {
            edge: SurfaceEdge::Above,
            watchdog: Some(Duration::from_secs(20)),
        }
    }
}

#[derive(Clone, Debug)]
pub enum SharePickerEvent {
    Opening { services: usize },
    // `showRelativeToRect:` returns with the picker still up; this is not "closed".
    Presented { blocked: Duration },
    Closed { chose: Option<String> },
}

struct PickerIvars {
    on_event: Rc<dyn Fn(SharePickerEvent, &mut Window, &mut App)>,
    // AppKit asks for the service list again as the picker tears down, with an empty proposal.
    announced: Cell<bool>,
    cx: AsyncWindowContext,
}

// `setDelegate:` and AppKit's own reference are both weak, so the picker is kept alive here.
struct LivePicker {
    _picker: Retained<NSSharingServicePicker>,
    delegate: Retained<PickerDelegate>,
    watchdog: Option<Retained<NSTimer>>,
}

thread_local! {
    static OPEN_PICKERS: RefCell<Vec<LivePicker>> = const { RefCell::new(Vec::new()) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSharePickerDelegate"]
    #[ivars = PickerIvars]
    struct PickerDelegate;

    unsafe impl NSObjectProtocol for PickerDelegate {}

    unsafe impl NSSharingServicePickerDelegate for PickerDelegate {
        #[unsafe(method(sharingServicePicker:sharingServicesForItems:proposedSharingServices:))]
        fn services_for_items(
            &self,
            _: &NSSharingServicePicker,
            _: &NSArray,
            proposed: &NSArray<NSSharingService>,
        ) -> *mut NSArray<NSSharingService> {
            if !self.ivars().announced.replace(true) {
                self.emit(SharePickerEvent::Opening {
                    services: proposed.len(),
                });
            }
            Retained::autorelease_return(proposed.retain())
        }

        #[unsafe(method(sharingServicePicker:didChooseSharingService:))]
        fn did_choose(&self, _: &NSSharingServicePicker, service: Option<&NSSharingService>) {
            self.emit(SharePickerEvent::Closed {
                chose: service.map(|service| service.title().to_string()),
            });
            OPEN_PICKERS.with_borrow_mut(|open| {
                open.retain(|live| {
                    if std::ptr::eq(&*live.delegate, self) {
                        if let Some(timer) = &live.watchdog {
                            timer.invalidate();
                        }
                        return false;
                    }
                    true
                });
            });
        }
    }
);

impl PickerDelegate {
    fn emit(&self, event: SharePickerEvent) {
        let ivars = self.ivars();
        let on_event = ivars.on_event.clone();
        ivars
            .cx
            .spawn(async move |cx| {
                let _ = cx.update(|window, app| on_event(event, window, app));
            })
            .detach();
    }
}

pub fn present_share_picker(
    window: &mut Window,
    cx: &mut App,
    anchor: &SurfaceAnchor,
    spec: SharePicker,
    items: Vec<ShareItem>,
    on_event: impl Fn(SharePickerEvent, &mut Window, &mut App) + 'static,
) {
    let Some(bounds) = anchor.bounds() else {
        return;
    };
    let host = chrome_host(window);
    let async_cx = window.to_async(cx);
    let delegate_cx = async_cx.clone();

    // Deferred a turn: the picker's nested tracking loop must not run under gpui's `App` borrow.
    async_cx
        .spawn(async move |_| {
            let mtm =
                MainThreadMarker::new().expect("gpui's foreground executor is the main thread");
            let array = share_array(&items);
            let picker = unsafe {
                NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &array)
            };
            let delegate: Retained<PickerDelegate> = {
                let this = PickerDelegate::alloc(mtm).set_ivars(PickerIvars {
                    on_event: Rc::new(on_event),
                    announced: Cell::new(false),
                    cx: delegate_cx,
                });
                unsafe { msg_send![super(this), init] }
            };
            picker.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

            let watchdog = spec.watchdog.map(|after| arm_dismissal(&picker, after));
            OPEN_PICKERS.with_borrow_mut(|open| {
                open.push(LivePicker {
                    _picker: picker.clone(),
                    delegate: delegate.clone(),
                    watchdog,
                });
            });

            let started = Instant::now();
            picker.showRelativeToRect_ofView_preferredEdge(
                native_rect(bounds),
                &host,
                spec.edge.rect_edge(),
            );
            delegate.emit(SharePickerEvent::Presented {
                blocked: started.elapsed(),
            });
        })
        .detach();
}

// A timer scheduled the ordinary way runs in the default mode only, never in event tracking.
fn arm_dismissal(picker: &NSSharingServicePicker, after: Duration) -> Retained<NSTimer> {
    let picker = picker.retain();
    let block = RcBlock::new(move |_: NonNull<NSTimer>| picker.close());
    let timer =
        unsafe { NSTimer::timerWithTimeInterval_repeats_block(after.as_secs_f64(), false, &block) };
    unsafe { NSRunLoop::currentRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    timer
}
