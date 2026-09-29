use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::marker::PhantomData;
use std::panic::Location;
use std::ptr;
use std::rc::Rc;

use block2::RcBlock;
use gpui::{
    AnyElement, App, AsyncWindowContext, Bounds, CursorStyle, Element, ElementId, FontWeight,
    GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, Pixels,
    Point, SharedString, Size, Window, px,
};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSCursor, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType,
    NSImage, NSLayoutConstraint, NSMenu, NSMenuDelegate, NSMenuItem, NSMenuToolbarItem, NSPopover,
    NSPopoverBehavior, NSSearchField, NSSearchToolbarItem, NSSegmentStyle, NSSegmentSwitchTracking,
    NSSegmentedControl, NSSharingServicePickerDelegate, NSSharingServicePickerToolbarItem,
    NSSharingServicePickerToolbarItemDelegate, NSSplitView, NSSplitViewController, NSTextAlignment,
    NSTextField, NSToolTipTag, NSToolbar, NSToolbarDelegate, NSToolbarDisplayMode,
    NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem, NSToolbarItemGroup,
    NSToolbarItemGroupControlRepresentation, NSToolbarItemGroupSelectionMode,
    NSToolbarItemIdentifier, NSToolbarSpaceItemIdentifier, NSTrackingArea, NSTrackingAreaOptions,
    NSTrackingSeparatorToolbarItem, NSUserInterfaceItemIdentification, NSView, NSViewController,
    NSWindow, NSWindowButton, NSWindowCollectionBehavior, NSWindowLevel, NSWindowOrderingMode,
    NSWindowStyleMask, NSWindowToolbarStyle,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSRectEdge, NSSize,
    NSString, NSUserDefaults,
};

use crate::bridge::appkit::{Container, gpui_view, to_rect};
use crate::bridge::host::element_key;
use crate::controls::menu::{MenuEntry, sync_menu};
use crate::system::wording::worded;
use crate::tokens;

pub fn native_rect(bounds: Bounds<Pixels>) -> NSRect {
    to_rect(bounds)
}

pub fn native_window_rect(host: &NSView, bounds: Bounds<Pixels>) -> NSRect {
    host.convertRect_toView(native_rect(bounds), None)
}

pub fn native_screen_rect(host: &NSView, bounds: Bounds<Pixels>) -> NSRect {
    let window = host
        .window()
        .expect("native_screen_rect: host view is not in a window");
    window.convertRectToScreen(native_window_rect(host, bounds))
}

pub fn chrome_host(window: &Window) -> Retained<NSView> {
    Retained::into_super(chrome(window))
}

#[derive(Copy, Clone, PartialEq)]
struct Region<T> {
    key: u64,
    rect: NSRect,
    value: T,
}

type Regions<T> = RefCell<Vec<Region<T>>>;
type Callout = Region<SharedString>;

const CALLOUT_DELAY: f64 = 0.87;
const CALLOUT_FONT_SIZE: f32 = 14.;
const CALLOUT_INSET_X: f64 = 8.;
const CALLOUT_INSET_Y: f64 = 4.;

#[derive(Clone)]
struct CalloutView {
    popover: Retained<NSPopover>,
    label: Retained<NSTextField>,
}

#[derive(Copy, Clone)]
struct Corner {
    supported: bool,
    system: f64,
    // `_setCornerRadius: 0` restores the system radius and reads back as it, so intent is kept here.
    written: Option<f64>,
}

struct Baseline {
    has_shadow: bool,
    is_opaque: bool,
    alpha: f64,
    movable_by_background: bool,
    titlebar_appears_transparent: bool,
    style: NSWindowStyleMask,
    level: NSWindowLevel,
    collection: NSWindowCollectionBehavior,
    appearance: Option<Retained<NSAppearance>>,
}

struct ChromeIvars {
    claims: Regions<Cursor>,
    tips: Regions<SharedString>,
    callouts: Regions<SharedString>,
    callout: RefCell<Option<CalloutView>>,
    shown: RefCell<Option<Callout>>,
    hovered: Cell<u64>,
    corner: Cell<Corner>,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    cursor_forced: Cell<bool>,
    toolbar: RefCell<Option<Retained<NSToolbar>>>,
    toolbar_delegate: RefCell<Option<Retained<ToolbarDelegate>>>,
    toolbar_installed: RefCell<Option<(ToolbarStyle, Vec<String>)>>,
    baseline: Baseline,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGChrome"]
    #[ivars = ChromeIvars]
    struct ChromeHost;

    impl ChromeHost {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        // `hitTest:` points arrive in the superview's space.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            let hit: *mut NSView = unsafe { msg_send![super(self), hitTest: point] };
            if !ptr::eq(hit.cast_const().cast::<ChromeHost>(), self) {
                return hit;
            }
            let local = self.convertPoint_fromView(point, unsafe { self.superview() }.as_deref());
            if self.claim_at(local).is_some() {
                return (self as *const Self as *mut Self).cast();
            }
            ptr::null_mut()
        }

        #[unsafe(method(cursorUpdate:))]
        fn cursor_update(&self, event: &NSEvent) {
            let local = self.convertPoint_fromView(event.locationInWindow(), None);
            if let Some(claim) = self.claim_at(local) {
                claim.value.ns().set();
            }
        }

        // AppKit resets the cursor after every move in a non-key window, so set it a turn later.
        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, _: &NSEvent) {
            self.hover_callout();
            if self.window().is_some_and(|window| window.isKeyWindow()) {
                return;
            }
            unsafe {
                let _: () = msg_send![
                    self,
                    performSelector: sel!(ngApplyCursor),
                    withObject: std::ptr::null::<AnyObject>(),
                    afterDelay: 0.0,
                ];
            }
        }

        #[unsafe(method(ngApplyCursor))]
        fn apply_cursor(&self) {
            let Some(window) = self.window() else {
                return;
            };
            let local = self.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
            match self.claim_at(local) {
                Some(claim) => {
                    claim.value.ns().set();
                    self.ivars().cursor_forced.set(true);
                }
                None => self.release_cursor(),
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _: &NSEvent) {
            self.hide_callout();
            self.release_cursor();
        }

        #[unsafe(method(ngShowCallout))]
        fn show_hovered_callout(&self) {
            if let Some(callout) = self.pointer().and_then(|point| self.callout_at(point)) {
                self.show_callout(&callout);
            }
        }

        #[unsafe(method_id(view:stringForToolTip:point:userData:))]
        fn string_for_tool_tip(
            &self,
            _: &NSView,
            _: NSToolTipTag,
            point: NSPoint,
            _: *mut c_void,
        ) -> Retained<NSString> {
            let tip = region_at(&self.ivars().tips, point).map(|tip| tip.value);
            NSString::from_str(tip.as_deref().unwrap_or(""))
        }

        // One rect per claim: `cursorUpdate:` alone cannot switch between adjacent regions.
        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            for claim in self.ivars().claims.borrow().iter() {
                self.addCursorRect_cursor(claim.rect, &claim.value.ns());
            }
        }
    }
);

