//! The window itself: its corners, toolbar, flags, cursor, and the coordinate recipe every
//! transient native surface anchors through: [`native_rect`] → [`native_window_rect`] →
//! [`native_screen_rect`]. The y-flip happens once, inside `convertRect:toView:nil`; a
//! hand-rolled `contentHeight - y - h` breaks the moment a toolbar changes the content height.

use std::cell::{Cell, RefCell};
use std::panic::Location;
use std::ptr;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AsyncWindowContext, Bounds, CursorStyle, Element, ElementId, FontWeight,
    GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, Pixels,
    Point, Window, px,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{AnyThread, ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSCursor, NSEvent, NSImage, NSToolbar, NSToolbarDelegate,
    NSToolbarDisplayMode, NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem,
    NSToolbarItemIdentifier, NSToolbarSpaceItemIdentifier, NSTrackingArea, NSTrackingAreaOptions,
    NSView, NSWindow, NSWindowButton, NSWindowCollectionBehavior, NSWindowLevel,
    NSWindowOrderingMode, NSWindowStyleMask, NSWindowToolbarStyle,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use crate::bridge::appkit::{gpui_view, to_rect};
use crate::bridge::host::element_key;
use crate::tokens;

/// A GPUI element's `bounds` as an `NSRect` in the chrome host's coordinate space.
///
/// The identity function: [`chrome_host`] is flipped, so GPUI's logical top-left points *are*
/// AppKit points there, and fractional bounds pass through unrounded — only `initWithFrame:`
/// rounds. This is the rect for `showRelativeToRect:ofView:preferredEdge:`, where a flipped
/// positioning view makes MinY render above it.
pub fn native_rect(bounds: Bounds<Pixels>) -> NSRect {
    to_rect(bounds)
}

/// A GPUI element's `bounds` in window base coordinates.
pub fn native_window_rect(host: &NSView, bounds: Bounds<Pixels>) -> NSRect {
    host.convertRect_toView(native_rect(bounds), None)
}

/// A GPUI element's `bounds` in screen coordinates — for anchoring a window or panel, which has no
/// positioning view to be relative to.
///
/// # Panics
///
/// If `host` is not in a window, which is impossible on the paint path.
pub fn native_screen_rect(host: &NSView, bounds: Bounds<Pixels>) -> NSRect {
    let window = host
        .window()
        .expect("native_screen_rect: host view is not in a window");
    window.convertRectToScreen(native_window_rect(host, bounds))
}

/// The flipped view that owns this window's chrome: the positioning view for transient surfaces
/// and the owner of every [`cursor_region`]. Installed lazily as the **bottom** subview of GPUI's
/// Metal view and autoresized to fill it, so a real hosted control always wins hit-testing;
/// GPUI's own Metal view is not flipped, so it is no substitute.
pub fn chrome_host(window: &Window) -> Retained<NSView> {
    Retained::into_super(chrome(window))
}

#[derive(Copy, Clone, PartialEq)]
struct Claim {
    key: u64,
    rect: NSRect,
    cursor: Cursor,
}

#[derive(Copy, Clone)]
struct Corner {
    supported: bool,
    system: f64,
    /// `_setCornerRadius: 0` means "system default", not square, and reads back as that default
    /// (16 on a bare window) rather than 0, so intent is tracked here, never diffed against a
    /// readback.
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
    corner: Cell<Corner>,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    toolbar: RefCell<Option<Retained<NSToolbar>>>,
    toolbar_delegate: RefCell<Option<Retained<ToolbarDelegate>>>,
    /// So a re-`set_toolbar` with the same shape does not churn the window frame.
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

        /// `hitTest:` arrives in the **superview's** space, so the point comes back through
        /// `convertPoint:fromView:` first. Returning `self` makes AppKit consult its cursor rects.
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

        /// One rect per claim: `cursorUpdate:` alone cannot switch between adjacent regions.
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
            corner: Cell::new(corner),
            tracking: RefCell::new(None),
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

    fn claim_at(&self, point: NSPoint) -> Option<Claim> {
        self.ivars()
            .claims
            .borrow()
            .iter()
            .rev()
            .find(|claim| {
                point.x >= claim.rect.origin.x
                    && point.x <= claim.rect.origin.x + claim.rect.size.width
                    && point.y >= claim.rect.origin.y
                    && point.y <= claim.rect.origin.y + claim.rect.size.height
            })
            .copied()
    }

    /// `InVisibleRect` follows resizes. Installed on the first claim, never removed.
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

    fn invalidate_cursor_rects(&self) {
        if let Some(window) = self.window() {
            window.invalidateCursorRectsForView(self);
        }
    }
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

/// Whether this macOS has the private setter [`set_corner_radius`] drives; when false, that call
/// is a no-op. Gated rather than sent blind: an `msg_send!` to a missing selector raises
/// `NSInvalidArgumentException`.
pub fn supports_corner_radius(window: &Window) -> bool {
    chrome(window).ivars().corner.get().supported
}

/// What the OS would round this window's corners to right now, if nobody overrode it. Not a
/// constant even for one window — it is half the current [`titlebar_band`] — and after
/// [`set_corner_radius`] writes, this reports the last value seen.
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

/// Round this window's real corners, so the WindowServer regenerates the drop shadow to match.
/// Not `layer.cornerRadius`: that leaves the window's own shape square, so the shadow stays square
/// and `masksToBounds` clips content away.
///
/// `None` follows the OS by sending `_setCornerRadius: 0`, which AppKit reads as the
/// system default rather than square, so `Some(px(0.))` folds into `None` and this cannot make a
/// square corner. Idempotent against the last value written, never a readback: safe every frame.
///
/// On a transparent window, what [`crate::window_options`] ships, the shadow derives from
/// content alpha rather than the window frame: an unpainted band or glass hole reaching a corner
/// gives a rounded window and a square shadow.
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

/// `_cornerRadius` as the window reports it *now*. Diagnostics only: after
/// `set_corner_radius(window, None)` this reads back the system default rather than 0, so a loop
/// that diffs it against intent writes forever. Use [`corner_radius`].
pub fn corner_radius_readback(window: &Window) -> Option<Pixels> {
    let ns_window = ns_window_of(window);
    if !ns_window.respondsToSelector(sel!(_cornerRadius)) {
        return None;
    }
    let radius: f64 = unsafe { msg_send![&*ns_window, _cornerRadius] };
    Some(px(radius as f32))
}

/// The two toolbar styles that keep a window looking like a document window. Both *give* GPUI
/// content rather than taking any: AppKit grows the window frame by the whole new titlebar height,
/// and GPUI's view autoresizes into the extra space.
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

/// One toolbar item. Symbol items are drawn by AppKit, so they get genuine liquid-glass treatment.
#[derive(Clone)]
pub enum ToolbarItem {
    Symbol {
        id: String,
        symbol: String,
        label: String,
        on_click: Option<ToolbarHandler>,
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

    fn identifier(&self) -> String {
        match self {
            ToolbarItem::Symbol { id, .. } => id.clone(),
            ToolbarItem::FlexibleSpace => unsafe {
                NSToolbarFlexibleSpaceItemIdentifier.to_string()
            },
            ToolbarItem::Space => unsafe { NSToolbarSpaceItemIdentifier.to_string() },
        }
    }
}

struct ToolbarIvars {
    items: RefCell<Vec<ToolbarItem>>,
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

    impl ToolbarDelegate {
        /// Deferred: this can fire while GPUI holds its `App` borrow, which would drop the click.
        #[unsafe(method(ngToolbarAction:))]
        fn action(&self, sender: *mut AnyObject) {
            let ident: Retained<NSString> = unsafe { msg_send![sender, itemIdentifier] };
            let wanted = ident.to_string();
            let handler = self.ivars().items.borrow().iter().find_map(|item| match item {
                ToolbarItem::Symbol { id, on_click, .. } if *id == wanted => on_click.clone(),
                ToolbarItem::Symbol { .. } => None,
                ToolbarItem::FlexibleSpace | ToolbarItem::Space => None,
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
    }
);

fn identifier_array(ids: &[String]) -> Retained<NSArray<NSToolbarItemIdentifier>> {
    let owned: Vec<Retained<NSString>> = ids.iter().map(|id| NSString::from_str(id)).collect();
    let refs: Vec<&NSString> = owned.iter().map(|s| &**s).collect();
    NSArray::from_slice(&refs)
}

impl ToolbarDelegate {
    fn new(mtm: MainThreadMarker, cx: AsyncWindowContext) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ToolbarIvars {
            items: RefCell::new(Vec::new()),
            cx,
        });
        unsafe { msg_send![super(this), init] }
    }

    fn identifiers(&self) -> Retained<NSArray<NSToolbarItemIdentifier>> {
        let items = self.ivars().items.borrow();
        let ids: Vec<String> = items.iter().map(ToolbarItem::identifier).collect();
        identifier_array(&ids)
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
        let ToolbarItem::Symbol {
            symbol,
            label,
            on_click,
            ..
        } = spec
        else {
            return None;
        };

        let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), ident);
        item.setLabel(&NSString::from_str(&label));
        item.setToolTip(Some(&NSString::from_str(&label)));
        item.setBordered(true);
        if tokens::sf_symbol(&symbol, tokens::system_font_size(), FontWeight::NORMAL).is_some() {
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(&symbol),
                Some(&NSString::from_str(&label)),
            ) {
                item.setImage(Some(&image));
            }
        }
        if on_click.is_some() {
            unsafe {
                item.setTarget(Some(self.as_ref()));
                item.setAction(Some(sel!(ngToolbarAction:)));
            }
        }
        Some(item)
    }
}

/// Give the window a real macOS toolbar, or take it away. `None` restores the original window
/// geometry exactly, traffic-light insets included.
///
/// **Mutually exclusive with `TitlebarOptions::traffic_light_position`.** With that set, GPUI's
/// `move_traffic_light()` hard-sets all three button frames on open, activation and fullscreen,
/// overwriting the insets AppKit applied for the toolbar. Nothing warns, the buttons just sit
/// wrong.
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
        slot.get_or_insert_with(|| ToolbarDelegate::new(mtm, window.to_async(cx)))
            .clone()
    };
    // Handlers are replaced every call: they close over the caller's current state.
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

