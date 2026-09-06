//! The window itself: its corners, toolbar, flags, cursor, and the native coordinate recipe
//! [`native_rect`] -> [`native_window_rect`] -> [`native_screen_rect`].

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::panic::Location;
use std::ptr;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AsyncWindowContext, Bounds, CursorStyle, Element, ElementId, FontWeight,
    GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, Pixels,
    Point, SharedString, Size, Window, px,
};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSCursor, NSEvent, NSImage, NSLayoutConstraint, NSMenu,
    NSMenuToolbarItem, NSSearchField, NSSearchToolbarItem, NSSharingServicePickerDelegate,
    NSSharingServicePickerToolbarItem, NSSharingServicePickerToolbarItemDelegate, NSSplitView,
    NSSplitViewController, NSToolTipTag, NSToolbar, NSToolbarDelegate, NSToolbarDisplayMode,
    NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem, NSToolbarItemGroup,
    NSToolbarItemGroupControlRepresentation, NSToolbarItemGroupSelectionMode,
    NSToolbarItemIdentifier, NSToolbarSpaceItemIdentifier, NSTrackingArea, NSTrackingAreaOptions,
    NSTrackingSeparatorToolbarItem, NSView, NSWindow, NSWindowButton, NSWindowCollectionBehavior,
    NSWindowLevel, NSWindowOrderingMode, NSWindowStyleMask, NSWindowToolbarStyle,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use crate::bridge::appkit::{gpui_view, to_rect};
use crate::bridge::host::element_key;
use crate::controls::menu::{MenuEntry, sync_menu};
use crate::tokens;

/// A GPUI element's `bounds` in [`chrome_host`] space, which is flipped — so, the identity.
pub fn native_rect(bounds: Bounds<Pixels>) -> NSRect {
    to_rect(bounds)
}

/// A GPUI element's `bounds` in window base coordinates.
pub fn native_window_rect(host: &NSView, bounds: Bounds<Pixels>) -> NSRect {
    host.convertRect_toView(native_rect(bounds), None)
}

/// A GPUI element's `bounds` in screen coordinates.
pub fn native_screen_rect(host: &NSView, bounds: Bounds<Pixels>) -> NSRect {
    let window = host
        .window()
        .expect("native_screen_rect: host view is not in a window");
    window.convertRectToScreen(native_window_rect(host, bounds))
}

/// The flipped view that owns this window's chrome: positioning view and [`cursor_region`] owner.
pub fn chrome_host(window: &Window) -> Retained<NSView> {
    Retained::into_super(chrome(window))
}

#[derive(Copy, Clone, PartialEq)]
struct Claim {
    key: u64,
    rect: NSRect,
    cursor: Cursor,
}

#[derive(Clone, PartialEq)]
struct Tip {
    key: u64,
    rect: NSRect,
    text: SharedString,
}

#[derive(Copy, Clone)]
struct Corner {
    supported: bool,
    system: f64,
    /// Tracked intent: `_setCornerRadius: 0` means "system default" and reads back as one.
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
    claims: RefCell<Vec<Claim>>,
    tips: RefCell<Vec<Tip>>,
    corner: Cell<Corner>,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    /// A cursor this host set itself because its window is not key.
    forced: Cell<bool>,
    toolbar: RefCell<Option<Retained<NSToolbar>>>,
    toolbar_delegate: RefCell<Option<Retained<ToolbarDelegate>>>,
    toolbar_installed: RefCell<Option<(ToolbarStyle, Vec<String>)>>,
    baseline: RefCell<Option<Baseline>>,
}