impl ChromeHost {
    fn new(
        mtm: MainThreadMarker,
        frame: NSRect,
        corner: Corner,
        baseline: Baseline,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ChromeIvars {
            claims: RefCell::new(Vec::new()),
            tips: RefCell::new(Vec::new()),
            callouts: RefCell::new(Vec::new()),
            callout: RefCell::new(None),
            shown: RefCell::new(None),
            hovered: Cell::new(0),
            corner: Cell::new(corner),
            tracking: RefCell::new(None),
            cursor_forced: Cell::new(false),
            toolbar: RefCell::new(None),
            toolbar_delegate: RefCell::new(None),
            toolbar_installed: RefCell::new(None),
            baseline,
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        this
    }

    fn release_cursor(&self) {
        if self.ivars().cursor_forced.replace(false) {
            NSCursor::arrowCursor().set();
        }
    }

    fn claim_at(&self, point: NSPoint) -> Option<Region<Cursor>> {
        region_at(&self.ivars().claims, point)
    }

    fn ensure_tracking(&self) {
        let mut slot = self.ivars().tracking.borrow_mut();
        if slot.is_some() {
            return;
        }
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                self.bounds(),
                NSTrackingAreaOptions::InVisibleRect
                    | NSTrackingAreaOptions::CursorUpdate
                    | NSTrackingAreaOptions::MouseMoved
                    | NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::ActiveAlways,
                Some(self.as_ref()),
                None,
            )
        };
        self.addTrackingArea(&area);
        *slot = Some(area);
    }

    fn set_claim(&self, key: u64, rect: NSRect, cursor: Cursor) {
        let claim = Region {
            key,
            rect,
            value: cursor,
        };
        if upsert(&self.ivars().claims, claim) {
            self.ensure_tracking();
            self.invalidate_cursor_rects();
        }
    }

    fn remove_claim(&self, key: u64) {
        if remove_region(&self.ivars().claims, key) {
            self.invalidate_cursor_rects();
        }
    }

    fn set_tip(&self, key: u64, rect: NSRect, text: &SharedString) {
        let tip = Region {
            key,
            rect,
            value: text.clone(),
        };
        if upsert(&self.ivars().tips, tip) {
            self.reset_tool_tips();
        }
    }

    fn remove_tip(&self, key: u64) {
        if remove_region(&self.ivars().tips, key) {
            self.reset_tool_tips();
        }
    }

    fn reset_tool_tips(&self) {
        self.removeAllToolTips();
        for tip in self.ivars().tips.borrow().iter() {
            unsafe { self.addToolTipRect_owner_userData(tip.rect, self.as_ref(), ptr::null_mut()) };
        }
    }

    fn callout_at(&self, point: NSPoint) -> Option<Callout> {
        region_at(&self.ivars().callouts, point)
    }

    fn set_callout(&self, key: u64, rect: NSRect, text: &SharedString) {
        let callout = Region {
            key,
            rect,
            value: text.clone(),
        };
        if upsert(&self.ivars().callouts, callout.clone()) {
            self.ensure_tracking();
            self.follow_callout(&callout);
        }
    }

    fn remove_callout(&self, key: u64) {
        remove_region(&self.ivars().callouts, key);
        if self.shows(key) {
            self.hide_callout();
        }
    }

    fn shows(&self, key: u64) -> bool {
        self.ivars()
            .shown
            .borrow()
            .as_ref()
            .is_some_and(|shown| shown.key == key)
    }

    fn follow_callout(&self, callout: &Callout) {
        if !self.shows(callout.key) {
            return;
        }
        match self.pointer() {
            Some(point) if contains(callout.rect, point) => self.show_callout(callout),
            _ => self.hide_callout(),
        }
    }

    fn pointer(&self) -> Option<NSPoint> {
        let window = self.window()?;
        Some(self.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None))
    }

    fn hover_callout(&self) {
        let hovered = self.pointer().and_then(|point| self.callout_at(point));
        let key = hovered.as_ref().map_or(0, |callout| callout.key);
        if key == self.ivars().hovered.replace(key) {
            return;
        }
        self.cancel_callout();
        let shown = self.ivars().shown.borrow().is_some();
        match hovered {
            None => self.hide_callout(),
            Some(callout) if shown => self.show_callout(&callout),
            Some(_) => unsafe {
                let _: () = msg_send![
                    self,
                    performSelector: sel!(ngShowCallout),
                    withObject: ptr::null::<AnyObject>(),
                    afterDelay: CALLOUT_DELAY,
                ];
            },
        }
    }

    // AppKit calls here can re-enter this host, so no ivar borrow spans one.
    fn show_callout(&self, callout: &Callout) {
        let mtm = MainThreadMarker::new().expect("AppKit tracking is a main-thread path");
        let view = self
            .ivars()
            .callout
            .borrow_mut()
            .get_or_insert_with(|| CalloutView::new(mtm))
            .clone();
        view.set_text(&callout.value);
        if let Some(window) = self.window() {
            view.popover
                .setAppearance(Some(&window.effectiveAppearance()));
        }
        *self.ivars().shown.borrow_mut() = Some(callout.clone());
        view.popover
            .showRelativeToRect_ofView_preferredEdge(callout.rect, self, NSRectEdge::MinY);
        view.disown_mouse();
    }

    fn hide_callout(&self) {
        self.cancel_callout();
        self.ivars().hovered.set(0);
        if self.ivars().shown.borrow_mut().take().is_none() {
            return;
        }
        let view = self.ivars().callout.borrow().clone();
        if let Some(view) = view {
            view.popover.close();
        }
    }

    fn cancel_callout(&self) {
        unsafe {
            let _: () = msg_send![
                NSObject::class(),
                cancelPreviousPerformRequestsWithTarget: self as &AnyObject,
                selector: sel!(ngShowCallout),
                object: ptr::null::<AnyObject>(),
            ];
        }
    }

    fn invalidate_cursor_rects(&self) {
        if let Some(window) = self.window() {
            window.invalidateCursorRectsForView(self);
        }
    }
}

impl CalloutView {
    fn new(mtm: MainThreadMarker) -> Self {
        let label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        label.setFont(
            tokens::ns_font(
                tokens::system_font_family(),
                px(CALLOUT_FONT_SIZE),
                FontWeight::NORMAL,
            )
            .as_deref(),
        );
        label.setAlignment(NSTextAlignment::Center);
        let content = Container::new(mtm, NSRect::ZERO, false);
        content.addSubview(&label);
        let controller = NSViewController::new(mtm);
        controller.setView(&content);
        let popover = NSPopover::new(mtm);
        popover.setContentViewController(Some(&controller));
        popover.setBehavior(NSPopoverBehavior::Semitransient);
        popover.setAnimates(false);
        Self { popover, label }
    }

    fn set_text(&self, text: &SharedString) {
        self.label.setStringValue(&NSString::from_str(text));
        let fit = self.label.fittingSize();
        let size = NSSize::new(
            (fit.width + 2. * CALLOUT_INSET_X).ceil(),
            (fit.height + 2. * CALLOUT_INSET_Y).ceil(),
        );
        self.label.setFrame(NSRect::new(
            NSPoint::new(CALLOUT_INSET_X, (size.height - fit.height) / 2.),
            NSSize::new(size.width - 2. * CALLOUT_INSET_X, fit.height),
        ));
        self.popover.setContentSize(size);
    }

    fn disown_mouse(&self) {
        if let Some(window) = self
            .popover
            .contentViewController()
            .and_then(|controller| controller.view().window())
        {
            window.setIgnoresMouseEvents(true);
        }
    }
}

fn contains(rect: NSRect, point: NSPoint) -> bool {
    point.x >= rect.origin.x
        && point.x <= rect.origin.x + rect.size.width
        && point.y >= rect.origin.y
        && point.y <= rect.origin.y + rect.size.height
}

fn region_at<T: Clone>(regions: &Regions<T>, point: NSPoint) -> Option<Region<T>> {
    regions
        .borrow()
        .iter()
        .rev()
        .find(|region| contains(region.rect, point))
        .cloned()
}

fn upsert<T: PartialEq>(regions: &Regions<T>, region: Region<T>) -> bool {
    let mut regions = regions.borrow_mut();
    match regions
        .iter_mut()
        .find(|existing| existing.key == region.key)
    {
        Some(existing) if *existing == region => return false,
        Some(existing) => *existing = region,
        None => regions.push(region),
    }
    true
}

fn remove_region<T>(regions: &Regions<T>, key: u64) -> bool {
    let mut regions = regions.borrow_mut();
    let before = regions.len();
    regions.retain(|region| region.key != key);
    regions.len() != before
}

fn chrome(window: &Window) -> Retained<ChromeHost> {
    let view = gpui_view(window);
    if let Some(host) = view
        .subviews()
        .iter()
        .find(|sub| sub.isKindOfClass(ChromeHost::class()))
    {
        return unsafe { Retained::cast_unchecked::<ChromeHost>(host) };
    }
    let mtm = MainThreadMarker::new().expect("chrome install is a main-thread path");
    let ns_window = view
        .window()
        .expect("GPUI's view is in its window by the time anything paints");

    let corner = Corner {
        supported: ns_window.respondsToSelector(sel!(_setCornerRadius:)),
        system: read_corner_radius(&ns_window).unwrap_or(0.),
        written: None,
    };
    let baseline = Baseline {
        has_shadow: ns_window.hasShadow(),
        is_opaque: ns_window.isOpaque(),
        alpha: ns_window.alphaValue(),
        movable_by_background: ns_window.isMovableByWindowBackground(),
        titlebar_appears_transparent: ns_window.titlebarAppearsTransparent(),
        style: ns_window.styleMask(),
        level: ns_window.level(),
        collection: ns_window.collectionBehavior(),
        appearance: ns_window.appearance(),
    };

    let host = ChromeHost::new(mtm, view.bounds(), corner, baseline);
    view.addSubview_positioned_relativeTo(&host, NSWindowOrderingMode::Below, None);
    host
}

pub(crate) fn ns_window_of(window: &Window) -> Retained<NSWindow> {
    gpui_view(window)
        .window()
        .expect("GPUI's view is in its window")
}

fn read_corner_radius(window: &NSWindow) -> Option<f64> {
    window
        .respondsToSelector(sel!(_cornerRadius))
        .then(|| unsafe { msg_send![window, _cornerRadius] })
}