/// How much of the window's height the titlebar and toolbar occupy, in points — the amount to
/// inset GPUI content by so nothing sits under the band.
///
/// Must come from `contentLayoutRect`: under `FullSizeContentView`, which is what
/// [`crate::window_options`] ships, the content rect *is* the frame, so the obvious
/// `frame.height - contentRectForFrameRect(frame).height` reads a constant 0.
pub fn titlebar_band(window: &Window) -> Pixels {
    let ns_window = ns_window_of(window);
    let frame = ns_window.frame();
    px((frame.size.height - ns_window.contentLayoutRect().size.height) as f32)
}

/// Where macOS has put the traffic lights, as an inset from the window frame's top-left, in GPUI's
/// coordinate sense. `None` when the window has no close button.
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

/// The window levels, resolved through `CGWindowLevelForKey` rather than written down — AppKit's
/// own `NSFloatingWindowLevel` is a compile-time constant baked out of the SDK header.
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

/// Every window-level flag this crate touches, in one struct: read it, change one field, write it
/// back without losing the others. Build a level with [`window_level`].
///
/// Clearing `titled` or setting `full_size_content` shrinks the frame and moves or removes the
/// traffic lights. `collection` and `level` are the real AppKit types on purpose: modelling their
/// bits as named bools would silently drop the ones this crate did not think of.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WindowFlags {
    pub has_shadow: bool,
    pub is_opaque: bool,
    pub alpha: f32,
    pub movable_by_background: bool,
    /// GPUI also writes this on both fullscreen transitions, so a value set here does not survive
    /// entering or leaving fullscreen.
    pub titlebar_appears_transparent: bool,
    pub titled: bool,
    pub full_size_content: bool,
    pub level: isize,
    pub collection: NSWindowCollectionBehavior,
    /// `None` follows the system. Window-scoped, unlike [`tokens::set_app_appearance`].
    pub appearance: Option<tokens::Appearance>,
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

/// Write all of them. Read [`window_flags`], change what you mean, write it back.
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

/// The system cursors, restricted to those GPUI can also name: a [`cursor_region`] drives both
/// mechanisms, and a cursor only one of them knows would flicker.
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
///
/// Drives two mechanisms on purpose: an AppKit cursor rect alone loses the race, because gpui's
/// `Window::reset_cursor_style` runs at the end of every drawn frame and sets `Arrow` when no GPUI
/// element asked for anything. The claim is clipped to the current content mask, so a region
/// inside a scroll container stops at the container's edge, and it is released when the element
/// stops painting.
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

/// Lives in GPUI's element state, so GPUI's own lifecycle releases the claim.
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

/// The selector [`set_corner_radius`] gates on, for a caller with no `Window` in hand.
pub fn corner_radius_selector() -> Sel {
    sel!(_setCornerRadius:)
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