define_class!(
    /// Flipped, event-transparent, one instance per GPUI window holding its whole chrome state.
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

        // `hitTest:` arrives in the superview's space, hence the conversion.
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
                claim.cursor.ns().set();
            }
        }

        // Cursor rects and gpui's own cursor styles both apply only in the key window, and AppKit
        // puts the arrow back after every move in a window that is not key. A window that can never
        // be key, such as a child panel, sets its cursor a turn later, after that reset.
        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, _: &NSEvent) {
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
                    claim.cursor.ns().set();
                    self.ivars().forced.set(true);
                }
                None => self.release_cursor(),
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _: &NSEvent) {
            self.release_cursor();
        }

        #[unsafe(method_id(view:stringForToolTip:point:userData:))]
        fn string_for_tool_tip(
            &self,
            _: &NSView,
            _: NSToolTipTag,
            point: NSPoint,
            _: *mut c_void,
        ) -> Retained<NSString> {
            NSString::from_str(self.tip_at(point).as_deref().unwrap_or(""))
        }

        // One rect per claim: `cursorUpdate:` alone cannot switch between adjacent regions.
        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            for claim in self.ivars().claims.borrow().iter() {
                self.addCursorRect_cursor(claim.rect, &claim.cursor.ns());
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
            corner: Cell::new(corner),
            tracking: RefCell::new(None),
            forced: Cell::new(false),
            toolbar: RefCell::new(None),
            toolbar_delegate: RefCell::new(None),
            toolbar_installed: RefCell::new(None),
            baseline: RefCell::new(Some(baseline)),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        this
    }

    fn release_cursor(&self) {
        if self.ivars().forced.replace(false) {
            NSCursor::arrowCursor().set();
        }
    }

    fn claim_at(&self, point: NSPoint) -> Option<Claim> {
        self.ivars()
            .claims
            .borrow()
            .iter()
            .rev()
            .find(|claim| contains(claim.rect, point))
            .copied()
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
        let claim = Claim { key, rect, cursor };
        let mut claims = self.ivars().claims.borrow_mut();
        match claims.iter_mut().find(|existing| existing.key == key) {
            Some(existing) if *existing == claim => return,
            Some(existing) => *existing = claim,
            None => claims.push(claim),
        }
        drop(claims);
        self.ensure_tracking();
        self.invalidate_cursor_rects();
    }

    fn remove_claim(&self, key: u64) {
        let mut claims = self.ivars().claims.borrow_mut();
        let before = claims.len();
        claims.retain(|claim| claim.key != key);
        let changed = claims.len() != before;
        drop(claims);
        if changed {
            self.invalidate_cursor_rects();
        }
    }

    fn tip_at(&self, point: NSPoint) -> Option<SharedString> {
        self.ivars()
            .tips
            .borrow()
            .iter()
            .rev()
            .find(|tip| contains(tip.rect, point))
            .map(|tip| tip.text.clone())
    }

    fn set_tip(&self, key: u64, rect: NSRect, text: &SharedString) {
        let tip = Tip {
            key,
            rect,
            text: text.clone(),
        };
        let mut tips = self.ivars().tips.borrow_mut();
        match tips.iter_mut().find(|existing| existing.key == key) {
            Some(existing) if *existing == tip => return,
            Some(existing) => *existing = tip,
            None => tips.push(tip),
        }
        drop(tips);
        self.reset_tool_tips();
    }

    fn remove_tip(&self, key: u64) {
        let mut tips = self.ivars().tips.borrow_mut();
        let before = tips.len();
        tips.retain(|tip| tip.key != key);
        let changed = tips.len() != before;
        drop(tips);
        if changed {
            self.reset_tool_tips();
        }
    }

    // A tool tip rect cannot be moved, only replaced, and replacing one restarts its delay.
    fn reset_tool_tips(&self) {
        self.removeAllToolTips();
        for tip in self.ivars().tips.borrow().iter() {
            unsafe { self.addToolTipRect_owner_userData(tip.rect, self.as_ref(), ptr::null_mut()) };
        }
    }

    fn invalidate_cursor_rects(&self) {
        if let Some(window) = self.window() {
            window.invalidateCursorRectsForView(self);
        }
    }
}

fn contains(rect: NSRect, point: NSPoint) -> bool {
    point.x >= rect.origin.x
        && point.x <= rect.origin.x + rect.size.width
        && point.y >= rect.origin.y
        && point.y <= rect.origin.y + rect.size.height
}