pub fn supports_corner_radius(window: &Window) -> bool {
    chrome(window).ivars().corner.get().supported
}

pub fn system_corner_radius(window: &Window) -> Pixels {
    let host = chrome(window);
    let mut corner = host.ivars().corner.get();
    if corner.written.is_none() {
        if let Some(live) = read_corner_radius(&ns_window_of(window)) {
            corner.system = live;
            host.ivars().corner.set(corner);
        }
    }
    px(corner.system as f32)
}

pub fn corner_radius(window: &Window) -> Option<Pixels> {
    chrome(window)
        .ivars()
        .corner
        .get()
        .written
        .map(|r| px(r as f32))
}

pub fn set_corner_radius(window: &Window, radius: Option<Pixels>) {
    let host = chrome(window);
    let mut corner = host.ivars().corner.get();
    if !corner.supported {
        return;
    }
    let wanted = radius.map(|r| f32::from(r) as f64).filter(|r| *r > 0.0);
    if corner.written == wanted {
        return;
    }
    let ns_window = ns_window_of(window);
    unsafe {
        let _: () = msg_send![&*ns_window, _setCornerRadius: wanted.unwrap_or(0.0)];
    }
    corner.written = wanted;
    host.ivars().corner.set(corner);
}

pub fn corner_radius_readback(window: &Window) -> Option<Pixels> {
    read_corner_radius(&ns_window_of(window)).map(|radius| px(radius as f32))
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ToolbarStyle {
    Unified,
    UnifiedCompact,
}

impl From<ToolbarStyle> for NSWindowToolbarStyle {
    fn from(style: ToolbarStyle) -> Self {
        match style {
            ToolbarStyle::Unified => NSWindowToolbarStyle::Unified,
            ToolbarStyle::UnifiedCompact => NSWindowToolbarStyle::UnifiedCompact,
        }
    }
}

type ToolbarHandler = Rc<dyn Fn(&mut Window, &mut App)>;
type ToolbarIndexHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;
type ToolbarTextHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;
type ToolbarViewBuilder = Rc<dyn Fn(MainThreadMarker) -> Retained<NSView>>;
type ToolbarEntriesBuilder = Rc<dyn Fn(&mut Window, &mut App) -> Vec<MenuEntry>>;
type ToolbarItemsBuilder = Rc<dyn Fn(&mut Window, &mut App) -> Vec<String>>;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ToolbarSegment {
    pub symbol: String,
    pub label: String,
    pub enabled: bool,
    /// What VoiceOver reads for the segment; the label when `None`.
    pub description: Option<String>,
}

impl ToolbarSegment {
    pub fn new(symbol: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            label: label.into(),
            enabled: true,
            description: None,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum ToolbarSelection {
    #[default]
    SelectOne,
    SelectAny,
    Momentary,
}

impl From<ToolbarSelection> for NSToolbarItemGroupSelectionMode {
    fn from(selection: ToolbarSelection) -> Self {
        match selection {
            ToolbarSelection::SelectOne => NSToolbarItemGroupSelectionMode::SelectOne,
            ToolbarSelection::SelectAny => NSToolbarItemGroupSelectionMode::SelectAny,
            ToolbarSelection::Momentary => NSToolbarItemGroupSelectionMode::Momentary,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum ToolbarGroupWidth {
    #[default]
    Automatic,
    Expanded,
    Collapsed,
}

impl From<ToolbarGroupWidth> for NSToolbarItemGroupControlRepresentation {
    fn from(width: ToolbarGroupWidth) -> Self {
        match width {
            ToolbarGroupWidth::Automatic => NSToolbarItemGroupControlRepresentation::Automatic,
            ToolbarGroupWidth::Expanded => NSToolbarItemGroupControlRepresentation::Expanded,
            ToolbarGroupWidth::Collapsed => NSToolbarItemGroupControlRepresentation::Collapsed,
        }
    }
}

#[derive(Clone)]
pub enum ToolbarItem {
    Symbol {
        id: String,
        symbol: String,
        label: String,
        tool_tip: Option<String>,
        on_click: Option<ToolbarHandler>,
        enabled: bool,
    },
    Group {
        id: String,
        label: String,
        segments: Vec<ToolbarSegment>,
        selection: ToolbarSelection,
        selected: usize,
        width: ToolbarGroupWidth,
        tool_tip: Option<String>,
        enabled: bool,
        on_select: Option<ToolbarIndexHandler>,
    },
    Search {
        id: String,
        label: String,
        placeholder: String,
        width: Pixels,
        tool_tip: Option<String>,
        on_change: Option<ToolbarTextHandler>,
    },
    Menu {
        id: String,
        symbol: String,
        label: String,
        entries: Vec<MenuEntry>,
        tool_tip: Option<String>,
        enabled: bool,
        on_select: Option<ToolbarIndexHandler>,
        on_open: Option<ToolbarEntriesBuilder>,
    },
    Share {
        id: String,
        label: String,
        items: Vec<String>,
        tool_tip: Option<String>,
        enabled: bool,
        on_open: Option<ToolbarItemsBuilder>,
    },
    Separator {
        id: String,
        divider: usize,
    },
    View {
        id: String,
        label: String,
        /// Zero in a dimension leaves it unconstrained.
        size: Size<Pixels>,
        tool_tip: Option<String>,
        build: ToolbarViewBuilder,
    },
    FlexibleSpace,
    Space,
}

impl ToolbarItem {
    pub fn symbol(
        id: impl Into<String>,
        symbol: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        ToolbarItem::Symbol {
            id: id.into(),
            symbol: symbol.into(),
            label: label.into(),
            tool_tip: None,
            on_click: None,
            enabled: true,
        }
    }

    pub fn group(
        id: impl Into<String>,
        label: impl Into<String>,
        segments: impl IntoIterator<Item = ToolbarSegment>,
        selected: usize,
    ) -> Self {
        ToolbarItem::Group {
            id: id.into(),
            label: label.into(),
            segments: segments.into_iter().collect(),
            selection: ToolbarSelection::default(),
            selected,
            width: ToolbarGroupWidth::default(),
            tool_tip: None,
            enabled: true,
            on_select: None,
        }
    }

    pub fn search(id: impl Into<String>, label: impl Into<String>) -> Self {
        ToolbarItem::Search {
            id: id.into(),
            label: label.into(),
            placeholder: String::new(),
            width: px(220.),
            tool_tip: None,
            on_change: None,
        }
    }

    pub fn menu(
        id: impl Into<String>,
        symbol: impl Into<String>,
        label: impl Into<String>,
        entries: impl IntoIterator<Item = MenuEntry>,
    ) -> Self {
        ToolbarItem::Menu {
            id: id.into(),
            symbol: symbol.into(),
            label: label.into(),
            entries: entries.into_iter().collect(),
            tool_tip: None,
            enabled: true,
            on_select: None,
            on_open: None,
        }
    }

    pub fn share(
        id: impl Into<String>,
        label: impl Into<String>,
        items: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        ToolbarItem::Share {
            id: id.into(),
            label: label.into(),
            items: items.into_iter().map(Into::into).collect(),
            tool_tip: None,
            enabled: true,
            on_open: None,
        }
    }

    pub fn separator(id: impl Into<String>, divider: usize) -> Self {
        ToolbarItem::Separator {
            id: id.into(),
            divider,
        }
    }

    pub fn view(
        id: impl Into<String>,
        label: impl Into<String>,
        size: Size<Pixels>,
        build: impl Fn(MainThreadMarker) -> Retained<NSView> + 'static,
    ) -> Self {
        ToolbarItem::View {
            id: id.into(),
            label: label.into(),
            size,
            tool_tip: None,
            build: Rc::new(build),
        }
    }

    pub fn flexible_space() -> Self {
        ToolbarItem::FlexibleSpace
    }

    pub fn space() -> Self {
        ToolbarItem::Space
    }

    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        if let ToolbarItem::Symbol { on_click, .. } = &mut self {
            *on_click = Some(Rc::new(handler));
        }
        self
    }

    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        let handler: ToolbarIndexHandler = Rc::new(handler);
        match &mut self {
            ToolbarItem::Group { on_select, .. } | ToolbarItem::Menu { on_select, .. } => {
                *on_select = Some(handler)
            }
            _ => {}
        }
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        if let ToolbarItem::Search { on_change, .. } = &mut self {
            *on_change = Some(Rc::new(handler));
        }
        self
    }

    /// Refills the menu as it opens; a menu given no entries until then hides its first row.
    pub fn entries_on_open(
        mut self,
        build: impl Fn(&mut Window, &mut App) -> Vec<MenuEntry> + 'static,
    ) -> Self {
        if let ToolbarItem::Menu { on_open, .. } = &mut self {
            *on_open = Some(Rc::new(build));
        }
        self
    }

    pub fn items_on_open(
        mut self,
        build: impl Fn(&mut Window, &mut App) -> Vec<String> + 'static,
    ) -> Self {
        if let ToolbarItem::Share { on_open, .. } = &mut self {
            *on_open = Some(Rc::new(build));
        }
        self
    }

    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        if let ToolbarItem::Search { placeholder, .. } = &mut self {
            *placeholder = text.into();
        }
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        if let ToolbarItem::Search { width: slot, .. } = &mut self {
            *slot = width;
        }
        self
    }

    pub fn selection(mut self, selection: ToolbarSelection) -> Self {
        if let ToolbarItem::Group {
            selection: slot, ..
        } = &mut self
        {
            *slot = selection;
        }
        self
    }

    pub fn group_width(mut self, width: ToolbarGroupWidth) -> Self {
        if let ToolbarItem::Group { width: slot, .. } = &mut self {
            *slot = width;
        }
        self
    }

    pub fn tool_tip(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        match &mut self {
            ToolbarItem::Symbol { tool_tip, .. }
            | ToolbarItem::Group { tool_tip, .. }
            | ToolbarItem::Search { tool_tip, .. }
            | ToolbarItem::Menu { tool_tip, .. }
            | ToolbarItem::Share { tool_tip, .. }
            | ToolbarItem::View { tool_tip, .. } => *tool_tip = Some(text),
            _ => {}
        }
        self
    }

    /// An item AppKit greys out and does not send; a group greys every segment.
    pub fn enabled(mut self, on: bool) -> Self {
        match &mut self {
            ToolbarItem::Symbol { enabled, .. }
            | ToolbarItem::Group { enabled, .. }
            | ToolbarItem::Menu { enabled, .. }
            | ToolbarItem::Share { enabled, .. } => *enabled = on,
            _ => {}
        }
        self
    }

    fn enabled_state(&self) -> Option<bool> {
        match self {
            ToolbarItem::Symbol { enabled, .. }
            | ToolbarItem::Group { enabled, .. }
            | ToolbarItem::Menu { enabled, .. }
            | ToolbarItem::Share { enabled, .. } => Some(*enabled),
            _ => None,
        }
    }

    fn tip(&self) -> Option<&str> {
        match self {
            ToolbarItem::Symbol { tool_tip, .. }
            | ToolbarItem::Group { tool_tip, .. }
            | ToolbarItem::Search { tool_tip, .. }
            | ToolbarItem::Menu { tool_tip, .. }
            | ToolbarItem::Share { tool_tip, .. }
            | ToolbarItem::View { tool_tip, .. } => tool_tip.as_deref(),
            _ => None,
        }
    }

    fn identifier(&self) -> Cow<'_, str> {
        match self {
            ToolbarItem::Symbol { id, .. }
            | ToolbarItem::Group { id, .. }
            | ToolbarItem::Search { id, .. }
            | ToolbarItem::Menu { id, .. }
            | ToolbarItem::Share { id, .. }
            | ToolbarItem::Separator { id, .. }
            | ToolbarItem::View { id, .. } => Cow::Borrowed(id),
            ToolbarItem::FlexibleSpace => unsafe {
                Cow::Owned(NSToolbarFlexibleSpaceItemIdentifier.to_string())
            },
            ToolbarItem::Space => unsafe { Cow::Owned(NSToolbarSpaceItemIdentifier.to_string()) },
        }
    }
}

struct ToolbarIvars {
    items: RefCell<Vec<ToolbarItem>>,
    built: RefCell<Vec<(String, Retained<NSToolbarItem>)>>,
    customization: RefCell<Option<ToolbarCustomization>>,
    monitor: RefCell<Option<Retained<AnyObject>>>,
    // Weak: the window owns the toolbar, which owns this delegate.
    window: Weak<NSWindow>,
    cx: AsyncWindowContext,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGToolbarDelegate"]
    #[ivars = ToolbarIvars]
    struct ToolbarDelegate;

    unsafe impl NSObjectProtocol for ToolbarDelegate {}

    unsafe impl NSToolbarDelegate for ToolbarDelegate {
        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed(&self, _: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            self.identifiers()
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_items(&self, _: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            match &*self.ivars().customization.borrow() {
                Some(customization) => ns_strings(&customization.defaults),
                None => self.identifiers(),
            }
        }

        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item_for(
            &self,
            _: &NSToolbar,
            ident: &NSString,
            inserted: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            self.build_item(ident, inserted)
        }
    }

    unsafe impl NSSharingServicePickerDelegate for ToolbarDelegate {}

    unsafe impl NSMenuDelegate for ToolbarDelegate {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let Some(id) = self.menu_owner(menu) else {
                return;
            };
            let Some(build) = self.find_item(|spec| match spec {
                ToolbarItem::Menu {
                    id: found, on_open, ..
                } if *found == id => on_open.clone(),
                _ => None,
            }) else {
                return;
            };
            if let Ok(entries) = self.ivars().cx.clone().update(|window, cx| build(window, cx)) {
                sync_menu(mtm, menu, &entries, Some((self.as_ref(), sel!(ngToolbarMenu:))));
            }
        }
    }

    unsafe impl NSSharingServicePickerToolbarItemDelegate for ToolbarDelegate {
        #[unsafe(method_id(itemsForSharingServicePickerToolbarItem:))]
        fn share_items(
            &self,
            item: &NSSharingServicePickerToolbarItem,
        ) -> Retained<NSArray<NSString>> {
            let wanted = item.itemIdentifier().to_string();
            let (items, build) = self
                .find_item(|spec| match spec {
                    ToolbarItem::Share {
                        id, items, on_open, ..
                    } if *id == wanted => Some((items.clone(), on_open.clone())),
                    _ => None,
                })
                .unwrap_or_default();
            let texts = build
                .and_then(|build| self.ivars().cx.clone().update(|window, cx| build(window, cx)).ok())
                .unwrap_or(items);
            ns_strings(&texts)
        }
    }

    impl ToolbarDelegate {
        #[unsafe(method(ngToolbarAction:))]
        fn action(&self, sender: *mut AnyObject) {
            let ident: Retained<NSString> = unsafe { msg_send![sender, itemIdentifier] };
            let wanted = ident.to_string();
            if let Some((id, index)) = self.group_selection(&wanted) {
                self.dispatch_index(&id, index);
                return;
            }
            let handler = self.find_item(|item| match item {
                ToolbarItem::Symbol { id, on_click, .. } if *id == wanted => on_click.clone(),
                _ => None,
            });
            if let Some(handler) = handler {
                self.defer(move |window, app| handler(window, app));
            }
        }

        #[unsafe(method(ngToolbarSegment:))]
        fn segment(&self, sender: &AnyObject) {
            let id: Option<Retained<NSString>> = unsafe { msg_send![sender, identifier] };
            // A click reports its segment; an overflow menu row carries it as its tag.
            let index: isize = match sender.downcast_ref::<NSSegmentedControl>() {
                Some(control) => control.selectedSegment(),
                None => unsafe { msg_send![sender, tag] },
            };
            if let (Some(id), Ok(index)) = (id, usize::try_from(index)) {
                self.dispatch_index(&id.to_string(), index);
            }
        }

        #[unsafe(method(ngToolbarSearch:))]
        fn search(&self, sender: *mut AnyObject) {
            let text: Retained<NSString> = unsafe { msg_send![sender, stringValue] };
            let text = text.to_string();
            let found = self.ivars().built.borrow().iter().find_map(|(id, item)| {
                let field = item
                    .downcast_ref::<NSSearchToolbarItem>()?
                    .searchField();
                ptr::eq(Retained::as_ptr(&field).cast::<AnyObject>(), sender.cast_const())
                    .then(|| id.clone())
            });
            let Some(id) = found else {
                return;
            };
            let handler = self.find_item(|item| match item {
                ToolbarItem::Search {
                    id: found,
                    on_change,
                    ..
                } if *found == id => on_change.clone(),
                _ => None,
            });
            if let Some(handler) = handler {
                self.defer(move |window, app| handler(&text, window, app));
            }
        }

        #[unsafe(method(ngToolbarMenu:))]
        fn menu_action(&self, sender: *mut AnyObject) {
            let index: isize = unsafe { msg_send![sender, tag] };
            let menu: Option<Retained<NSMenu>> = unsafe { msg_send![sender, menu] };
            let found = menu.and_then(|menu| self.menu_owner(&menu));
            if let (Some(id), Ok(index)) = (found, usize::try_from(index)) {
                self.dispatch_index(&id, index);
            }
        }
    }
);

fn ns_strings<S: AsRef<str>>(texts: impl IntoIterator<Item = S>) -> Retained<NSArray<NSString>> {
    let texts: Vec<_> = texts
        .into_iter()
        .map(|text| NSString::from_str(text.as_ref()))
        .collect();
    NSArray::from_retained_slice(&texts)
}

impl Drop for ToolbarIvars {
    fn drop(&mut self) {
        if let Some(monitor) = self.monitor.get_mut().take() {
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
    }
}

/// The deepest view outside `content` whose menu is the toolbar's own.
fn toolbar_menu(
    view: &NSView,
    content: &NSView,
    event: &NSEvent,
) -> Option<(Retained<NSMenu>, Retained<NSView>)> {
    if ptr::eq(view, content) {
        return None;
    }
    let deeper = view
        .subviews()
        .iter()
        .find_map(|sub| toolbar_menu(&sub, content, event));
    if deeper.is_some() {
        return deeper;
    }
    let menu = view.menuForEvent(event)?;
    let customizes = menu
        .itemArray()
        .iter()
        .any(|row| row.action() == Some(sel!(runToolbarCustomizationPalette:)));
    customizes.then(|| (menu, view.retain()))
}

impl ToolbarDelegate {
    fn new(mtm: MainThreadMarker, window: &NSWindow, cx: AsyncWindowContext) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ToolbarIvars {
            items: RefCell::new(Vec::new()),
            built: RefCell::new(Vec::new()),
            customization: RefCell::new(None),
            monitor: RefCell::new(None),
            window: Weak::new(window),
            cx,
        });
        unsafe { msg_send![super(this), init] }
    }

    // Under a transparent titlebar AppKit hands a click on empty toolbar space to the content below.
    fn watch_band(this: &Retained<Self>) {
        let weak = Weak::new(&**this);
        let block = RcBlock::new(move |event: ptr::NonNull<NSEvent>| -> *mut NSEvent {
            let answered = weak
                .load()
                .is_some_and(|this| this.band_menu(unsafe { event.as_ref() }));
            if answered {
                ptr::null_mut()
            } else {
                event.as_ptr()
            }
        });
        let mask = NSEventMask::RightMouseDown | NSEventMask::LeftMouseDown;
        let monitor =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
        *this.ivars().monitor.borrow_mut() = monitor;
    }

    fn band_menu(&self, event: &NSEvent) -> bool {
        let secondary = event.r#type() == NSEventType::RightMouseDown
            || event
                .modifierFlags()
                .contains(NSEventModifierFlags::Control);
        let Some(window) = self.ivars().window.load() else {
            return false;
        };
        let point = event.locationInWindow();
        let layout = window.contentLayoutRect();
        let ours = event.windowNumber() == window.windowNumber();
        if !secondary || !ours || point.y < layout.origin.y + layout.size.height {
            return false;
        }
        let (Some(toolbar), Some(content)) = (window.toolbar(), window.contentView()) else {
            return false;
        };
        let Some(theme) = (unsafe { content.superview() }) else {
            return false;
        };
        let inside = theme
            .hitTest(point)
            .is_some_and(|hit| hit.isDescendantOf(&content));
        if !inside || !toolbar.isVisible() {
            return false;
        }
        let Some((menu, view)) = toolbar_menu(&theme, &content, event) else {
            return false;
        };
        NSMenu::popUpContextMenu_withEvent_forView(&menu, event, &view);
        true
    }

    fn identifiers(&self) -> Retained<NSArray<NSToolbarItemIdentifier>> {
        ns_strings(
            self.ivars()
                .items
                .borrow()
                .iter()
                .map(ToolbarItem::identifier),
        )
    }

    fn find_item<R>(&self, found: impl FnMut(&ToolbarItem) -> Option<R>) -> Option<R> {
        self.ivars().items.borrow().iter().find_map(found)
    }

    fn defer(&self, run: impl FnOnce(&mut Window, &mut App) + 'static) {
        self.ivars()
            .cx
            .spawn(async move |cx| {
                let _ = cx.update(run);
            })
            .detach();
    }

    fn menu_owner(&self, menu: &NSMenu) -> Option<String> {
        self.ivars().built.borrow().iter().find_map(|(id, item)| {
            let owned = item.downcast_ref::<NSMenuToolbarItem>()?.menu();
            ptr::eq(Retained::as_ptr(&owned), menu).then(|| id.clone())
        })
    }

    // The sender's position among the subitems: `selectedIndex` is -1 for Momentary.
    fn group_selection(&self, sender_id: &str) -> Option<(String, usize)> {
        self.ivars().built.borrow().iter().find_map(|(id, item)| {
            let group = item.downcast_ref::<NSToolbarItemGroup>()?;
            let clicked = group
                .subitems()
                .iter()
                .position(|sub| sub.itemIdentifier().to_string() == sender_id);
            match clicked {
                Some(index) => Some((id.clone(), index)),
                None if *id == sender_id => {
                    Some((id.clone(), usize::try_from(group.selectedIndex()).ok()?))
                }
                None => None,
            }
        })
    }

    fn dispatch_index(&self, id: &str, index: usize) {
        let handler = self.find_item(|item| match item {
            ToolbarItem::Group {
                id: found,
                on_select,
                ..
            }
            | ToolbarItem::Menu {
                id: found,
                on_select,
                ..
            } if found == id => on_select.clone(),
            _ => None,
        });
        if let Some(handler) = handler {
            self.defer(move |window, app| handler(index, window, app));
        }
    }

    fn build_item(&self, ident: &NSString, inserted: bool) -> Option<Retained<NSToolbarItem>> {
        let mtm = MainThreadMarker::new()?;
        let wanted = ident.to_string();
        let spec = self.find_item(|item| (item.identifier() == wanted).then(|| item.clone()))?;

        let tip = spec.tip().map(worded);
        let enabled = spec.enabled_state();
        let item: Retained<NSToolbarItem> = match spec {
            ToolbarItem::Symbol {
                symbol,
                label,
                on_click,
                ..
            } => {
                let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), ident);
                item.setLabel(&worded(&label));
                item.setBordered(true);
                if let Some(image) = symbol_image(&symbol, &label) {
                    item.setImage(Some(&image));
                }
                if on_click.is_some() {
                    unsafe {
                        item.setTarget(Some(self.as_ref()));
                        item.setAction(Some(sel!(ngToolbarAction:)));
                    }
                }
                item
            }
            ToolbarItem::Group {
                label,
                segments,
                selection,
                selected,
                width,
                ..
            } => {
                let images: Vec<Retained<NSImage>> = segments
                    .iter()
                    .map(|segment| {
                        let spoken = segment.description.as_deref().unwrap_or(&segment.label);
                        symbol_image(&segment.symbol, spoken).unwrap_or_default()
                    })
                    .collect();
                let images = NSArray::from_retained_slice(&images);
                if selection == ToolbarSelection::Momentary {
                    // The group constructor's control is a point wider than a separated one.
                    let control = unsafe {
                        NSSegmentedControl::segmentedControlWithImages_trackingMode_target_action(
                            &images,
                            NSSegmentSwitchTracking::Momentary,
                            Some(self.as_ref()),
                            Some(sel!(ngToolbarSegment:)),
                            mtm,
                        )
                    };
                    control.setSegmentStyle(NSSegmentStyle::Separated);
                    control.setIdentifier(Some(ident));
                    let rows = NSMenu::new(mtm);
                    rows.setAutoenablesItems(false);
                    for (index, segment) in segments.iter().enumerate() {
                        let row = unsafe {
                            NSMenuItem::initWithTitle_action_keyEquivalent(
                                NSMenuItem::alloc(mtm),
                                &NSString::from_str(&segment.label),
                                Some(sel!(ngToolbarSegment:)),
                                &NSString::new(),
                            )
                        };
                        unsafe { row.setTarget(Some(self.as_ref())) };
                        row.setTag(index as isize);
                        row.setIdentifier(Some(ident));
                        rows.addItem(&row);
                    }
                    let overflow = NSMenuItem::new(mtm);
                    overflow.setTitle(&worded(&label));
                    overflow.setSubmenu(Some(&rows));
                    let item =
                        NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), ident);
                    item.setLabel(&worded(&label));
                    item.setView(Some(&control));
                    item.setMenuFormRepresentation(Some(&overflow));
                    item
                } else {
                    // The images constructor, not `subitems`: only it has a control to select in.
                    let labels = ns_strings(segments.iter().map(|segment| &segment.label));
                    let group = unsafe {
                        NSToolbarItemGroup::groupWithItemIdentifier_images_selectionMode_labels_target_action(
                            ident,
                            &images,
                            selection.into(),
                            Some(&labels),
                            Some(self.as_ref()),
                            Some(sel!(ngToolbarAction:)),
                            mtm,
                        )
                    };
                    group.setControlRepresentation(width.into());
                    if selected < segments.len() {
                        group.setSelectedIndex(selected as isize);
                    }
                    group.setLabel(&worded(&label));
                    Retained::into_super(group)
                }
            }
            ToolbarItem::Search {
                label,
                placeholder,
                width,
                ..
            } => {
                let item = NSSearchToolbarItem::initWithItemIdentifier(
                    NSSearchToolbarItem::alloc(mtm),
                    ident,
                );
                item.setLabel(&worded(&label));
                // The item takes over the field's layout on assignment, so configure it first.
                let field = NSSearchField::new(mtm);
                if !placeholder.is_empty() {
                    field.setPlaceholderString(Some(&NSString::from_str(&placeholder)));
                }
                unsafe {
                    field.setSendsSearchStringImmediately(true);
                    field.setSendsWholeSearchString(false);
                    field.setTarget(Some(self.as_ref()));
                    field.setAction(Some(sel!(ngToolbarSearch:)));
                }
                item.setSearchField(&field);
                item.setPreferredWidthForSearchField(f32::from(width) as f64);
                Retained::into_super(item)
            }
            ToolbarItem::Menu {
                symbol,
                label,
                entries,
                on_open,
                ..
            } => {
                let item =
                    NSMenuToolbarItem::initWithItemIdentifier(NSMenuToolbarItem::alloc(mtm), ident);
                item.setLabel(&worded(&label));
                item.setBordered(true);
                if let Some(image) = symbol_image(&symbol, &label) {
                    item.setImage(Some(&image));
                }
                // No action on the item: with one, only the chevron half would open the menu.
                item.setShowsIndicator(true);
                let menu = NSMenu::new(mtm);
                if on_open.is_some() {
                    menu.setDelegate(Some(ProtocolObject::from_ref(self)));
                }
                sync_menu(
                    mtm,
                    &menu,
                    &entries,
                    Some((self.as_ref(), sel!(ngToolbarMenu:))),
                );
                item.setMenu(&menu);
                Retained::into_super(item)
            }
            ToolbarItem::Share { label, .. } => {
                let item = NSSharingServicePickerToolbarItem::initWithItemIdentifier(
                    NSSharingServicePickerToolbarItem::alloc(mtm),
                    ident,
                );
                item.setLabel(&worded(&label));
                item.setDelegate(Some(ProtocolObject::from_ref(self)));
                Retained::into_super(item)
            }
            ToolbarItem::Separator { divider, .. } => {
                let split = self.split_view()?;
                let item = NSTrackingSeparatorToolbarItem::trackingSeparatorToolbarItemWithIdentifier_splitView_dividerIndex(
                    ident,
                    &split,
                    divider as isize,
                );
                Retained::into_super(item)
            }
            ToolbarItem::View {
                label, size, build, ..
            } => {
                let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), ident);
                item.setLabel(&worded(&label));
                let view = build(mtm);
                let mut pinned = Vec::new();
                if size.width > px(0.) {
                    pinned.push(
                        view.widthAnchor()
                            .constraintEqualToConstant(f32::from(size.width) as f64),
                    );
                }
                if size.height > px(0.) {
                    pinned.push(
                        view.heightAnchor()
                            .constraintEqualToConstant(f32::from(size.height) as f64),
                    );
                }
                if !pinned.is_empty() {
                    view.setTranslatesAutoresizingMaskIntoConstraints(false);
                    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&pinned));
                }
                item.setView(Some(&view));
                item
            }
            ToolbarItem::FlexibleSpace | ToolbarItem::Space => return None,
        };
        item.setToolTip(tip.as_deref());
        if let Some(customization) = &*self.ivars().customization.borrow() {
            (customization.finish)(&item);
        }
        // The customization palette asks for copies of its own, which must stay out of `built`.
        if !inserted {
            // An automatic group draws collapsed in the palette; Finder's palette shows its segments.
            if let Some(group) = item.downcast_ref::<NSToolbarItemGroup>() {
                group.setControlRepresentation(NSToolbarItemGroupControlRepresentation::Expanded);
            }
            return Some(item);
        }
        if let Some(enabled) = enabled {
            item.setAutovalidates(false);
            item.setEnabled(enabled);
        }

        let mut built = self.ivars().built.borrow_mut();
        match built.iter_mut().find(|(id, _)| *id == wanted) {
            Some(slot) => slot.1 = item.clone(),
            None => built.push((wanted, item.clone())),
        }
        Some(item)
    }

    fn refresh(&self) {
        let items = self.ivars().items.borrow();
        let built = self.ivars().built.borrow();
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        for spec in items.iter() {
            let id = spec.identifier();
            let Some((_, item)) = built.iter().find(|(found, _)| *found == id) else {
                continue;
            };
            if let Some(enabled) = spec.enabled_state() {
                if item.isEnabled() != enabled {
                    item.setEnabled(enabled);
                }
            }
            match spec {
                ToolbarItem::Group {
                    segments,
                    selection,
                    selected,
                    width,
                    ..
                } => {
                    if *selection == ToolbarSelection::Momentary {
                        let Some(control) = item
                            .view()
                            .and_then(|view| view.downcast::<NSSegmentedControl>().ok())
                        else {
                            continue;
                        };
                        let rows = item.menuFormRepresentation().and_then(|row| row.submenu());
                        for (at, segment) in (0..).zip(segments) {
                            if control.isEnabledForSegment(at) != segment.enabled {
                                control.setEnabled_forSegment(segment.enabled, at);
                            }
                            if let Some(row) = rows.as_ref().and_then(|rows| rows.itemAtIndex(at)) {
                                row.setEnabled(segment.enabled);
                            }
                        }
                        continue;
                    }
                    let Some(group) = item.downcast_ref::<NSToolbarItemGroup>() else {
                        continue;
                    };
                    let representation = (*width).into();
                    if group.controlRepresentation() != representation {
                        group.setControlRepresentation(representation);
                    }
                    if *selection == ToolbarSelection::SelectOne && *selected < segments.len() {
                        let wanted = *selected as isize;
                        if group.selectedIndex() != wanted {
                            group.setSelectedIndex(wanted);
                        }
                    }
                }
                ToolbarItem::Search {
                    placeholder, width, ..
                } => {
                    let Some(search) = item.downcast_ref::<NSSearchToolbarItem>() else {
                        continue;
                    };
                    let wanted = f32::from(*width) as f64;
                    if search.preferredWidthForSearchField() != wanted {
                        search.setPreferredWidthForSearchField(wanted);
                    }
                    let field = search.searchField();
                    let current = field
                        .placeholderString()
                        .map_or(String::new(), |text| text.to_string());
                    if current != *placeholder {
                        field.setPlaceholderString(Some(&NSString::from_str(placeholder)));
                    }
                }
                ToolbarItem::Menu { entries, .. } => {
                    let Some(menu_item) = item.downcast_ref::<NSMenuToolbarItem>() else {
                        continue;
                    };
                    sync_menu(
                        mtm,
                        &menu_item.menu(),
                        entries,
                        Some((self.as_ref(), sel!(ngToolbarMenu:))),
                    );
                }
                ToolbarItem::Symbol { label, .. } => {
                    if item.label().to_string() != *label {
                        item.setLabel(&NSString::from_str(label));
                    }
                }
                _ => {}
            }
        }
    }

    fn split_view(&self) -> Option<Retained<NSSplitView>> {
        let window = self.ivars().window.load()?;
        let controller = window
            .contentViewController()?
            .downcast::<NSSplitViewController>()
            .ok()?;
        Some(controller.splitView())
    }
}

