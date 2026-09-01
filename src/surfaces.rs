//! The transient surfaces: popovers, sheets and the system share picker.
//!
//! Each lives in its own `NSWindow` above the GPUI window and is drawn entirely by macOS, which
//! is also the limit on what can go inside one — see [`present_popover`].
//!
//! A popover or a share picker points at a GPUI element whose geometry is only known during
//! `paint`, so wrap it in [`native_anchor`] with a [`SurfaceAnchor`] you own. A popover follows
//! its anchor for as long as it is up — see [`present_popover`]. A share picker is positioned
//! once, through [`crate::native_rect`] and [`crate::chrome_host`], whose coordinate space *is*
//! GPUI's — do not hand-roll `content_height - y - height`.
//!
//! Already native in GPUI, do not wrap again: [`gpui::Window::prompt`] is a real `NSAlert` sheet,
//! and [`gpui::App::prompt_for_paths`] / [`gpui::App::prompt_for_new_path`] are `NSOpenPanel` /
//! `NSSavePanel` presented as independent modal windows, not sheets attached to your window.

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

/// The painted bounds of a GPUI element, kept for a surface to point at. Cheap to clone; every
/// clone sees the same bounds.
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

    /// The hosted view [`NativeAnchor`] keeps pinned to the element, or `None` before its first
    /// paint.
    fn tracker(&self) -> Option<Retained<Container>> {
        self.0.tracker.borrow().clone()
    }

    /// This anchor's key in the window's view pool. [`NativeAnchor`] has no `ElementId` to derive
    /// one from, and needs none: the anchor *is* the caller-owned identity of the element it
    /// wraps, so its allocation is unique among live anchors and stable for as long as one lives.
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
        // Unrounded: a popover centred on a rounded rect is off by up to half a point.
        self.anchor.0.bounds.set(Some(bounds));

        // An `NSPopover` is positioned once and then tracks its positioning view, so the anchor
        // keeps a hosted view pinned to the element for a popover to point at. Going through the
        // pool is what makes the popover follow a scroll: `place` writes the frame gpui has
        // already presented, so the popover lands on the anchor as drawn rather than up to two
        // frames ahead of it.
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
///
/// `NSRectEdge` is interpreted in the *positioning view's* space, and every positioning view this
/// crate uses is flipped, so `MinY` is the anchor's **top** edge and the surface renders above it.
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
        // NSRectEdge: MinX = 0, MinY = 1, MaxX = 2, MaxY = 3.
        NSRectEdge(match self {
            Self::Left => 0,
            Self::Above => 1,
            Self::Right => 2,
            Self::Below => 3,
        })
    }
}

/// `SurfaceActions`' storage. Public only because `define_class!` makes a class's ivars type part
/// of its public interface; there is nothing here to reach.
#[doc(hidden)]
pub struct ActionIvars {
    handlers: RefCell<Vec<Rc<dyn Fn(&mut Window, &mut App)>>>,
    cx: AsyncWindowContext,
}

