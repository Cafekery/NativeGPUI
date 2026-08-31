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
    Point, Size, Window, px,
};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{AnyThread, ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSCursor, NSEvent, NSImage, NSLayoutConstraint, NSMenu,
    NSMenuToolbarItem, NSSearchField, NSSearchToolbarItem, NSSharingServicePickerDelegate,
    NSSharingServicePickerToolbarItem, NSSharingServicePickerToolbarItemDelegate, NSSplitView,
    NSSplitViewController, NSToolbar, NSToolbarDelegate, NSToolbarDisplayMode,
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
/// The shadow silhouette follows the window frame and this radius, and is blind to what you paint.
/// Measured in one session across three arms — radius never set, set to 24, and set to 0 after 24 —
/// where toggling a fully unpainted 32pt band changed 0 of 85,600 shadow-region pixels in all
/// three. Two positives on the same region the same session: `has_shadow` off moved 40,874, and
/// 16 -> 24 with identical content moved 10,496. After `0` the shadow is byte-identical to the
/// never-set arm, so 0 really does restore the system silhouette.
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

/// The two toolbar styles that keep a window looking like a document window. Both *take* GPUI
/// content: under [`crate::window_options`]'s `FullSizeContentView` the content rect **is** the
/// frame, so AppKit has no content height to preserve by growing the window. Measured at 820x560:
/// the frame never moves, while `contentLayoutRect` goes 528 -> 508 unified and -> 520 compact, so
/// unified costs 20pt and compact 8pt. Inset your chrome by the live band, never by a constant.
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
/// A handler for a control that reports which of its parts the user picked.
type ToolbarIndexHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;
/// A handler for a control that reports text.
type ToolbarTextHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;
/// Builds a [`ToolbarItem::View`]'s view, once, the first time the toolbar asks for that item.
type ToolbarViewBuilder = Rc<dyn Fn(MainThreadMarker) -> Retained<NSView>>;

/// One glyph of a [`ToolbarItem::group`] capsule.
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

/// How a [`ToolbarItem::Group`] tracks clicks.
///
/// AppKit honours this only on a group built by one of its own convenience constructors, which is
/// the only kind this crate builds — a group assembled from `subitems` has no control to select in.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum ToolbarSelection {
    /// Exactly one glyph stays lit. The view-mode picker.
    #[default]
    SelectOne,
    /// Any number lit at once. [`ToolbarItem::on_select`] reports one index and cannot describe
    /// that, so read the group back if you need the whole set.
    SelectAny,
    /// Nothing stays lit; each click is a press.
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

/// What a [`ToolbarItem::Group`] does when the toolbar runs out of room.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum ToolbarGroupWidth {
    /// Stay a capsule while there is room, then collapse into a menu. What Finder does.
    #[default]
    Automatic,
    /// Always a capsule, even when it means the toolbar scrolls items out.
    Expanded,
    /// Always a single button that opens a menu.
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

/// One toolbar item, which AppKit draws — so every one of these gets the current system treatment,
/// liquid glass included, without this crate describing what that looks like.
///
/// Every variant but [`ToolbarItem::View`] is a real `NSToolbarItem` subclass rather than a control
/// this crate hosts in a plain item, because the behaviour is in the subclass and is not
/// reachable from a view. [`ToolbarItem::Search`] is the clearest case: `NSSearchToolbarItem`
/// declares `view` as `NS_UNAVAILABLE` and owns its field, so the collapse-to-a-glyph and the
/// expand-and-shove-the-other-items-aside are unavailable to any caller supplying a view.
#[derive(Clone)]
pub enum ToolbarItem {
    Symbol {
        id: String,
        symbol: String,
        label: String,
        on_click: Option<ToolbarHandler>,
    },
    /// Several glyphs in one capsule, selected as a set: `NSToolbarItemGroup`.
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
    ///
    /// A row carrying a submenu or a custom view reports nothing through
    /// [`ToolbarItem::on_select`]: `setSubmenu:` installs AppKit's own action on that row, and
    /// replacing it would turn opening the submenu into dismissing the menu.
    Menu {
        id: String,
        symbol: String,
        label: String,
        entries: Vec<MenuEntry>,
        on_select: Option<ToolbarIndexHandler>,
    },
    /// The system share sheet: `NSSharingServicePickerToolbarItem`. The glyph, the picker and the
    /// service list are all macOS's.
    Share {
        id: String,
        label: String,
        /// Offered to the picker as text. Empty offers nothing, and the picker says so.
        items: Vec<String>,
    },
    /// A divider that stays lined up with a split view divider as the user drags it:
    /// `NSTrackingSeparatorToolbarItem`. Needs [`crate::sidebar::NativeSidebar`] installed — the split
    /// view it tracks is the window's content view controller, which is what a sidebar installs.
    Separator {
        id: String,
        /// Which divider, counting from the leading edge. A one-sidebar window has divider 0.
        divider: usize,
    },
    /// Any other AppKit view, built once and retained by the toolbar.
    ///
    /// The escape hatch, not the main road: the builder runs when AppKit asks for the item, which
    /// is not a paint pass, so the view cannot come from [`crate::NativeHost`]'s pool and none of
    /// the crate's own elements can be used here. What the caller gives up is the whole pooled
    /// path — GPUI-driven geometry, the per-frame property diff, and `on_change` plumbing — so the
    /// view has to own its own state and wire its own target/action.
    ///
    /// **Build-once.** `build` runs the first time AppKit asks for this identifier and never
    /// again, so a later `set_toolbar` carrying different state does not rebuild it and a closure
    /// that captured a value shows that value forever. Unlike the other variants, which
    /// [`set_toolbar`] re-pushes state onto every call, a view item is updated by asking for the
    /// view back with [`toolbar_item_view`] and writing to it.
    View {
        id: String,
        label: String,
        /// Pinned with real layout constraints, per dimension, and only where the value is
        /// positive: `NSToolbarItem`'s `minSize`/`maxSize` have been deprecated since macOS 12 in
        /// favour of the view measuring itself, and the header's own advice is to let it, so a
        /// control with an intrinsic size wants `Size::default()` here. A view *without* one gets
        /// no width at all — measured 0 wide for a bare `NSView`, whose
        /// `intrinsicContentSize` is `NSViewNoIntrinsicMetric` in both axes — and an item whose
        /// view is 0 wide is invisible while still occupying its place in the item order.
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

    /// `build` runs once per identifier, on the main thread, outside any paint pass.
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

    /// The picked index, for a [`ToolbarItem::Group`] or a [`ToolbarItem::Menu`]. A menu's index
    /// counts separators, so it indexes the `entries` slice directly.
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

    /// What a [`ToolbarItem::Search`] field widens to on focus.
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
    /// Every item this delegate has handed AppKit, by identifier.
    ///
    /// `NSToolbar` retains its items, but the sender of an action is a subitem, a search field or
    /// a menu row rather than the item itself, so the way back to the caller's handler is to
    /// recognise the sender against what was built. Also the retain for a
    /// [`ToolbarItem::View`]'s view, which nothing else owns.
    built: RefCell<Vec<(String, Retained<NSToolbarItem>)>>,
    /// Weak on purpose: the window retains its toolbar, which retains this delegate. A strong
    /// reference back would keep the window alive forever.
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
        /// Deferred: this can fire while GPUI holds its `App` borrow, which would drop the click.
        ///
        /// Sender is the item itself for a [`ToolbarItem::Symbol`], and for a
        /// [`ToolbarItem::Group`] it is one of the subitems AppKit synthesised — which inherits
        /// the group's action and carries an identifier of its own — so the group is found by
        /// asking each built group whether it owns the sender.
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

        /// Sender is the `NSSearchField` the item owns, so the text is read here and the item is
        /// recognised by which built search item owns that field.
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

        /// Sender is the `NSMenuItem`, whose tag is its row index — set by [`sync_menu`], which
        /// counts separators, so it indexes the caller's `entries` slice.
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
            match (found, usize::try_from(index)) {
                (Some(id), Ok(index)) => self.dispatch_index(&id, index),
                _ => {}
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

    /// The group that owns `sender_id` and the segment the click landed on, read here rather than
    /// in the deferred handler: the user can click another segment before the handler runs.
    ///
    /// The sender's position among the subitems, not `selectedIndex`: a
    /// [`ToolbarSelection::Momentary`] group holds no selection and reports -1 forever, so an
    /// index taken from the selection would drop every momentary click. `selectedIndex` is only
    /// consulted when the sender *is* the group, which is what AppKit sends when it has collapsed
    /// the capsule into a menu and there is no subitem to have been clicked.
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
    ///
    /// Rebuilt from scratch per identifier rather than diffed in place, unlike a pooled control:
    /// AppKit asks once per identifier per toolbar and keeps the item, so there is no per-frame
    /// path here to make cheap. What a re-`set_toolbar` with a changed item list costs is one
    /// construction of the items whose identifiers are new.
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
                // The images constructor and not `subitems`: `selectionMode` and the capsule only
                // exist on a group AppKit built its own control for.
                let images: Vec<Retained<NSImage>> = segments
                    .iter()
                    .map(|segment| {
                        symbol_image(&segment.symbol, &segment.label).unwrap_or_else(NSImage::new)
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
                // "The field should be configured before assigned": the item takes over the
                // field's layout on assignment, so target, action and placeholder go on first.
                let field = NSSearchField::new(mtm);
                if !placeholder.is_empty() {
                    field.setPlaceholderString(Some(&NSString::from_str(&placeholder)));
                }
                // Per keystroke rather than on Return only, which is what a filter field wants and
                // is not the default.
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
                item.setShowsIndicator(true);
                // No action on the item on purpose: with one, AppKit splits the control so the
                // glyph fires and only the chevron opens the menu. Finder's sort item opens on
                // either half.
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
                // Constraints, not `minSize`/`maxSize`: those have been deprecated since macOS 12.
                // A zero here means "measure yourself", which is what the header recommends and
                // what a control with an intrinsic size wants.
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

    /// Push the caller's current state onto the items AppKit has already built.
    ///
    /// [`ToolbarDelegate::build_item`] runs once per identifier and the toolbar keeps what it
    /// returned, so without this a re-`set_toolbar` carrying a new selection or new menu rows
    /// changes nothing on screen. Selection is diffed against the live value on purpose — the user
    /// is the other writer, and a blind write would fight a click.
    ///
    /// A [`ToolbarItem::Search`] field's *text* is deliberately not pushed: the item owns the
    /// field and its editing session, and writing the string back per call would fight the user
    /// mid-word. Read it from `on_change`.
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

    /// The window's split view, which is what [`crate::sidebar::NativeSidebar`] installs as the content
    /// view controller. `None` when there is no sidebar, so a [`ToolbarItem::Separator`] with
    /// nothing to track is dropped rather than tracking a divider that does not exist.
    fn split_view(&self) -> Option<Retained<NSSplitView>> {
        let window = self.ivars().window.load()?;
        let controller = window
            .contentViewController()?
            .downcast::<NSSplitViewController>()
            .ok()?;
        Some(controller.splitView())
    }
}

/// An unknown name resolves to no image rather than a placeholder, so a typo degrades to a blank
/// item that looks deliberate. Gated on [`tokens::sf_symbol`] so the name is checked once.
fn symbol_image(symbol: &str, label: &str) -> Option<Retained<NSImage>> {
    tokens::sf_symbol(symbol, tokens::system_font_size(), FontWeight::NORMAL)?;
    NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(symbol),
        Some(&NSString::from_str(label)),
    )
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
        slot.get_or_insert_with(|| ToolbarDelegate::new(mtm, &ns_window, window.to_async(cx)))
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

/// What the toolbar actually built, one line per item: identifier, the AppKit class AppKit ended
/// up with, whether it is currently visible rather than clipped, and whatever that class can be
/// asked for.
///
/// The instrument for a [`ToolbarItem`], and it has to read back from `NSToolbar` rather than from
/// the item list: whether a `Group` stayed a capsule or collapsed into a menu, and whether the
/// search field is wide or a glyph, are decisions AppKit makes from the width available.
///
/// A plain item and a group report no frame — the control belongs to AppKit and `view` is nil on
/// both, including on a group built by the convenience constructor that the header says has one.
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
        let shown = if visible.iter().any(|found| *found == id) {
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
///
/// The update route for the one variant [`set_toolbar`] cannot refresh: its builder runs once per
/// identifier and the toolbar keeps what it returned, so there is nothing to re-run. The other
/// variants need this for nothing — a group's selection, a search field's placeholder and width,
/// and a menu's rows are all pushed on every `set_toolbar` call.
///
/// `None` when there is no toolbar, no item by that identifier, or the item is not a view item.
/// `NSSearchToolbarItem` reports `None` too: it declares `view` unavailable and owns its field.
pub fn toolbar_item_view(window: &Window, id: &str) -> Option<Retained<NSView>> {
    let ns_window = ns_window_of(window);
    ns_window
        .toolbar()?
        .items()
        .iter()
        .find(|item| item.itemIdentifier().to_string() == id)
        .and_then(|item| item.view())
}

/// Expand a [`ToolbarItem::Search`] to its preferred width and put the keyboard in it, or collapse
/// it back to the magnifier. What clicking the glyph does, for a caller wiring ⌘F.
///
/// The expansion is `NSSearchToolbarItem`'s own, and it is a real width change rather than a
/// styling one: with `width` at 240pt the field goes 37x36 collapsed — a magnifier in a circle —
/// to 246x36 expanded. Whether the items beside it survive that is the toolbar's call, not this
/// one's: measured with ten items, an 820pt window keeps every item `visible`, while a 560pt
/// window already clips two collapsed and five expanded. `false` returns it to the glyph, and so
/// does a window resize.
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