/// Install-or-get. Everything public in this module funnels through here.
fn chrome(window: &Window) -> Retained<ChromeHost> {
    let view = gpui_view(window);
    let class = ChromeHost::class();
    for sub in view.subviews().iter() {
        if sub.isKindOfClass(class) {
            return unsafe { Retained::cast_unchecked::<ChromeHost>(sub) };
        }
    }

    let mtm = MainThreadMarker::new().expect("chrome install is a main-thread path");
    let ns_window = view
        .window()
        .expect("GPUI's view is in its window by the time anything paints");

    let supported = ns_window.respondsToSelector(sel!(_setCornerRadius:));
    let system = if ns_window.respondsToSelector(sel!(_cornerRadius)) {
        unsafe { msg_send![&*ns_window, _cornerRadius] }
    } else {
        0.0f64
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

    let host = ChromeHost::new(
        mtm,
        view.bounds(),
        Corner {
            supported,
            system,
            written: None,
        },
        baseline,
    );
    view.addSubview_positioned_relativeTo(&host, NSWindowOrderingMode::Below, None);
    host
}

fn ns_window_of(window: &Window) -> Retained<NSWindow> {
    gpui_view(window)
        .window()
        .expect("GPUI's view is in its window")
}

/// Whether this macOS has the private setter [`set_corner_radius`] drives; if not, it is a no-op.
pub fn supports_corner_radius(window: &Window) -> bool {
    chrome(window).ivars().corner.get().supported
}

/// What the OS would round this window's corners to right now, if nobody overrode it.
pub fn system_corner_radius(window: &Window) -> Pixels {
    let host = chrome(window);
    let mut corner = host.ivars().corner.get();
    if corner.written.is_none() {
        if let Some(live) = corner_radius_readback(window) {
            corner.system = f32::from(live) as f64;
            host.ivars().corner.set(corner);
        }
    }
    px(corner.system as f32)
}

/// The radius this crate last wrote, or `None` if the window is following the system.
pub fn corner_radius(window: &Window) -> Option<Pixels> {
    chrome(window)
        .ivars()
        .corner
        .get()
        .written
        .map(|r| px(r as f32))
}

/// Round the window's real corners so the shadow follows; `None` and `px(0.)` both mean the OS's.
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

/// `_cornerRadius` as reported now. Diagnostics only — use [`corner_radius`] for intent.
pub fn corner_radius_readback(window: &Window) -> Option<Pixels> {
    let ns_window = ns_window_of(window);
    if !ns_window.respondsToSelector(sel!(_cornerRadius)) {
        return None;
    }
    let radius: f64 = unsafe { msg_send![&*ns_window, _cornerRadius] };
    Some(px(radius as f32))
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ToolbarSegment {
    pub symbol: String,
    pub label: String,
}

impl ToolbarSegment {
    pub fn new(symbol: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            label: label.into(),
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
/// How a [`ToolbarItem::Group`] tracks clicks.
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
/// What a [`ToolbarItem::Group`] does when the toolbar runs out of room.
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
/// One toolbar item, which AppKit draws — so each gets the current system treatment.
pub enum ToolbarItem {
    Symbol {
        id: String,
        symbol: String,
        label: String,
        on_click: Option<ToolbarHandler>,
    },
    /// Several glyphs in one capsule: `NSToolbarItemGroup`.
    Group {
        id: String,
        label: String,
        segments: Vec<ToolbarSegment>,
        selection: ToolbarSelection,
        selected: usize,
        width: ToolbarGroupWidth,
        on_select: Option<ToolbarIndexHandler>,
    },
    /// The search field that collapses to a glyph: `NSSearchToolbarItem`.
    Search {
        id: String,
        label: String,
        placeholder: String,
        /// What the field widens to when it takes focus.
        width: Pixels,
        on_change: Option<ToolbarTextHandler>,
    },
    /// A glyph with a chevron that drops a real menu: `NSMenuToolbarItem`.
    Menu {
        id: String,
        symbol: String,
        label: String,
        entries: Vec<MenuEntry>,
        on_select: Option<ToolbarIndexHandler>,
    },
    /// The system share sheet: `NSSharingServicePickerToolbarItem`.
    Share {
        id: String,
        label: String,
        items: Vec<String>,
    },
    /// A divider tracking a split-view divider: `NSTrackingSeparatorToolbarItem`. Needs a sidebar.
    Separator {
        id: String,
        /// Which divider, counting from the leading edge. A one-sidebar window has divider 0.
        divider: usize,
    },
    /// Any other AppKit view, built once per identifier and retained by the toolbar.
    View {
        id: String,
        label: String,
        /// Constrained per dimension, and only where positive: zero means "measure yourself".
        size: Size<Pixels>,
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
            on_click: None,
        }
    }

    /// `selected` is the index you own; wire [`ToolbarItem::on_select`] to keep it.
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
            on_select: None,
        }
    }

    pub fn search(id: impl Into<String>, label: impl Into<String>) -> Self {
        ToolbarItem::Search {
            id: id.into(),
            label: label.into(),
            placeholder: String::new(),
            width: px(220.),
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
            on_select: None,
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

    /// The picked index; a [`ToolbarItem::Menu`]'s counts separators, so it indexes `entries`.
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

    /// Every keystroke in a [`ToolbarItem::Search`] field, and the cancel button's clear.
    pub fn on_change(mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        if let ToolbarItem::Search { on_change, .. } = &mut self {
            *on_change = Some(Rc::new(handler));
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

    fn identifier(&self) -> String {
        match self {
            ToolbarItem::Symbol { id, .. }
            | ToolbarItem::Group { id, .. }
            | ToolbarItem::Search { id, .. }
            | ToolbarItem::Menu { id, .. }
            | ToolbarItem::Share { id, .. }
            | ToolbarItem::Separator { id, .. }
            | ToolbarItem::View { id, .. } => id.clone(),
            ToolbarItem::FlexibleSpace => unsafe {
                NSToolbarFlexibleSpaceItemIdentifier.to_string()
            },
            ToolbarItem::Space => unsafe { NSToolbarSpaceItemIdentifier.to_string() },
        }
    }
}

struct ToolbarIvars {
    items: RefCell<Vec<ToolbarItem>>,
    built: RefCell<Vec<(String, Retained<NSToolbarItem>)>>,
    /// Weak on purpose: the window retains the toolbar, which retains this delegate.
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
            self.identifiers()
        }

        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item_for(
            &self,
            _: &NSToolbar,
            ident: &NSString,
            _: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            self.build_item(ident)
        }
    }

    unsafe impl NSSharingServicePickerDelegate for ToolbarDelegate {}

    unsafe impl NSSharingServicePickerToolbarItemDelegate for ToolbarDelegate {
        #[unsafe(method_id(itemsForSharingServicePickerToolbarItem:))]
        fn share_items(&self, item: &NSSharingServicePickerToolbarItem) -> Retained<NSArray> {
            let wanted = item.itemIdentifier().to_string();
            let texts = self
                .ivars()
                .items
                .borrow()
                .iter()
                .find_map(|spec| match spec {
                    ToolbarItem::Share { id, items, .. } if *id == wanted => Some(items.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let owned: Vec<Retained<NSString>> =
                texts.iter().map(|text| NSString::from_str(text)).collect();
            let refs: Vec<&AnyObject> = owned.iter().map(|text| text.as_ref()).collect();
            NSArray::from_slice(&refs)
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
            let handler = self.ivars().items.borrow().iter().find_map(|item| match item {
                ToolbarItem::Symbol { id, on_click, .. } if *id == wanted => on_click.clone(),
                _ => None,
            });
            let Some(handler) = handler else {
                return;
            };
            self.ivars()
                .cx
                .spawn(async move |cx| {
                    let _ = cx.update(|window, app| handler(window, app));
                })
                .detach();
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
            let handler = self.ivars().items.borrow().iter().find_map(|item| match item {
                ToolbarItem::Search {
                    id: found,
                    on_change,
                    ..
                } if *found == id => on_change.clone(),
                _ => None,
            });
            let Some(handler) = handler else {
                return;
            };
            self.ivars()
                .cx
                .spawn(async move |cx| {
                    let _ = cx.update(|window, app| handler(&text, window, app));
                })
                .detach();
        }

        #[unsafe(method(ngToolbarMenu:))]
        fn menu_action(&self, sender: *mut AnyObject) {
            let index: isize = unsafe { msg_send![sender, tag] };
            let menu: Option<Retained<NSMenu>> = unsafe { msg_send![sender, menu] };
            let Some(menu) = menu else {
                return;
            };
            let found = self.ivars().built.borrow().iter().find_map(|(id, item)| {
                let owned = item.downcast_ref::<NSMenuToolbarItem>()?.menu();
                ptr::eq(Retained::as_ptr(&owned), Retained::as_ptr(&menu)).then(|| id.clone())
            });
            if let (Some(id), Ok(index)) = (found, usize::try_from(index)) {
                self.dispatch_index(&id, index);
            }
        }
    }
);

fn identifier_array(ids: &[String]) -> Retained<NSArray<NSToolbarItemIdentifier>> {
    let owned: Vec<Retained<NSString>> = ids.iter().map(|id| NSString::from_str(id)).collect();
    let refs: Vec<&NSString> = owned.iter().map(|s| &**s).collect();
    NSArray::from_slice(&refs)
}

impl ToolbarDelegate {
    fn new(mtm: MainThreadMarker, window: &NSWindow, cx: AsyncWindowContext) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ToolbarIvars {
            items: RefCell::new(Vec::new()),
            built: RefCell::new(Vec::new()),
            window: Weak::new(window),
            cx,
        });
        unsafe { msg_send![super(this), init] }
    }

    fn identifiers(&self) -> Retained<NSArray<NSToolbarItemIdentifier>> {
        let items = self.ivars().items.borrow();
        let ids: Vec<String> = items.iter().map(ToolbarItem::identifier).collect();
        identifier_array(&ids)
    }

    /// The sender's position among the subitems, not `selectedIndex`, which is -1 for Momentary.
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
        let handler = self
            .ivars()
            .items
            .borrow()
            .iter()
            .find_map(|item| match item {
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
        let Some(handler) = handler else {
            return;
        };
        self.ivars()
            .cx
            .spawn(async move |cx| {
                let _ = cx.update(|window, app| handler(index, window, app));
            })
            .detach();
    }

    /// `None` for a standard spacer: those are AppKit's own items and it builds them itself.
    fn build_item(&self, ident: &NSString) -> Option<Retained<NSToolbarItem>> {
        let mtm = MainThreadMarker::new()?;
        let wanted = ident.to_string();
        let spec = self
            .ivars()
            .items
            .borrow()
            .iter()
            .find(|item| item.identifier() == wanted)
            .cloned()?;

        let item: Retained<NSToolbarItem> = match spec {
            ToolbarItem::Symbol {
                symbol,
                label,
                on_click,
                ..
            } => {
                let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), ident);
                item.setLabel(&NSString::from_str(&label));
                item.setToolTip(Some(&NSString::from_str(&label)));
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
                // The images constructor, not `subitems`: only it has a control to select in.
                let images: Vec<Retained<NSImage>> = segments
                    .iter()
                    .map(|segment| {
                        symbol_image(&segment.symbol, &segment.label).unwrap_or_default()
                    })
                    .collect();
                let image_refs: Vec<&NSImage> = images.iter().map(|image| &**image).collect();
                let labels: Vec<Retained<NSString>> = segments
                    .iter()
                    .map(|segment| NSString::from_str(&segment.label))
                    .collect();
                let label_refs: Vec<&NSString> = labels.iter().map(|text| &**text).collect();
                let group = unsafe {
                    NSToolbarItemGroup::groupWithItemIdentifier_images_selectionMode_labels_target_action(
                        ident,
                        &NSArray::from_slice(&image_refs),
                        selection.into(),
                        Some(&NSArray::from_slice(&label_refs)),
                        Some(self.as_ref()),
                        Some(sel!(ngToolbarAction:)),
                        mtm,
                    )
                };
                group.setControlRepresentation(width.into());
                if selection != ToolbarSelection::Momentary && selected < segments.len() {
                    group.setSelectedIndex(selected as isize);
                }
                group.setLabel(&NSString::from_str(&label));
                Retained::into_super(group)
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
                item.setLabel(&NSString::from_str(&label));
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
                ..
            } => {
                let item =
                    NSMenuToolbarItem::initWithItemIdentifier(NSMenuToolbarItem::alloc(mtm), ident);
                item.setLabel(&NSString::from_str(&label));
                item.setToolTip(Some(&NSString::from_str(&label)));
                item.setBordered(true);
                if let Some(image) = symbol_image(&symbol, &label) {
                    item.setImage(Some(&image));
                }
                // No action on the item: with one, only the chevron half would open the menu.
                item.setShowsIndicator(true);
                let menu = NSMenu::new(mtm);
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
                item.setLabel(&NSString::from_str(&label));
                item.setToolTip(Some(&NSString::from_str(&label)));
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
                item.setLabel(&NSString::from_str(&label));
                let view = build(mtm);
                let mut pinned: Vec<Retained<NSLayoutConstraint>> = Vec::new();
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
                    let refs: Vec<&NSLayoutConstraint> = pinned.iter().map(|pin| &**pin).collect();
                    NSLayoutConstraint::activateConstraints(&NSArray::from_slice(&refs));
                }
                item.setView(Some(&view));
                item
            }
            ToolbarItem::FlexibleSpace | ToolbarItem::Space => return None,
        };

        let mut built = self.ivars().built.borrow_mut();
        match built.iter_mut().find(|(id, _)| *id == wanted) {
            Some(slot) => slot.1 = item.clone(),
            None => built.push((wanted, item.clone())),
        }
        Some(item)
    }

    /// Push current state onto items AppKit already built; selection is diffed against the live one.
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
            match spec {
                ToolbarItem::Group {
                    segments,
                    selection,
                    selected,
                    width,
                    ..
                } => {
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
                    let current = field.placeholderString().map(|text| text.to_string());
                    if current.as_deref().unwrap_or("") != placeholder {
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
                _ => {}
            }
        }
    }

    /// The window's split view, which is what [`crate::sidebar::NativeSidebar`] installs.
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

/// Give the window a real toolbar, or take it away. Conflicts with `traffic_light_position`.
pub fn set_toolbar(
    window: &mut Window,
    cx: &mut App,
    style: Option<ToolbarStyle>,
    items: &[ToolbarItem],
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
    let identifiers: Vec<String> = items.iter().map(ToolbarItem::identifier).collect();

    let delegate = {
        let mut slot = host.ivars().toolbar_delegate.borrow_mut();
        slot.get_or_insert_with(|| ToolbarDelegate::new(mtm, &ns_window, window.to_async(cx)))
            .clone()
    };
    // Replaced every call: the handlers close over the caller's current state.
    *delegate.ivars().items.borrow_mut() = items.to_vec();

    let toolbar = {
        let mut slot = host.ivars().toolbar.borrow_mut();
        slot.get_or_insert_with(|| {
            let toolbar = NSToolbar::initWithIdentifier(
                NSToolbar::alloc(mtm),
                &NSString::from_str("ng.window_chrome.toolbar"),
            );
            toolbar.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*delegate)));
            toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
            toolbar.setAllowsUserCustomization(false);
            toolbar
        })
        .clone()
    };
    delegate.refresh();

    let unchanged = host
        .ivars()
        .toolbar_installed
        .borrow()
        .as_ref()
        .is_some_and(|(installed_style, installed)| {
            *installed_style == style && *installed == identifiers
        });
    if unchanged {
        return;
    }

    toolbar.setItemIdentifiers(&identifier_array(&identifiers));

    ns_window.setToolbarStyle(style.into());
    ns_window.setToolbar(Some(&toolbar));
    toolbar.setVisible(true);
    *host.ivars().toolbar_installed.borrow_mut() = Some((style, identifiers));
}