fn symbol_image(symbol: &str, label: &str) -> Option<Retained<NSImage>> {
    tokens::sf_symbol(symbol, tokens::system_font_size(), FontWeight::NORMAL)?;
    NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(symbol),
        Some(&NSString::from_str(label)),
    )
}

/// Where a user-arranged toolbar starts; AppKit keeps the user's arrangement in defaults thereafter.
#[derive(Clone, Debug)]
pub struct ToolbarCustomization {
    pub defaults: Vec<String>,
    /// The first arrangement, less anything the palette doesn't offer.
    pub initial: Vec<String>,
    /// Runs on every item built, the palette's copies included.
    pub finish: fn(&NSToolbarItem),
}

/// Conflicts with `traffic_light_position`.
pub fn set_toolbar(
    window: &mut Window,
    cx: &mut App,
    style: Option<ToolbarStyle>,
    items: &[ToolbarItem],
) {
    install_toolbar(window, cx, style, items, None);
}

/// A toolbar the user arranges from a palette offering `items`, in their order.
pub fn set_customizable_toolbar(
    window: &mut Window,
    cx: &mut App,
    style: ToolbarStyle,
    items: &[ToolbarItem],
    customization: &ToolbarCustomization,
) {
    install_toolbar(window, cx, Some(style), items, Some(customization));
}