define_class!(
    /// The action sink for controls a *surface* owns: a button inside a popover or a sheet has no
    /// GPUI element, so it cannot go through [`crate::NativeHost`]'s element-keyed target.
    /// `setTarget:` is weak, so the surface retains this for as long as its controls exist.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSurfaceActions"]
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
            // Deferred: an action can fire while gpui holds its `App` borrow, which would drop a
            // synchronous update.
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

    /// Fire `handler` when `control` acts, reading whatever you need off the control inside it.
    /// Uses the control's `tag`, so do not set your own.
    ///
    /// A *pooled* element must use [`crate::NativeHost::on_event`] instead: GPUI's `paint` can
    /// write its control between the action firing and this deferred handler running.
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
    /// Closes only on [`NativePopover::close`]. The default, because `Transient` self-dismisses
    /// on the first stray click and your GPUI state then disagrees with the screen.
    #[default]
    ApplicationDefined,
    /// AppKit closes it on the next click outside, so wire `on_close` or your `open` flag desyncs.
    Transient,
    /// Like `Transient`, but survives clicks inside your own window.
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

/// What a popover should look like. `..Default::default()` for everything you do not care about.
#[derive(Copy, Clone, Debug)]
pub struct Popover {
    pub size: Size<Pixels>,
    pub edge: SurfaceEdge,
    pub behavior: PopoverBehavior,
    pub animates: bool,
}

impl Default for Popover {
    fn default() -> Self {
        Self {
            size: gpui::size(px(240.), px(110.)),
            edge: SurfaceEdge::Above,
            behavior: PopoverBehavior::default(),
            animates: true,
        }
    }
}

/// A live popover. Dropping this does **not** close it — call [`NativePopover::close`], or hold
/// it in your view state for as long as the popover is up.
pub struct NativePopover {
    popover: Retained<NSPopover>,
    /// `setTarget:` and `setDelegate:` are both weak.
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
///
/// The popover **follows the anchor**: it is positioned against the hosted view
/// [`native_anchor`] keeps pinned to the element, and `NSPopover` re-anchors itself whenever that
/// view's position in the window changes — so a scroll takes the popover with it. Positioning it
/// against a rect in [`crate::chrome_host`] instead reads correctly and is wrong the moment the
/// page moves: the host never moves, so the popover stays put while the element leaves from under
/// it. `positioningRect` is no help either; on a shown popover, writing it moves nothing.
///
/// `build` receives the surface's [`SurfaceActions`] so its controls can drive GPUI state, and the
/// view it returns becomes the content, sized to `spec.size`. That content must be AppKit: it goes
/// in a new `_NSPopoverWindow`, and gpui renders a window's whole scene into the one Metal view
/// `MacWindow` created.
pub fn present_popover(
    window: &mut Window,
    cx: &mut App,
    anchor: &SurfaceAnchor,
    spec: Popover,
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
    /// Run `on_close` when *AppKit* closes the popover — a [`PopoverBehavior::Transient`]
    /// dismissal, or the positioning view leaving the window, which is why it is worth wiring even
    /// with [`PopoverBehavior::ApplicationDefined`]. A close you asked for does not call back; see
    /// [`NativePopover::close`].
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

    /// Close the popover.
    ///
    /// `on_close` does **not** fire for a close you asked for. It arrives a main-loop turn later,
    /// so on the close-then-open path — every "open a popover somewhere else" — the outgoing
    /// popover's callback lands *after* the incoming one is stored and clears it. Dropping that
    /// handle releases the only strong reference to the surface's [`SurfaceActions`], and
    /// `setTarget:` is weak, so the popover still on screen has controls that reach nothing and
    /// no longer closes at all.
    pub fn close(&self) {
        self.popover.setDelegate(None);
        self.popover.close();
    }

    pub fn is_shown(&self) -> bool {
        self.popover.isShown()
    }

    /// The popover's own `NSWindow`, with its own screen rect. It is a *child* of the GPUI
    /// window, so a capture scoped to that window composites the popover in anyway — the separate
    /// window number is not a separate capture.
    pub fn window(&self) -> Option<Retained<NSWindow>> {
        self.popover
            .contentViewController()
            .and_then(|controller| controller.view().window())
    }
}

/// The `NSModalResponse` a sheet ended with. `OK` and `CANCEL` are AppKit's own values; any other
/// is whatever you passed to [`NativeSheet::dismiss`], so a three-button sheet needs no plumbing.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct SheetResponse(pub isize);

impl SheetResponse {
    pub const OK: Self = Self(1);
    pub const CANCEL: Self = Self(0);
}

/// A sheet attached to the GPUI window.
pub struct NativeSheet {
    sheet: Retained<NSWindow>,
    parent: Retained<NSWindow>,
    _actions: Retained<SurfaceActions>,
}

/// Present `build`'s window as a sheet on the GPUI window, and call `on_end`, deferred onto GPUI,
/// when it ends: AppKit's completion handler fires while GPUI may hold its `App` borrow.
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

/// Something to hand to the share picker. A URL string AppKit refuses to parse is dropped, and an
/// empty item list offers no services — which is what [`share_service_count`] shows you.
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

/// How many share services macOS currently offers for `items`. Check before showing a picker: an
/// empty item list produces zero services, and a picker that opened empty still looks fine.
///
/// Deprecated in favour of `standardShareMenuItem`, which never says how many services are behind
/// it, so there is no non-deprecated way to ask ahead of time.
#[allow(deprecated)]
pub fn share_service_count(items: &[ShareItem]) -> usize {
    unsafe { NSSharingService::sharingServicesForItems(&share_array(items)) }.len()
}

/// What to do with the picker.
#[derive(Copy, Clone, Debug)]
pub struct SharePicker {
    pub edge: SurfaceEdge,
    /// How long before the picker is forced shut. **Keep this set:** a picker nobody dismisses
    /// stays up forever, and where `showRelativeToRect:ofView:preferredEdge:` runs a nested
    /// tracking loop it takes the whole app with it.
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

/// What the picker did.
#[derive(Clone, Debug)]
pub enum SharePickerEvent {
    /// The picker has built its list and is about to show it; zero `services` means empty.
    Opening { services: usize },
    /// `showRelativeToRect:ofView:preferredEdge:` returned, having held the main thread for
    /// `blocked`. **Not** "the picker closed": on current macOS the call returns immediately with
    /// the picker still up, while earlier releases ran a nested tracking loop until dismissal.
    Presented { blocked: Duration },
    /// The picker is gone, reported by its own delegate. `chose` is the title of the service the
    /// user picked, or `None` if it was dismissed — which is what the watchdog produces.
    Closed { chose: Option<String> },
}

struct PickerIvars {
    on_event: Rc<dyn Fn(SharePickerEvent, &mut Window, &mut App)>,
    /// AppKit asks for the service list a second time as the picker tears down, with an empty
    /// proposal — reporting it would claim "0 services" about a picker that showed four.
    announced: Cell<bool>,
    cx: AsyncWindowContext,
}

/// Holds picker, delegate and watchdog together while the picker is up: `setDelegate:` and
/// AppKit's own reference are both too weak, and without this the delegate dies moments after the
/// picker appears, so `didChooseSharingService:` never arrives and the watchdog never disarms.
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
        // Returns the proposal unchanged; this delegate exists to *count* it. `Retained` is not an
        // encodable return type outside the `new`/`init`/`copy` families, so it autoreleases.
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

        // A nil service means dismissed without a choice, which is what the watchdog produces.
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

/// Open the system share picker on `anchor`, sharing `items`.
///
/// Presentation is **deferred by one main-loop turn on purpose:** where the picker runs a nested
/// tracking loop, showing it from a GPUI event handler would run that loop while GPUI holds its
/// `App` borrow, and any GPUI work reached from it then panics.
///
/// **It will not open for a background app:** `Opening` still arrives and the call still returns,
/// but no window is created — a [`SharePickerEvent::Closed`] that never comes is the symptom.
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

            // Armed before showing and disarmed by the delegate: if the picker blocks, this thread
            // reaches no further statement of ours until it is gone.
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

/// Schedule the thing that gets rid of a picker nobody is going to dismiss. `close` delivers
/// `didChooseSharingService:` with a nil service, so there is no synthesised Escape and no
/// guessing which window is the picker's.
///
/// `NSRunLoopCommonModes` is load-bearing: it includes `NSEventTrackingRunLoopMode`, and a timer
/// scheduled the ordinary way runs in the default mode only — where the picker blocks, it would
/// never fire during the loop it exists to break.
fn arm_dismissal(picker: &NSSharingServicePicker, after: Duration) -> Retained<NSTimer> {
    let picker = picker.retain();
    let block = RcBlock::new(move |_: NonNull<NSTimer>| picker.close());
    let timer =
        unsafe { NSTimer::timerWithTimeInterval_repeats_block(after.as_secs_f64(), false, &block) };
    unsafe { NSRunLoop::currentRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    timer
}