/// One line per built item: identifier, the AppKit class, visible or clipped, and class details.
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

/// The view a [`ToolbarItem::View`] built, so a caller can write new state into it.
pub fn toolbar_item_view(window: &Window, id: &str) -> Option<Retained<NSView>> {
    let ns_window = ns_window_of(window);
    ns_window
        .toolbar()?
        .items()
        .iter()
        .find(|item| item.itemIdentifier().to_string() == id)
        .and_then(|item| item.view())
}

/// Expand a [`ToolbarItem::Search`] to its preferred width and focus it, or collapse it back.
pub fn set_toolbar_search_active(window: &Window, id: &str, active: bool) {
    let ns_window = ns_window_of(window);
    let Some(toolbar) = ns_window.toolbar() else {
        return;
    };
    for item in toolbar.items().iter() {
        if item.itemIdentifier().to_string() != id {
            continue;
        }
        let Some(search) = item.downcast_ref::<NSSearchToolbarItem>() else {
            return;
        };
        if active {
            search.beginSearchInteraction();
        } else {
            search.endSearchInteraction();
        }
        return;
    }
}

/// How much of the window's height the titlebar and toolbar occupy, in points.
pub fn titlebar_band(window: &Window) -> Pixels {
    let ns_window = ns_window_of(window);
    let frame = ns_window.frame();
    // contentRectForFrameRect is the whole frame under FullSizeContentView; use contentLayoutRect.
    px((frame.size.height - ns_window.contentLayoutRect().size.height) as f32)
}