fn install_toolbar(
    window: &mut Window,
    cx: &mut App,
    style: Option<ToolbarStyle>,
    items: &[ToolbarItem],
    customization: Option<&ToolbarCustomization>,
) {
    let host = chrome(window);
    let ns_window = ns_window_of(window);

    let Some(style) = style else {
        ns_window.setToolbar(None);
        *host.ivars().toolbar.borrow_mut() = None;
        *host.ivars().toolbar_installed.borrow_mut() = None;
        return;
    };

    let mtm = MainThreadMarker::new().expect("set_toolbar is a main-thread path");
    let identifiers: Vec<String> = items
        .iter()
        .map(|item| item.identifier().into_owned())
        .collect();

    let delegate = {
        let mut slot = host.ivars().toolbar_delegate.borrow_mut();
        slot.get_or_insert_with(|| ToolbarDelegate::new(mtm, &ns_window, window.to_async(cx)))
            .clone()
    };
    *delegate.ivars().items.borrow_mut() = items.to_vec();

    let mut seed = None;
    let toolbar = {
        let mut slot = host.ivars().toolbar.borrow_mut();
        slot.get_or_insert_with(|| {
            let name = "ng.window_chrome.toolbar";
            let toolbar =
                NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &NSString::from_str(name));
            toolbar.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
            toolbar.setAllowsUserCustomization(customization.is_some());
            if let Some(customization) = customization {
                // AppKit's autosave key; the first window of a first run finds nothing there.
                let key = NSString::from_str(&format!("NSToolbar Configuration {name}"));
                if NSUserDefaults::standardUserDefaults()
                    .objectForKey(&key)
                    .is_none()
                {
                    let offered = customization
                        .initial
                        .iter()
                        .filter(|id| identifiers.contains(id));
                    seed = Some(ns_strings(offered));
                }
                *delegate.ivars().customization.borrow_mut() = Some(customization.clone());
                toolbar.setAutosavesConfiguration(true);
                ToolbarDelegate::watch_band(&delegate);
            }
            toolbar
        })
        .clone()
    };
    let unchanged = host
        .ivars()
        .toolbar_installed
        .borrow()
        .as_ref()
        .is_some_and(|(installed_style, installed)| {
            *installed_style == style && (customization.is_some() || *installed == identifiers)
        });
    if !unchanged {
        if customization.is_none() {
            toolbar.setItemIdentifiers(&ns_strings(&identifiers));
        }
        ns_window.setToolbarStyle(style.into());
        ns_window.setToolbar(Some(&toolbar));
        if let Some(seed) = &seed {
            toolbar.setItemIdentifiers(seed);
        }
        toolbar.setVisible(true);
        *host.ivars().toolbar_installed.borrow_mut() = Some((style, identifiers));
    }
    delegate.refresh();
}

pub fn toolbar_description(window: &Window) -> String {
    let ns_window = ns_window_of(window);
    let Some(toolbar) = ns_window.toolbar() else {
        return String::from("no toolbar");
    };
    let visible: Vec<String> = toolbar
        .visibleItems()
        .map(|items| {
            items
                .iter()
                .map(|item| item.itemIdentifier().to_string())
                .collect()
        })
        .unwrap_or_default();
    let mut lines = Vec::new();
    for item in toolbar.items().iter() {
        let id = item.itemIdentifier().to_string();
        let class = item.class().name().to_string_lossy().into_owned();
        let shown = if visible.contains(&id) {
            "visible"
        } else {
            "clipped"
        };
        let detail = if let Some(group) = item.downcast_ref::<NSToolbarItemGroup>() {
            format!(
                " segments={} selected={} representation={:?}",
                group.subitems().count(),
                group.selectedIndex(),
                group.controlRepresentation().0,
            )
        } else if let Some(search) = item.downcast_ref::<NSSearchToolbarItem>() {
            let frame = search.searchField().frame().size;
            format!(
                " field={:.0}x{:.0} preferred={:.0}",
                frame.width,
                frame.height,
                search.preferredWidthForSearchField(),
            )
        } else if let Some(menu) = item.downcast_ref::<NSMenuToolbarItem>() {
            format!(" rows={}", menu.menu().numberOfItems())
        } else {
            item.view()
                .map(|view| {
                    let size = view.frame().size;
                    format!(" view={:.0}x{:.0}", size.width, size.height)
                })
                .unwrap_or_default()
        };
        lines.push(format!("{id} {class} {shown}{detail}"));
    }
    lines.join("\n")
}