/// Where macOS has put the traffic lights, as an inset from the window frame's top-left.
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
/// Window levels, resolved through `CGWindowLevelForKey` rather than the SDK's baked constants.
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

/// The system's current value for a named window level.
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
/// Every window-level flag this crate touches: read it, change one field, write it back.
pub struct WindowFlags {
    pub has_shadow: bool,
    pub is_opaque: bool,
    pub alpha: f32,
    pub movable_by_background: bool,
    /// GPUI rewrites this on both fullscreen transitions, so a value set here does not survive one.
    pub titlebar_appears_transparent: bool,
    pub titled: bool,
    pub full_size_content: bool,
    pub level: isize,
    pub collection: NSWindowCollectionBehavior,
    /// `None` follows the system. Window-scoped, unlike [`tokens::set_app_appearance`].
    pub appearance: Option<tokens::Appearance>,
}

/// Restore this window's frame from the one AppKit saved under `name`, and keep saving it there
/// as the window moves and resizes.
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
    // Installs the chrome host, so `restore_window_flags` has a baseline even from a first call.
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

/// Put every flag back the way it was when this module first touched the window.
pub fn restore_window_flags(window: &Window) {
    let host = chrome(window);
    let borrowed = host.ivars().baseline.borrow();
    let Some(baseline) = borrowed.as_ref() else {
        return;
    };
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

/// Give a GPUI element a real system cursor over its own bounds, wrapping `child` transparently.
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

struct ClaimGuard {
    host: Retained<ChromeHost>,
    key: u64,
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        self.host.remove_claim(self.key);
    }
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
        let visible = bounds.intersect(&window.content_mask().bounds);
        if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
            return None;
        }
        Some(window.insert_hitbox(visible, HitboxBehavior::Normal))
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

        let (Some(global_id), Some(hitbox)) = (id, hitbox.as_ref()) else {
            return;
        };
        // Both mechanisms: gpui resets the cursor to Arrow at the end of every drawn frame.
        window.set_cursor_style(self.cursor.gpui(), hitbox);

        let host = chrome(window);
        let key = element_key(id);
        window.with_element_state::<ClaimGuard, ()>(global_id, |existing, _| {
            let guard = existing.unwrap_or_else(|| ClaimGuard {
                host: host.clone(),
                key,
            });
            ((), guard)
        });
        host.set_claim(key, native_rect(hitbox.bounds), self.cursor);
    }
}

impl IntoElement for CursorRegion {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Give a GPUI element the system tooltip over its own bounds, wrapping `child` transparently.
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

struct TipGuard {
    host: Retained<ChromeHost>,
    key: u64,
}

impl Drop for TipGuard {
    fn drop(&mut self) {
        self.host.remove_tip(self.key);
    }
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
        let visible = bounds.intersect(&window.content_mask().bounds);
        (visible.size.width > px(0.) && visible.size.height > px(0.)).then_some(visible)
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

        let Some(global_id) = id else {
            return;
        };
        let host = chrome(window);
        let key = element_key(id);
        window.with_element_state::<TipGuard, ()>(global_id, |existing, _| {
            let guard = existing.unwrap_or_else(|| TipGuard {
                host: host.clone(),
                key,
            });
            ((), guard)
        });
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

/// The current `NSCursor`'s class, hot spot and image size, for verifying a [`cursor_region`].
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