fn toolbar_item(window: &Window, id: &str) -> Option<Retained<NSToolbarItem>> {
    ns_window_of(window)
        .toolbar()?
        .items()
        .iter()
        .find(|item| item.itemIdentifier().to_string() == id)
}

pub fn toolbar_item_view(window: &Window, id: &str) -> Option<Retained<NSView>> {
    toolbar_item(window, id)?.view()
}

pub fn set_toolbar_search_active(window: &Window, id: &str, active: bool) {
    let Some(item) = toolbar_item(window, id) else {
        return;
    };
    let Some(search) = item.downcast_ref::<NSSearchToolbarItem>() else {
        return;
    };
    if active {
        search.beginSearchInteraction();
    } else {
        search.endSearchInteraction();
    }
}

pub fn titlebar_band(window: &Window) -> Pixels {
    let ns_window = ns_window_of(window);
    let frame = ns_window.frame();
    // Not contentRectForFrameRect: under FullSizeContentView that is the whole frame.
    px((frame.size.height - ns_window.contentLayoutRect().size.height) as f32)
}

pub fn traffic_light_inset(window: &Window) -> Option<Point<Pixels>> {
    let ns_window = ns_window_of(window);
    let button = ns_window.standardWindowButton(NSWindowButton::CloseButton)?;
    let frame = ns_window.frame();
    let in_window = button.convertRect_toView(button.bounds(), None);
    Some(Point {
        x: px(in_window.origin.x as f32),
        y: px((frame.size.height - in_window.origin.y - in_window.size.height) as f32),
    })
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum WindowLevel {
    Normal,
    Floating,
    TornOffMenu,
    ModalPanel,
    PopUpMenu,
    Status,
    ScreenSaver,
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowLevelForKey(key: i32) -> i32;
}

pub fn window_level(level: WindowLevel) -> isize {
    // `CGWindowLevelKey`, in header order.
    let key = match level {
        WindowLevel::Normal => 4,
        WindowLevel::Floating => 5,
        WindowLevel::TornOffMenu => 6,
        WindowLevel::Status => 9,
        WindowLevel::ModalPanel => 10,
        WindowLevel::PopUpMenu => 11,
        WindowLevel::ScreenSaver => 13,
    };
    unsafe { CGWindowLevelForKey(key) as isize }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WindowFlags {
    pub has_shadow: bool,
    pub is_opaque: bool,
    pub alpha: f32,
    pub movable_by_background: bool,
    pub titlebar_appears_transparent: bool,
    pub titled: bool,
    pub full_size_content: bool,
    pub level: isize,
    pub collection: NSWindowCollectionBehavior,
    pub appearance: Option<tokens::Appearance>,
}

pub fn remember_frame(window: &Window, name: &str) {
    let ns_window = ns_window_of(window);
    let name = NSString::from_str(name);
    ns_window.setFrameUsingName(&name);
    ns_window.setFrameAutosaveName(&name);
}

pub fn window_flags(window: &Window) -> WindowFlags {
    let ns_window = ns_window_of(window);
    let style = ns_window.styleMask();
    WindowFlags {
        has_shadow: ns_window.hasShadow(),
        is_opaque: ns_window.isOpaque(),
        alpha: ns_window.alphaValue() as f32,
        movable_by_background: ns_window.isMovableByWindowBackground(),
        titlebar_appears_transparent: ns_window.titlebarAppearsTransparent(),
        titled: style.contains(NSWindowStyleMask::Titled),
        full_size_content: style.contains(NSWindowStyleMask::FullSizeContentView),
        level: ns_window.level(),
        collection: ns_window.collectionBehavior(),
        appearance: ns_window.appearance().map(appearance_of),
    }
}

pub fn set_window_flags(window: &Window, flags: WindowFlags) {
    let ns_window = ns_window_of(window);
    // Installing the host captures the baseline `restore_window_flags` returns to.
    let _ = chrome(window);

    ns_window.setHasShadow(flags.has_shadow);
    ns_window.setOpaque(flags.is_opaque);
    ns_window.setAlphaValue(flags.alpha as f64);
    ns_window.setMovableByWindowBackground(flags.movable_by_background);
    ns_window.setTitlebarAppearsTransparent(flags.titlebar_appears_transparent);

    let mut style = ns_window.styleMask();
    style.set(NSWindowStyleMask::Titled, flags.titled);
    style.set(
        NSWindowStyleMask::FullSizeContentView,
        flags.full_size_content,
    );
    if style != ns_window.styleMask() {
        ns_window.setStyleMask(style);
    }

    if ns_window.level() != flags.level {
        ns_window.setLevel(flags.level);
    }
    if ns_window.collectionBehavior() != flags.collection {
        ns_window.setCollectionBehavior(flags.collection);
    }
    let appearance = flags.appearance.map(ns_appearance);
    ns_window.setAppearance(appearance.as_deref());
}

pub fn restore_window_flags(window: &Window) {
    let host = chrome(window);
    let baseline = &host.ivars().baseline;
    let ns_window = ns_window_of(window);
    ns_window.setHasShadow(baseline.has_shadow);
    ns_window.setOpaque(baseline.is_opaque);
    ns_window.setAlphaValue(baseline.alpha);
    ns_window.setMovableByWindowBackground(baseline.movable_by_background);
    ns_window.setTitlebarAppearsTransparent(baseline.titlebar_appears_transparent);
    if ns_window.styleMask() != baseline.style {
        ns_window.setStyleMask(baseline.style);
    }
    ns_window.setLevel(baseline.level);
    ns_window.setCollectionBehavior(baseline.collection);
    ns_window.setAppearance(baseline.appearance.as_deref());
}

fn ns_appearance(appearance: tokens::Appearance) -> Retained<NSAppearance> {
    let name = unsafe {
        match appearance {
            tokens::Appearance::Light => NSAppearanceNameAqua,
            tokens::Appearance::Dark => NSAppearanceNameDarkAqua,
        }
    };
    NSAppearance::appearanceNamed(name).expect("the two base appearances always resolve")
}

fn appearance_of(appearance: Retained<NSAppearance>) -> tokens::Appearance {
    if appearance.name().to_string().contains("Dark") {
        tokens::Appearance::Dark
    } else {
        tokens::Appearance::Light
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Cursor {
    Arrow,
    PointingHand,
    IBeam,
    IBeamVertical,
    Crosshair,
    OpenHand,
    ClosedHand,
    ColumnResize,
    RowResize,
    ContextualMenu,
    DragCopy,
    DragLink,
    OperationNotAllowed,
}

impl Cursor {
    fn ns(self) -> Retained<NSCursor> {
        match self {
            Cursor::Arrow => NSCursor::arrowCursor(),
            Cursor::PointingHand => NSCursor::pointingHandCursor(),
            Cursor::IBeam => NSCursor::IBeamCursor(),
            Cursor::IBeamVertical => NSCursor::IBeamCursorForVerticalLayout(),
            Cursor::Crosshair => NSCursor::crosshairCursor(),
            Cursor::OpenHand => NSCursor::openHandCursor(),
            Cursor::ClosedHand => NSCursor::closedHandCursor(),
            Cursor::ColumnResize => NSCursor::columnResizeCursor(),
            Cursor::RowResize => NSCursor::rowResizeCursor(),
            Cursor::ContextualMenu => NSCursor::contextualMenuCursor(),
            Cursor::DragCopy => NSCursor::dragCopyCursor(),
            Cursor::DragLink => NSCursor::dragLinkCursor(),
            Cursor::OperationNotAllowed => NSCursor::operationNotAllowedCursor(),
        }
    }

    fn gpui(self) -> CursorStyle {
        match self {
            Cursor::Arrow => CursorStyle::Arrow,
            Cursor::PointingHand => CursorStyle::PointingHand,
            Cursor::IBeam => CursorStyle::IBeam,
            Cursor::IBeamVertical => CursorStyle::IBeamCursorForVerticalLayout,
            Cursor::Crosshair => CursorStyle::Crosshair,
            Cursor::OpenHand => CursorStyle::OpenHand,
            Cursor::ClosedHand => CursorStyle::ClosedHand,
            Cursor::ColumnResize => CursorStyle::ResizeColumn,
            Cursor::RowResize => CursorStyle::ResizeRow,
            Cursor::ContextualMenu => CursorStyle::ContextualMenu,
            Cursor::DragCopy => CursorStyle::DragCopy,
            Cursor::DragLink => CursorStyle::DragLink,
            Cursor::OperationNotAllowed => CursorStyle::OperationNotAllowed,
        }
    }
}

pub fn cursor_region(
    id: impl Into<ElementId>,
    cursor: Cursor,
    child: impl IntoElement,
) -> CursorRegion {
    CursorRegion {
        id: id.into(),
        cursor,
        child: child.into_any_element(),
    }
}

pub struct CursorRegion {
    id: ElementId,
    cursor: Cursor,
    child: AnyElement,
}

struct RegionGuard<E> {
    host: Retained<ChromeHost>,
    key: u64,
    remove: fn(&ChromeHost, u64),
    element: PhantomData<E>,
}

impl<E> Drop for RegionGuard<E> {
    fn drop(&mut self) {
        (self.remove)(&self.host, self.key);
    }
}

fn track<E: 'static>(
    window: &mut Window,
    id: &GlobalElementId,
    remove: fn(&ChromeHost, u64),
) -> (Retained<ChromeHost>, u64) {
    let host = chrome(window);
    let key = element_key(Some(id));
    window.with_element_state::<RegionGuard<E>, ()>(id, |existing, _| {
        let guard = existing.unwrap_or_else(|| RegionGuard {
            host: host.clone(),
            key,
            remove,
            element: PhantomData,
        });
        ((), guard)
    });
    (host, key)
}

fn visible(bounds: Bounds<Pixels>, window: &Window) -> Option<Bounds<Pixels>> {
    let visible = bounds.intersect(&window.content_mask().bounds);
    (visible.size.width > px(0.) && visible.size.height > px(0.)).then_some(visible)
}

impl Element for CursorRegion {
    type RequestLayoutState = ();
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
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
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Hitbox> {
        self.child.prepaint(window, cx);
        visible(bounds, window).map(|visible| window.insert_hitbox(visible, HitboxBehavior::Normal))
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Option<Hitbox>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);

        let (Some(id), Some(hitbox)) = (id, hitbox.as_ref()) else {
            return;
        };
        // Both mechanisms: gpui resets the cursor to Arrow at the end of every drawn frame.
        window.set_cursor_style(self.cursor.gpui(), hitbox);
        let (host, key) = track::<Self>(window, id, ChromeHost::remove_claim);
        host.set_claim(key, native_rect(hitbox.bounds), self.cursor);
    }
}

impl IntoElement for CursorRegion {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

pub fn native_tooltip(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    child: impl IntoElement,
) -> NativeTooltip {
    NativeTooltip {
        id: id.into(),
        text: text.into(),
        child: child.into_any_element(),
    }
}

pub struct NativeTooltip {
    id: ElementId,
    text: SharedString,
    child: AnyElement,
}

impl Element for NativeTooltip {
    type RequestLayoutState = ();
    type PrepaintState = Option<Bounds<Pixels>>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
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
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.child.prepaint(window, cx);
        visible(bounds, window)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        visible: &mut Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);

        let Some(id) = id else {
            return;
        };
        let (host, key) = track::<Self>(window, id, ChromeHost::remove_tip);
        match visible {
            Some(bounds) => host.set_tip(key, native_rect(*bounds), &self.text),
            None => host.remove_tip(key),
        }
    }
}

impl IntoElement for NativeTooltip {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

pub fn native_callout(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    child: impl IntoElement,
) -> NativeCallout {
    NativeCallout {
        id: id.into(),
        text: text.into(),
        child: child.into_any_element(),
    }
}

pub struct NativeCallout {
    id: ElementId,
    text: SharedString,
    child: AnyElement,
}

impl Element for NativeCallout {
    type RequestLayoutState = ();
    type PrepaintState = Option<Bounds<Pixels>>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
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
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.child.prepaint(window, cx);
        visible(bounds, window)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        visible: &mut Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);

        let Some(id) = id else {
            return;
        };
        let (host, key) = track::<Self>(window, id, ChromeHost::remove_callout);
        match visible {
            Some(bounds) => host.set_callout(key, native_rect(*bounds), &self.text),
            None => host.remove_callout(key),
        }
    }
}

impl IntoElement for NativeCallout {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

pub fn current_cursor_description() -> String {
    let cursor = NSCursor::currentCursor();
    let hot = cursor.hotSpot();
    let size: NSSize = cursor.image().size();
    format!(
        "{} hotSpot={:.1},{:.1} imageSize={:.1}x{:.1}",
        cursor.class().name().to_string_lossy(),
        hot.x,
        hot.y,
        size.width,
        size.height
    )
}
