//! A system-drawn macOS sidebar.
//!
//! `NSSplitViewItem.sidebar` is a property of the *window's* view hierarchy, not of a rectangle
//! inside it, so unlike everything else in this crate this is a window shell: install once, then
//! update per frame.
//!
//! ```ignore
//! let sidebar = NativeSidebar::install(window, cx, SidebarOptions::default());
//! sidebar.update(&items);
//! sidebar.set_selected(self.route.as_deref());
//! ```
//!
//! Rows are real [`NSTableCellView`]s, so the source-list font, insets, indentation, disclosure
//! control, icon box and the label-colour flip under selection are AppKit's — the cell's own
//! `layout` places only the count badge, which `NSTableCellView` has no outlet for. Row height,
//! icon size and label font come as one class from [`SidebarRowSize`], which defaults to what the
//! running macOS calls standard. Two rules for the host:
//!
//! * The window must be non-opaque ([`crate::window_options`] does this): the sidebar's material
//!   is `behindWindow`, and on an opaque window it silently renders flat grey.
//! * GPUI's coordinate space still spans the whole window, but the sidebar strip is not yours —
//!   read [`SidebarMetrics`] for the gutter, paint no background over it, and keep interactive
//!   targets 3pt clear of the pane's left edge: a click 1-2pt in never reaches GPUI.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui::{AnyWindowHandle, App, AsyncWindowContext, Global, Pixels, SharedString, Window, px};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezierPath, NSBundleImageExtension, NSButton, NSCellImagePosition,
    NSColor, NSControlTextEditingDelegate, NSDragOperation, NSDraggingInfo, NSFont,
    NSFontWeightRegular, NSImage, NSImageAlignment, NSImageScaling, NSImageSymbolConfiguration,
    NSImageView, NSLineBreakMode, NSOutlineView, NSOutlineViewDataSource, NSOutlineViewDelegate,
    NSOutlineViewDropOnItemIndex, NSPasteboardItem, NSPasteboardWriting, NSScreen, NSScrollView,
    NSSplitViewController, NSSplitViewItem, NSTableCellView, NSTableColumn,
    NSTableColumnResizingOptions, NSTableViewAnimationOptions, NSTableViewColumnAutoresizingStyle,
    NSTableViewRowSizeStyle, NSTableViewStyle, NSTextAlignment, NSTextField, NSToolbar,
    NSToolbarDelegate, NSToolbarDisplayMode, NSToolbarToggleSidebarItemIdentifier,
    NSUserInterfaceItemIdentification, NSView, NSViewController, NSVisualEffectBlendingMode,
    NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowOrderingMode, NSWindowStyleMask,
    NSWindowToolbarStyle,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSBundle, NSIndexSet, NSInteger, NSMutableIndexSet, NSNotification,
    NSNumber, NSObject, NSPoint, NSRect, NSSize, NSString,
};

use crate::Material;
use crate::bridge::appkit::{Container, gpui_view, rects_equal};
use crate::materials::RawMaterial;
use crate::tokens;

/// Gap between a row's label and its trailing count badge, in points. The one hand-chosen number
/// left in a row: AppKit publishes a token for the icon's box and its gap (see
/// [`SidebarRowSize`]) but none for a badge, which `NSTableCellView` has no outlet for.
pub const BADGE_GAP: f64 = 6.0;

/// Where the system keeps the sidebar artwork Finder draws for the rows SF Symbols do not cover.
/// World-readable, no entitlement, and its images are monochrome and templatable.
pub const CORE_TYPES_BUNDLE: &str = "/System/Library/CoreServices/CoreTypes.bundle";

/// Where a row's glyph comes from.
///
/// [`Symbol`](Self::Symbol) is the answer for nearly every row, and that is a measurement rather
/// than a preference: scored against a capture of the real thing, macOS 26's Finder draws SF
/// Symbols for most of its own sidebar — `hammer` 0.945, `house` 0.975, `trash` 0.974,
/// `network` 0.864 — and reaches for Apple's own artwork only where the public catalogue has
/// nothing. Reach for [`SystemResource`](Self::SystemResource) on that evidence, per row, never as
/// a blanket rule: a whole sidebar drawn from [`CORE_TYPES_BUNDLE`] is a set Finder has mostly
/// abandoned and scores *worse* — `compass.drawing` is the wrong glyph for Applications, but
/// `SidebarDocumentsFolder` is equally wrong for Documents at 0.076 against `doc`'s 0.802.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SidebarIcon {
    /// An SF Symbol name. One the running macOS does not know renders as no icon.
    Symbol(SharedString),
    /// A monochrome image resource in [`CORE_TYPES_BUNDLE`], drawn as a template so the row's
    /// selection colours still reach it.
    SystemResource(SharedString),
}

/// One row. Build with [`sidebar_item`] or [`sidebar_group`].
#[derive(Clone, Debug)]
pub struct SidebarItem {
    id: SharedString,
    title: SharedString,
    icon: Option<SidebarIcon>,
    badge: Option<i64>,
    eject: bool,
    group: bool,
    children: Vec<SidebarItem>,
}

/// A selectable row.
pub fn sidebar_item(id: impl Into<SharedString>, title: impl Into<SharedString>) -> SidebarItem {
    SidebarItem {
        id: id.into(),
        title: title.into(),
        icon: None,
        badge: None,
        eject: false,
        group: false,
        children: Vec::new(),
    }
}

/// A non-selectable group header — `isGroupItem:` is what makes the tree read as a source list.
pub fn sidebar_group(id: impl Into<SharedString>, title: impl Into<SharedString>) -> SidebarItem {
    SidebarItem {
        group: true,
        ..sidebar_item(id, title)
    }
}

impl SidebarItem {
    /// An SF Symbol name. One the running macOS does not know renders as no icon.
    pub fn symbol(self, name: impl Into<SharedString>) -> Self {
        self.icon(SidebarIcon::Symbol(name.into()))
    }

    /// A [`CORE_TYPES_BUNDLE`] resource, e.g. `SidebarAirDrop`.
    pub fn system_image(self, resource: impl Into<SharedString>) -> Self {
        self.icon(SidebarIcon::SystemResource(resource.into()))
    }

    pub fn icon(mut self, icon: SidebarIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Show the trailing eject affordance a mounted volume has. Clicks arrive at
    /// [`NativeSidebar::on_eject`]; the sidebar itself unmounts nothing.
    pub fn ejectable(mut self, ejectable: bool) -> Self {
        self.eject = ejectable;
        self
    }

    pub fn badge(mut self, count: i64) -> Self {
        self.badge = Some(count);
        self
    }

    pub fn child(mut self, item: SidebarItem) -> Self {
        self.children.push(item);
        self
    }

    pub fn children(mut self, items: impl IntoIterator<Item = SidebarItem>) -> Self {
        self.children.extend(items);
        self
    }
}

fn key_of(id: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

struct Node {
    key: u64,
    id: SharedString,
    title: SharedString,
    icon: Option<SidebarIcon>,
    badge: Option<i64>,
    eject: bool,
    group: bool,
    parent: Option<usize>,
    children: Vec<usize>,
}

impl Node {
    /// Row appearance only — `children.is_empty()` is here because it draws the disclosure arrow.
    fn content(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.title.hash(&mut hasher);
        self.icon.hash(&mut hasher);
        self.badge.hash(&mut hasher);
        self.eject.hash(&mut hasher);
        self.group.hash(&mut hasher);
        self.children.is_empty().hash(&mut hasher);
        hasher.finish()
    }
}

#[derive(Default)]
struct Model {
    nodes: Vec<Node>,
    roots: Vec<usize>,
    by_key: HashMap<u64, usize>,
}

impl Model {
    fn build(items: &[SidebarItem]) -> Self {
        let mut model = Model::default();
        model.roots = model.push_all(items, None);
        model
    }

    fn push_all(&mut self, items: &[SidebarItem], parent: Option<usize>) -> Vec<usize> {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let index = self.nodes.len();
            let key = key_of(&item.id);
            self.nodes.push(Node {
                key,
                id: item.id.clone(),
                title: item.title.clone(),
                icon: item.icon.clone(),
                badge: item.badge,
                eject: item.eject,
                group: item.group,
                parent,
                children: Vec::new(),
            });
            debug_assert!(
                !self.by_key.contains_key(&key),
                "sidebar item ids must be unique across the whole tree; `{}` is repeated",
                item.id
            );
            self.by_key.insert(key, index);
            let children = self.push_all(&item.children, Some(index));
            self.nodes[index].children = children;
            out.push(index);
        }
        out
    }

    fn node(&self, key: u64) -> Option<&Node> {
        self.by_key.get(&key).map(|&i| &self.nodes[i])
    }

    fn children_keys(&self, parent: Option<u64>) -> Vec<u64> {
        let indices = match parent {
            None => &self.roots,
            Some(key) => match self.by_key.get(&key) {
                Some(&i) => &self.nodes[i].children,
                None => return Vec::new(),
            },
        };
        indices.iter().map(|&i| self.nodes[i].key).collect()
    }

    fn parent_key(&self, key: u64) -> Option<u64> {
        let index = *self.by_key.get(&key)?;
        self.nodes[index].parent.map(|p| self.nodes[p].key)
    }

    /// Move a child within its own parent. `to` is the index it should end up at *after* the
    /// removal, which is the convention `moveItemAtIndex:inParent:toIndex:inParent:` uses, so the
    /// model and AppKit take the same number.
    fn move_child(&mut self, parent: Option<u64>, from: usize, to: usize) -> bool {
        let parent_index = match parent {
            None => None,
            Some(key) => match self.by_key.get(&key) {
                Some(&index) => Some(index),
                None => return false,
            },
        };
        let list = match parent_index {
            None => &mut self.roots,
            Some(index) => &mut self.nodes[index].children,
        };
        if from >= list.len() || to >= list.len() {
            return false;
        }
        let moved = list.remove(from);
        list.insert(to, moved);
        true
    }

    /// Parents before children — `apply_diff` reloads everything if a child is visited first.
    fn preorder(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<usize> = self.roots.iter().rev().copied().collect();
        while let Some(index) = stack.pop() {
            out.push(self.nodes[index].key);
            for &child in self.nodes[index].children.iter().rev() {
                stack.push(child);
            }
        }
        out
    }

    fn id_for(&self, key: u64) -> Option<SharedString> {
        self.node(key).map(|n| n.id.clone())
    }

    fn key_for_id(&self, id: &str) -> Option<u64> {
        let key = key_of(id);
        self.by_key.contains_key(&key).then_some(key)
    }
}

/// How GPUI's view is framed inside the content pane.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum PaneAlign {
    /// The shipped framing: GPUI keeps the window's whole coordinate space.
    #[default]
    WindowAligned,
    /// Sizes GPUI's view to fill the pane. Broken by construction — the control arm
    /// `examples/sidebar_probe.rs` measures against under `NG_ARM=fill`, never a mode to ship.
    FillPane,
}

/// Row metrics class, `NSTableViewRowSizeStyle`.
///
/// One knob for four numbers: `NSTableCellView` lays out its `textField` and `imageView` outlets
/// from this, so it drives row height, the icon's box, the label font and the group-header font
/// together. Measured on 26.5, source-list rows:
///
/// | | row | label | group header |
/// |---|---|---|---|
/// | [`Small`](Self::Small) | 24pt | 11pt | 11pt |
/// | [`Medium`](Self::Medium) | 32pt | 13pt | 11pt |
/// | [`Large`](Self::Large) | 40pt | 15pt | 11pt |
///
/// `NSTableViewRowSizeStyleCustom`, AppKit's own property default, is deliberately absent: it
/// means "use `rowHeight` and lay out nothing", and `.sourceList` seeds `rowHeight` to 24 — so a
/// sidebar that never sets this property renders `Small`-height rows around a `Medium` label and
/// a group header three sizes too big. That hybrid is not one AppKit ships.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum SidebarRowSize {
    /// `NSTableViewRowSizeStyleDefault` — whatever the running macOS calls standard for a source
    /// list, which is what Finder gets. Resolves to [`Medium`](Self::Medium) on 26.5; read
    /// [`SidebarProbe::effective_row_size_style`] for what it resolved to.
    #[default]
    System,
    Small,
    Medium,
    Large,
}

impl SidebarRowSize {
    fn appkit(self) -> NSTableViewRowSizeStyle {
        match self {
            Self::System => NSTableViewRowSizeStyle::Default,
            Self::Small => NSTableViewRowSizeStyle::Small,
            Self::Medium => NSTableViewRowSizeStyle::Medium,
            Self::Large => NSTableViewRowSizeStyle::Large,
        }
    }

    /// Height of the icon box `NSTableCellView` lays out for this class, in points.
    ///
    /// Read off a laid-out source-list cell on 26.5: the image view comes out 20x15 at
    /// [`Small`](Self::Small), 23x19 at [`Medium`](Self::Medium), 27x21 at [`Large`](Self::Large).
    /// The box is wider than it is tall, so the height is what a square glyph gets to use.
    fn icon_points(style: NSTableViewRowSizeStyle) -> f64 {
        match style {
            NSTableViewRowSizeStyle::Small => 15.,
            NSTableViewRowSizeStyle::Large => 21.,
            _ => 19.,
        }
    }
}

/// Configuration honoured on the first [`NativeSidebar::install`] for a window.
#[derive(Clone, Debug)]
pub struct SidebarOptions {
    /// Requested thickness. AppKit rounds it to its own standard width.
    pub thickness: Pixels,
    pub min_thickness: Pixels,
    pub max_thickness: Pixels,
    /// Attach a unified `NSToolbar` carrying the system sidebar-toggle item.
    pub toolbar: bool,
    pub collapsed: bool,
    /// Expand every group on install and on insertion.
    pub expand_groups: bool,
    /// Row height, icon box and label font, as one class. See [`SidebarRowSize`].
    pub row_size: SidebarRowSize,
    pub align: PaneAlign,
    /// Re-parent [`crate::Layer::Below`] containers into the content pane and mirror GPUI's frame
    /// onto them. Without it `setContentViewController:` detaches them from the window.
    pub adopt_below_layer: bool,
    /// Let the user drag a row to a new place among its own siblings, reported through
    /// [`NativeSidebar::on_reorder`]. Cross-group drops are refused, which is what Finder does.
    ///
    /// This changes when a plain click takes effect, and that is AppKit's doing rather than a
    /// choice made here: a table whose data source merely **defines**
    /// `outlineView:pasteboardWriterForItem:` stops committing the selection on mouse-down and
    /// commits it on mouse-up instead — measured both ways, and true even when the method returns
    /// nil, because AppKit asks `respondsToSelector:` and never calls it. So a sidebar that can be
    /// dragged does not light up under a held finger, and one that cannot does. The option exists
    /// to keep that from being imposed on a caller who wanted a plain list.
    pub reorderable: bool,
    /// A window-spanning backdrop, inserted beneath both panes. **This is the sidebar's vibrancy,
    /// not the window's background** — the content pane paints [`tokens::WINDOW_BACKGROUND`] over
    /// it, so only the sidebar strip is left showing it.
    ///
    /// What a Mac window has behind its sidebar. Without it the sidebar samples whatever is
    /// behind the *window*, and `allowsFullHeightLayout` runs it up under a titlebar strip that
    /// is unpainted too; with it both sample the window's own backing and still read as vibrancy
    /// rather than a flat fill.
    ///
    /// It spans the window instead of living in a pane because GPUI's drawing is clipped to the
    /// content pane — a material placed from GPUI stops at the sidebar edge and squares off the
    /// window's rounded corners — and sits below the split view instead of being drawn by GPUI,
    /// whose view covers the traffic lights and toolbar under `NSFullSizeContentViewWindowMask`.
    ///
    /// Resist making it opaque to match the content side. It blends behind-window, so it is the
    /// one surface here that shows the desktop, and swapping it for an opaque material moved the
    /// sidebar 40 -> 27 in the band and 43 -> 32 in its interior — a flatter panel, and the step
    /// it was meant to close is closed at the pane instead.
    ///
    /// `None` leaves the window unbacked, for a caller painting its own opaque background.
    pub backdrop: Option<Material>,
}

impl Default for SidebarOptions {
    fn default() -> Self {
        Self {
            thickness: px(240.),
            min_thickness: px(180.),
            max_thickness: px(360.),
            toolbar: true,
            collapsed: false,
            expand_groups: true,
            row_size: SidebarRowSize::default(),
            align: PaneAlign::WindowAligned,
            adopt_below_layer: true,
            reorderable: false,
            backdrop: Some(Material::WindowBackground),
        }
    }
}

/// Insets the sidebar imposes on GPUI's layout, in GPUI-logical points.
///
/// A [`Global`], republished with a window refresh whenever AppKit moves the pane — which
/// includes every frame of the collapse animation.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct SidebarMetrics {
    pub left: Pixels,
    /// Pane top offset plus its `safeAreaInsets.top`, which is where a unified toolbar's height
    /// comes from instead of a constant.
    pub top: Pixels,
    pub right: Pixels,
    pub bottom: Pixels,
    /// Effective sidebar thickness, 0 when collapsed.
    pub thickness: Pixels,
    pub collapsed: bool,
}

impl Global for SidebarMetrics {}

/// What one [`NativeSidebar::update`] wrote to AppKit. A steady render loop must produce
/// `SidebarDiff::default()`.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct SidebarDiff {
    pub unchanged: bool,
    pub reloaded_items: usize,
    pub inserted: usize,
    pub removed: usize,
    pub moved: usize,
    /// 1 on the first push, since there is nothing to diff against. Afterwards it means the
    /// incremental path bailed out and reloaded.
    pub full_reloads: usize,
}

/// Live AppKit state, for callers that want to prove the sidebar is the system's.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct SidebarProbe {
    /// `NSSplitViewItemBehavior`; 1 is `Sidebar`.
    pub behavior: isize,
    pub allows_full_height_layout: bool,
    /// `NSTableViewStyle`; 3 is `SourceList`.
    pub style: isize,
    pub effective_style: isize,
    /// `NSTableViewRowSizeStyle` as set; -1 is `Default`, 0 `Custom`, 1..3 Small/Medium/Large.
    pub row_size_style: isize,
    /// What `Default` resolved to. Never -1, so this is the one to assert on.
    pub effective_row_size_style: isize,
    pub row_height: f64,
    pub indentation_per_level: f64,
    pub divider_thickness: f64,
    pub rows: isize,
    /// `NSVisualEffectMaterial` of the window-spanning backdrop, or -1 without one. Every
    /// material renders near-black over a dark desktop, so this readback — not a pixel mean — is
    /// what tells a caller the backdrop changed.
    pub backdrop_material: isize,
    pub toolbar_items: usize,
    pub scroll_draws_background: bool,
    pub outline_is_first_responder: bool,
}

/// Where AppKit placed the parts of one row. See [`NativeSidebar::row_layout`].
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct RowLayout {
    /// In the outline view.
    pub row: NSRect,
    /// In the outline view; `origin.x` is AppKit's own style inset.
    pub cell: NSRect,
    /// In the cell's coordinates.
    pub icon: NSRect,
    pub title: NSRect,
    pub badge: NSRect,
    pub level: isize,
    pub expanded: bool,
}

/// A reorder the user just performed. See [`NativeSidebar::on_reorder`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SidebarMove {
    /// The row that moved.
    pub id: SharedString,
    /// The group it belongs to; `None` for a root row.
    pub parent: Option<SharedString>,
    /// Its new index among its siblings.
    pub index: usize,
}

type SelectHandler = Rc<dyn Fn(&Option<SharedString>, &mut Window, &mut App)>;
type ReorderHandler = Rc<dyn Fn(&SidebarMove, &mut Window, &mut App)>;
type EjectHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

/// Shared with the outline view's data source and delegate.
///
/// Split from [`Views`] on purpose: `reloadData`, `endUpdates` and `expandItem:` all re-enter the
/// data source synchronously, so what the callbacks read must not be borrowed during a mutation.
struct Shared {
    model: RefCell<Model>,
    interned: RefCell<HashMap<u64, Retained<NSNumber>>>,
    on_select: RefCell<Option<SelectHandler>>,
    on_reorder: RefCell<Option<ReorderHandler>>,
    on_eject: RefCell<Option<EjectHandler>>,
    /// Non-zero while we drive the selection ourselves, so the round-trip does not echo.
    suppress: Cell<u32>,
    writes: Cell<u64>,
    cx: RefCell<AsyncWindowContext>,
    expand_groups: Cell<bool>,
    /// Set once the outline view exists, so the delegate needs no back-reference to [`Views`].
    outline: RefCell<Option<Retained<NSOutlineView>>>,
}

impl Shared {
    /// One retained `NSNumber` per key for the sidebar's life: `NSOutlineView` keys expansion and
    /// selection on item *identity*, so a fresh equal-but-distinct object would lose both.
    fn item(&self, key: u64) -> Retained<AnyObject> {
        let number = self
            .interned
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| NSNumber::new_i64(key as i64))
            .clone();
        unsafe { Retained::cast_unchecked::<AnyObject>(number) }
    }

    fn key_of_item(item: &AnyObject) -> u64 {
        let raw: i64 = unsafe { msg_send![item, longLongValue] };
        raw as u64
    }

    fn selected_id(&self) -> Option<SharedString> {
        let outline = self.outline.borrow().clone()?;
        let row = outline.selectedRow();
        if row < 0 {
            return None;
        }
        let item = outline.itemAtRow(row)?;
        self.model.borrow().id_for(Self::key_of_item(&item))
    }
}

/// The AppKit objects the shell owns. Retained for the window's lifetime.
struct Views {
    outline: Retained<NSOutlineView>,
    scroll: Retained<NSScrollView>,
    split: Retained<NSSplitViewController>,
    sidebar_item: Retained<NSSplitViewItem>,
    pane: Retained<ContentPane>,
    /// `setDataSource:`/`setDelegate:` are weak, so this must outlive the outline view. Typed as
    /// the common superclass because which of the two source classes it is depends on
    /// [`SidebarOptions::reorderable`].
    _source: Retained<NSObject>,
    /// Kept built even while detached, so [`NativeSidebar::set_toolbar`] can put it back.
    toolbar: Retained<NSToolbar>,
    _toolbar_delegate: Retained<SidebarToolbarDelegate>,
    _sidebar_vc: Retained<NSViewController>,
    _content_vc: Retained<NSViewController>,
    _content_item: Retained<NSSplitViewItem>,
    backdrop: Option<Retained<NSVisualEffectView>>,
}

struct SidebarState {
    views: Views,
    shared: Rc<Shared>,
    pane_shared: Rc<PaneShared>,
}

/// A handle on the window's sidebar. Cheap to clone; every clone is the same sidebar.
#[derive(Clone)]
pub struct NativeSidebar(Rc<SidebarState>);

#[derive(Default)]
struct Sidebars(HashMap<AnyWindowHandle, NativeSidebar>);
impl Global for Sidebars {}

impl NativeSidebar {
    /// Install the split-view shell into this window, or return the one already installed.
    ///
    /// Idempotent — a second call is a lookup and ignores `options` — so calling it
    /// unconditionally from `render` does not stack two split controllers.
    pub fn install(window: &mut Window, cx: &mut App, options: SidebarOptions) -> Self {
        if let Some(existing) = Self::get(window, cx) {
            return existing;
        }
        let sidebar = build(window, cx, options);
        cx.default_global::<Sidebars>()
            .0
            .insert(window.window_handle(), sidebar.clone());
        sidebar
    }

    pub fn get(window: &Window, cx: &App) -> Option<Self> {
        cx.try_global::<Sidebars>()?
            .0
            .get(&window.window_handle())
            .cloned()
    }

    /// Push the row model, diffed against what the outline view already shows.
    ///
    /// An unchanged model writes nothing at all, which is what lets selection, expansion and
    /// scroll offset survive a render loop running at the display's refresh rate.
    pub fn update(&self, items: &[SidebarItem]) -> SidebarDiff {
        let state = &*self.0;
        // The pane's own hooks catch everything AppKit does to the pane, but the host's
        // `Layer::Below` container appears in it a frame after `install` with the pane unmoved.
        state.pane_shared.clone().align(&state.views.pane);

        let next = Model::build(items);
        let plan = plan_diff(&state.shared.model.borrow(), &next);
        // Model first, AppKit second: every delta call in `apply_diff` re-enters the data source.
        *state.shared.model.borrow_mut() = next;
        if plan.is_empty() {
            return SidebarDiff {
                unchanged: true,
                ..Default::default()
            };
        }
        apply_diff(state, plan)
    }

    /// Select the row with this id, or clear the selection. Diffed; does not re-enter
    /// [`NativeSidebar::on_select`].
    ///
    /// Expands the item's ancestors first. Without that a collapsed parent leaves the item with
    /// no row, `rowForItem:` answers -1, and the -1 branch below *clears* the selection — which
    /// is indistinguishable from success whenever nothing was selected to begin with.
    pub fn set_selected(&self, id: Option<&str>) {
        let state = &*self.0;
        let outline = &state.views.outline;

        let wanted = id.and_then(|id| state.shared.model.borrow().key_for_id(id));
        let row = match wanted {
            Some(key) => {
                reveal(state, key);
                let item = state.shared.item(key);
                unsafe { outline.rowForItem(Some(&item)) }
            }
            None => -1,
        };
        if row == outline.selectedRow() {
            return;
        }

        state.shared.suppress.set(state.shared.suppress.get() + 1);
        state.shared.writes.set(state.shared.writes.get() + 1);
        if row < 0 {
            unsafe { outline.deselectAll(None) };
        } else {
            outline.selectRowIndexes_byExtendingSelection(
                &NSIndexSet::indexSetWithIndex(row as usize),
                false,
            );
        }
        state.shared.suppress.set(state.shared.suppress.get() - 1);
    }

    pub fn selected(&self) -> Option<SharedString> {
        self.0.shared.selected_id()
    }

    /// Called when the *user* changes the selection.
    ///
    /// Dispatch is deferred, never synchronous: a selection notification can fire while GPUI
    /// already holds its `App` borrow, and a synchronous update then drops the event permanently.
    pub fn on_select(
        &self,
        handler: impl Fn(&Option<SharedString>, &mut Window, &mut App) + 'static,
    ) {
        *self.0.shared.on_select.borrow_mut() = Some(Rc::new(handler));
    }

    /// Called when the user drags a row to a new place, once the outline view has already moved
    /// it. The host applies the same move to its own data; the next [`NativeSidebar::update`]
    /// then diffs to nothing. A host that ignores it puts the row back on the next update, which
    /// is the right answer if the host's order is the truth.
    ///
    /// Does nothing unless the sidebar was installed with [`SidebarOptions::reorderable`].
    pub fn on_reorder(&self, handler: impl Fn(&SidebarMove, &mut Window, &mut App) + 'static) {
        *self.0.shared.on_reorder.borrow_mut() = Some(Rc::new(handler));
    }

    /// Called with the row's id when the user clicks its eject affordance. The sidebar unmounts
    /// nothing and does not remove the row: what ejecting means is the host's to decide.
    pub fn on_eject(&self, handler: impl Fn(&SharedString, &mut Window, &mut App) + 'static) {
        *self.0.shared.on_eject.borrow_mut() = Some(Rc::new(handler));
    }

    /// Collapse or expand with AppKit's own animation — the toolbar item's own code path.
    pub fn toggle(&self) {
        unsafe { self.0.views.split.toggleSidebar(None) };
    }

    pub fn set_collapsed(&self, collapsed: bool) {
        if self.0.views.sidebar_item.isCollapsed() != collapsed {
            self.toggle();
        }
    }

    pub fn is_collapsed(&self) -> bool {
        self.0.views.sidebar_item.isCollapsed()
    }

    /// Change the row metrics class after install.
    ///
    /// `NSTableCellView` reads `rowSizeStyle` when it lays out, and pooled cells were laid out
    /// under the old one, so the rows have to be rebuilt for the new metrics to reach them.
    pub fn set_row_size(&self, size: SidebarRowSize) {
        let state = &*self.0;
        let outline = &state.views.outline;
        let wanted = size.appkit();
        if outline.rowSizeStyle() == wanted {
            return;
        }
        let saved = SavedState::capture(state);
        outline.setRowSizeStyle(wanted);
        outline.reloadData();
        if state.shared.expand_groups.get() {
            expand_groups(state);
        }
        saved.restore(state);
        state.shared.writes.set(state.shared.writes.get() + 1);
        state.pane_shared.clone().align(&state.views.pane);
    }

    pub fn row_size(&self) -> SidebarRowSize {
        match self.0.views.outline.rowSizeStyle() {
            NSTableViewRowSizeStyle::Small => SidebarRowSize::Small,
            NSTableViewRowSizeStyle::Medium => SidebarRowSize::Medium,
            NSTableViewRowSizeStyle::Large => SidebarRowSize::Large,
            _ => SidebarRowSize::System,
        }
    }

    /// Move the divider. AppKit clamps to the item's min and max thickness.
    pub fn set_thickness(&self, thickness: Pixels) {
        let split = self.0.views.split.splitView();
        split.setPosition_ofDividerAtIndex(thickness.to_f64(), 0);
        self.0.pane_shared.clone().align(&self.0.views.pane);
    }

    pub fn set_thickness_limits(&self, min: Pixels, max: Pixels) {
        let item = &self.0.views.sidebar_item;
        item.setMinimumThickness(min.to_f64());
        item.setMaximumThickness(max.to_f64());
        self.0.views.split.view().layoutSubtreeIfNeeded();
        self.0.pane_shared.clone().align(&self.0.views.pane);
    }

    /// Attach or detach the unified toolbar. Detaching it drops the pane's top safe-area inset,
    /// so [`SidebarMetrics::top`] changes with it.
    pub fn set_toolbar(&self, visible: bool) {
        let Some(window) = self.window_frame_window() else {
            return;
        };
        if window.toolbar().is_some() == visible {
            return;
        }
        window.setToolbar(visible.then(|| self.0.views.toolbar.clone()).as_deref());
        self.0.views.split.view().layoutSubtreeIfNeeded();
        self.0.pane_shared.clone().align(&self.0.views.pane);
    }

    pub fn has_toolbar(&self) -> bool {
        self.window_frame_window()
            .is_some_and(|w| w.toolbar().is_some())
    }

    /// Whether later insertions auto-expand their group. Turning it on expands what is there now.
    pub fn set_expand_groups(&self, expand: bool) {
        self.0.shared.expand_groups.set(expand);
        if expand {
            expand_groups(&self.0);
        }
    }

    pub fn expands_groups(&self) -> bool {
        self.0.shared.expand_groups.get()
    }

    /// Retint the window-spanning backdrop. A sidebar installed with `backdrop: None` has no view
    /// to retint and ignores this.
    pub fn set_backdrop(&self, material: Material) {
        if let Some(view) = self.0.views.backdrop.as_ref() {
            view.setMaterial(RawMaterial::from(material).appkit());
        }
    }

    pub fn set_expanded(&self, id: &str, expanded: bool) {
        let state = &*self.0;
        let Some(key) = state.shared.model.borrow().key_for_id(id) else {
            return;
        };
        let item = state.shared.item(key);
        if unsafe { state.views.outline.isItemExpanded(Some(&item)) } == expanded {
            return;
        }
        state.shared.writes.set(state.shared.writes.get() + 1);
        unsafe {
            if expanded {
                state.views.outline.expandItem(Some(&item));
            } else {
                state.views.outline.collapseItem(Some(&item));
            }
        }
    }

    pub fn is_expanded(&self, id: &str) -> bool {
        let state = &*self.0;
        let Some(key) = state.shared.model.borrow().key_for_id(id) else {
            return false;
        };
        let item = state.shared.item(key);
        unsafe { state.views.outline.isItemExpanded(Some(&item)) }
    }

    /// Vertical scroll offset of the row list, in points.
    pub fn scroll_offset(&self) -> f32 {
        self.0.views.scroll.contentView().bounds().origin.y as f32
    }

    pub fn scroll_to(&self, offset: f32) {
        let clip = self.0.views.scroll.contentView();
        clip.scrollToPoint(NSPoint::new(clip.bounds().origin.x, offset as f64));
        self.0.views.scroll.reflectScrolledClipView(&clip);
    }

    /// The [`SidebarMetrics`] global's value, readable synchronously without an `App`.
    pub fn metrics(&self) -> SidebarMetrics {
        self.0.pane_shared.metrics.get()
    }

    /// How many times the pane has re-aligned GPUI's view.
    pub fn align_count(&self) -> u64 {
        self.0.pane_shared.align_count.get()
    }

    /// Times the alignment found GPUI's frame changed by someone other than itself and repaired
    /// it. Non-zero means a second writer exists.
    pub fn frame_corrections(&self) -> u64 {
        self.0.pane_shared.corrections.get()
    }

    /// Cumulative AppKit mutations issued by this sidebar.
    pub fn appkit_writes(&self) -> u64 {
        self.0.shared.writes.get()
    }

    /// GPUI's view frame in its superview, the content pane.
    pub fn gpui_frame(&self) -> NSRect {
        self.0.pane_shared.gpui.frame()
    }

    pub fn content_view_size(&self) -> NSSize {
        self.window_frame_window()
            .and_then(|w| w.contentView())
            .map_or(NSSize::ZERO, |v| v.frame().size)
    }

    /// The content pane's rect in window-content-view space.
    pub fn pane_rect(&self) -> NSRect {
        let pane = &self.0.views.pane;
        match pane.window().and_then(|w| w.contentView()) {
            Some(cv) => pane.convertRect_toView(pane.bounds(), Some(&cv)),
            None => pane.frame(),
        }
    }

    pub fn probe(&self) -> SidebarProbe {
        let views = &self.0.views;
        let outline = &views.outline;
        let first_responder = outline
            .window()
            .and_then(|w| w.firstResponder())
            .is_some_and(|r| {
                std::ptr::eq(
                    &*r as *const _ as *const NSView,
                    &**outline as *const _ as *const NSView,
                )
            });
        SidebarProbe {
            behavior: views.sidebar_item.behavior().0,
            allows_full_height_layout: views.sidebar_item.allowsFullHeightLayout(),
            style: outline.style().0,
            effective_style: outline.effectiveStyle().0,
            row_size_style: outline.rowSizeStyle().0,
            effective_row_size_style: outline.effectiveRowSizeStyle().0,
            row_height: outline.rowHeight(),
            indentation_per_level: outline.indentationPerLevel(),
            divider_thickness: views.split.splitView().dividerThickness(),
            rows: outline.numberOfRows(),
            backdrop_material: views.backdrop.as_ref().map_or(-1, |v| v.material().0),
            // The window, not our retained toolbar: the shell keeps one built even while it is
            // detached, so only the window knows whether one is attached.
            toolbar_items: outline
                .window()
                .and_then(|w| w.toolbar())
                .map_or(0, |t| t.items().count()),
            scroll_draws_background: views.scroll.drawsBackground(),
            outline_is_first_responder: first_responder,
        }
    }

    /// Height of the screen whose bottom-left is Cocoa's global origin — the constant that flips
    /// a Cocoa frame into the top-left-origin space `CGEventPost` uses.
    pub fn screen_height(&self) -> f64 {
        let mtm = MainThreadMarker::new()
            .expect("a NativeSidebar is !Send, so it is only reachable on gpui's main thread");
        NSScreen::screens(mtm)
            .firstObject()
            .map_or(0., |screen| screen.frame().size.height)
    }

    /// The hosting `NSWindow`, for callers that need to attach a child window to it.
    pub fn window_frame_window(&self) -> Option<Retained<NSWindow>> {
        self.0.pane_shared.gpui.window()
    }

    /// A synthetic click into a non-key window is swallowed by `acceptsFirstMouse:` returning NO,
    /// which is indistinguishable from having no target.
    pub fn window_is_key(&self) -> bool {
        self.window_frame_window().is_some_and(|w| w.isKeyWindow())
    }

    /// The `CGWindowID` of the hosting window.
    pub fn window_number(&self) -> isize {
        self.window_frame_window().map_or(0, |w| w.windowNumber())
    }

    /// The hosting window's frame in Cocoa screen coordinates.
    pub fn window_frame(&self) -> NSRect {
        self.window_frame_window()
            .map_or(NSRect::ZERO, |w| w.frame())
    }

    /// Direct subviews of the split view. Two, however many times [`NativeSidebar::install`] runs.
    pub fn split_subview_count(&self) -> usize {
        self.0.views.split.splitView().subviews().count()
    }

    pub fn row_layout(&self, row: isize) -> Option<RowLayout> {
        let outline = &self.0.views.outline;
        if row < 0 || row >= outline.numberOfRows() {
            return None;
        }
        let item = outline.itemAtRow(row)?;
        let view = outline.viewAtColumn_row_makeIfNecessary(0, row, false)?;
        let cell = unsafe { Retained::cast_unchecked::<SidebarCell>(view) };
        let frame = |view: Option<&NSView>| view.map_or(NSRect::ZERO, |v| v.frame());
        let icon = unsafe { cell.imageView() };
        let title = unsafe { cell.textField() };
        let badge = cell.ivars().badge.borrow().clone();
        Some(RowLayout {
            row: outline.rectOfRow(row),
            cell: outline.frameOfCellAtColumn_row(0, row),
            icon: frame(icon.as_deref().map(|v| v.as_ref())),
            title: frame(title.as_deref().map(|v| v.as_ref())),
            badge: frame(badge.as_deref().map(|v| v.as_ref())),
            level: outline.levelForRow(row),
            expanded: unsafe { outline.isItemExpanded(Some(&item)) },
        })
    }

    /// The centre of a row in GPUI-logical points from the content view's top-left — the space
    /// GPUI's own mouse events use. `None` for a row that is not currently laid out.
    pub fn row_point(&self, row: isize) -> Option<NSPoint> {
        let outline = &self.0.views.outline;
        if row < 0 || row >= outline.numberOfRows() {
            return None;
        }
        let rect = outline.rectOfRow(row);
        let window = outline.window()?;
        let content = window.contentView()?;
        let in_content = outline.convertRect_toView(rect, Some(&content));
        Some(NSPoint::new(
            in_content.origin.x + in_content.size.width / 2.,
            content.frame().size.height - (in_content.origin.y + in_content.size.height / 2.),
        ))
    }

    /// Make GPUI's view first responder again. Installing the shell, and any click in the
    /// sidebar, moves it away.
    pub fn focus_content(&self) {
        let gpui = &self.0.pane_shared.gpui;
        if let Some(window) = gpui.window() {
            window.makeFirstResponder(Some(gpui));
        }
    }
}

struct PaneShared {
    gpui: Retained<NSView>,
    align: PaneAlign,
    adopt_below: bool,
    intended_gpui: Cell<NSRect>,
    metrics: Cell<SidebarMetrics>,
    published: Cell<SidebarMetrics>,
    align_count: Cell<u64>,
    corrections: Cell<u64>,
    cx: RefCell<AsyncWindowContext>,
}

impl PaneShared {
    fn align(&self, pane: &NSView) {
        let Some(window) = pane.window() else { return };
        let Some(content_view) = window.contentView() else {
            return;
        };
        self.align_count.set(self.align_count.get() + 1);

        let cv_size = content_view.frame().size;
        // The pane's own `frame.origin.x` reads 0 while it sits a sidebar's width in; converting
        // is mandatory or the alignment silently collapses to the broken arm.
        let pane_in_cv = pane.convertRect_toView(pane.bounds(), Some(&content_view));
        let safe = pane.safeAreaInsets();

        // gpui reads its logical size from contentView.frame(), its drawable from its own frame,
        // and mouse points unconverted — hence contentView's size at a pane-cancelling origin.
        let wanted = match self.align {
            PaneAlign::WindowAligned => NSRect::new(
                NSPoint::new(-pane_in_cv.origin.x, -pane_in_cv.origin.y),
                cv_size,
            ),
            PaneAlign::FillPane => pane.bounds(),
        };
        // Diffing intent alone is only valid while nothing else writes the frame, so the frame is
        // read back too — cheap because an `NSView` frame round-trips bit-exactly.
        let as_intended = rects_equal(self.intended_gpui.get(), wanted);
        let in_place = rects_equal(self.gpui.frame(), wanted);
        if !as_intended || !in_place {
            if as_intended {
                self.corrections.set(self.corrections.get() + 1);
            }
            self.gpui.setFrame(wanted);
            self.intended_gpui.set(wanted);
        }

        if self.adopt_below {
            self.sync_below(pane, wanted);
        }

        let pane_top = cv_size.height - (pane_in_cv.origin.y + pane_in_cv.size.height);
        let metrics = SidebarMetrics {
            left: px((pane_in_cv.origin.x + safe.left) as f32),
            top: px((pane_top + safe.top) as f32),
            right: px(
                (cv_size.width - (pane_in_cv.origin.x + pane_in_cv.size.width) + safe.right) as f32,
            ),
            bottom: px((pane_in_cv.origin.y + safe.bottom) as f32),
            thickness: px(pane_in_cv.origin.x as f32),
            collapsed: pane_in_cv.origin.x <= 1.0,
        };
        self.metrics.set(metrics);
        if self.published.get() != metrics {
            self.published.set(metrics);
            // Deferred: this runs inside an AppKit layout pass, which can happen while GPUI holds
            // its borrow. The refresh is what re-lays-out GPUI on every collapse-animation frame.
            self.cx
                .borrow()
                .spawn(async move |cx| {
                    let _ = cx.update(|window, app| {
                        app.set_global(metrics);
                        window.refresh();
                    });
                })
                .detach();
        }
    }

    /// Put every [`crate::Layer::Below`] container in this pane into GPUI's coordinate space.
    ///
    /// `gpui_frame` is the pane-cancelling frame, so this is the only thing that lines a below
    /// container up with GPUI's own view; one that misses it keeps its pane-local origin and
    /// everything hosted in it draws a sidebar's width to the right.
    ///
    /// Scanned every time rather than cached against the pane's subview **count**, because a
    /// count is not a set: one container added and another removed between two passes leaves it
    /// unchanged, and a container created while it happens to be unchanged is never picked up at
    /// all. The scan is a handful of `isKindOfClass` calls beside work this function already does.
    fn sync_below(&self, pane: &NSView, gpui_frame: NSRect) {
        for view in pane.subviews().iter() {
            if view.isKindOfClass(Container::class()) && !rects_equal(view.frame(), gpui_frame) {
                view.setFrame(gpui_frame);
            }
        }
    }
}

define_class!(
    /// The content split item's view: clips GPUI's oversized child to the pane, re-aligns it on
    /// every geometry change, and paints the window background under it. All four geometry
    /// overrides are needed — `setFrame:` is *not* implemented in terms of
    /// `setFrameSize:`/`setFrameOrigin:` and is how `NSSplitView` positions its panes, `layout`
    /// covers the collapse animation, `setFrameOrigin:` the divider drag.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSidebarContentPane"]
    #[ivars = Rc<PaneShared>]
    struct ContentPane;

    impl ContentPane {
        #[unsafe(method(layout))]
        fn layout(&self) {
            let _: () = unsafe { msg_send![super(self), layout] };
            self.ivars().clone().align(self);
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            self.ivars().clone().align(self);
        }

        #[unsafe(method(setFrameOrigin:))]
        fn set_frame_origin(&self, origin: NSPoint) {
            let _: () = unsafe { msg_send![super(self), setFrameOrigin: origin] };
            self.ivars().clone().align(self);
        }

        #[unsafe(method(setFrame:))]
        fn set_frame(&self, frame: NSRect) {
            let _: () = unsafe { msg_send![super(self), setFrame: frame] };
            self.ivars().clone().align(self);
        }

        /// The window background, for the whole content side including its own toolbar band.
        ///
        /// This pane runs the full height of the window and is inset from the band only by its
        /// `safeAreaInsets`, so GPUI lays out below the band while the pane spans it. Nothing else
        /// can paint there: GPUI content stops at the inset, and the window's own
        /// `backgroundColor` is inert while a window-spanning backdrop covers it — writing it from
        /// clear to an opaque `windowBackgroundColor` left the capture byte-identical.
        ///
        /// The fill is beneath every subview, so a [`crate::Layer::Below`] view keeps compositing
        /// over it. An `NSColor` and not a cached value: it re-resolves in whatever appearance is
        /// current, and AppKit redisplays a layer-backed view on an appearance change by itself,
        /// so no invalidation is wired here.
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            tokens::WINDOW_BACKGROUND.ns_color().setFill();
            NSBezierPath::fillRect(dirty);
        }
    }
);

impl ContentPane {
    fn new(mtm: MainThreadMarker, frame: NSRect, shared: Rc<PaneShared>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(shared);
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setWantsLayer(true);
        if let Some(layer) = this.layer() {
            layer.setMasksToBounds(true);
        }
        this
    }
}

struct CellIvars {
    badge: RefCell<Option<Retained<NSTextField>>>,
    eject: RefCell<Option<Retained<NSButton>>>,
}

impl CellIvars {
    /// The one thing on a row's trailing edge. Exclusive on purpose: a volume has an eject and a
    /// mailbox has a count, and no Mac sidebar puts both on one row.
    fn trailing(&self) -> Option<Retained<NSView>> {
        let visible = |view: &NSView| !view.isHidden();
        if let Some(eject) = self.eject.borrow().clone().filter(|v| visible(v)) {
            return Some(Retained::into_super(Retained::into_super(eject)));
        }
        let badge = self.badge.borrow().clone().filter(|v| visible(v))?;
        Some(Retained::into_super(Retained::into_super(badge)))
    }
}

define_class!(
    /// A source-list row. `NSTableCellView` lays out its `textField` and `imageView` outlets from
    /// `rowSizeStyle`, so `super`'s layout is what puts the icon and the label where the system
    /// wants them — matching Finder to the point. This override only adds the trailing accessory,
    /// which `NSTableCellView` has no outlet for and therefore never places.
    #[unsafe(super(NSTableCellView))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSidebarCell"]
    #[ivars = CellIvars]
    struct SidebarCell;

    impl SidebarCell {
        #[unsafe(method(layout))]
        fn layout(&self) {
            let _: () = unsafe { msg_send![super(self), layout] };

            // The image view is deliberately never hidden. An SF Symbol the running macOS does
            // not know leaves `image` nil, and hiding the view then collapses AppKit's reserved
            // icon box and pulls that one row's label left of every sibling's. Nil draws nothing
            // on its own, so the row loses its glyph and keeps its alignment.

            let Some(trailing) = self.ivars().trailing() else {
                return;
            };
            let bounds = self.bounds();
            let size = trailing.fittingSize();
            trailing.setFrame(NSRect::new(
                NSPoint::new(
                    bounds.size.width - size.width,
                    ((bounds.size.height - size.height) / 2.).round(),
                ),
                size,
            ));

            if let Some(title) = unsafe { self.textField() } {
                let frame = title.frame();
                let width = bounds.size.width - size.width - BADGE_GAP - frame.origin.x;
                title.setFrame(NSRect::new(
                    frame.origin,
                    NSSize::new(width.max(0.), frame.size.height),
                ));
            }
        }
    }
);

/// `NSTableView` keeps one recycling queue per identifier, so splitting rows by shape is how a
/// recycled cell avoids being restructured on every reuse.
const ID_GROUP: &str = "NGSidebarGroupRow";
const ID_ICON: &str = "NGSidebarIconRow";
const ID_PLAIN: &str = "NGSidebarPlainRow";

fn label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    field.setUsesSingleLineMode(true);
    field.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    field
}

/// Point size that renders `eject`'s ink at Finder's measured 9.5 x 9.5pt. The `.fill` variant
/// comes out 9.5 x 9.0 at the same size, so the outline is the one Finder draws.
const EJECT_POINTS: f64 = 10.0;

fn make_cell(
    mtm: MainThreadMarker,
    identifier: &str,
    with_icon: bool,
    target: &AnyObject,
) -> Retained<SidebarCell> {
    let this = SidebarCell::alloc(mtm).set_ivars(CellIvars {
        badge: RefCell::new(None),
        eject: RefCell::new(None),
    });
    let cell: Retained<SidebarCell> = unsafe { msg_send![super(this), init] };
    cell.setIdentifier(Some(&NSString::from_str(identifier)));

    let title = label(mtm, "");
    cell.addSubview(&title);
    unsafe { cell.setTextField(Some(&title)) };

    if with_icon {
        let icon = NSImageView::new(mtm);
        // No colour is set: a template symbol takes `labelColor` from the image view, and
        // `NSTableCellView` flips that to the emphasized colour under a selection pill.
        icon.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        icon.setImageAlignment(NSImageAlignment::AlignCenter);
        cell.addSubview(&icon);
        unsafe { cell.setImageView(Some(&icon)) };
    }

    // There is no system token for a source-list count badge — `NSItemBadge` is documented for
    // `NSToolbarItem` only — so it is a plain label in the system's small font.
    let badge = label(mtm, "");
    badge.setAlignment(NSTextAlignment::Right);
    badge.setFont(Some(&NSFont::systemFontOfSize(
        NSFont::smallSystemFontSize(),
    )));
    badge.setTextColor(Some(&NSColor::secondaryLabelColor()));
    badge.setHidden(true);
    cell.addSubview(&badge);
    *cell.ivars().badge.borrow_mut() = Some(badge);

    let eject = NSButton::new(mtm);
    eject.setBordered(false);
    eject.setImagePosition(NSCellImagePosition::ImageOnly);
    eject.setImage(
        NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("eject"),
            Some(&NSString::from_str("Eject")),
        )
        .and_then(|image| {
            image.imageWithSymbolConfiguration(
                &NSImageSymbolConfiguration::configurationWithPointSize_weight(
                    EJECT_POINTS,
                    unsafe { NSFontWeightRegular },
                ),
            )
        })
        .as_deref(),
    );
    // Finder's eject ink measures 94 peak against its row labels' 244 — a ratio of 0.385, and the
    // tiers over that background resolve to 220 / 152 / 83 / 49, i.e. 1.00 / 0.69 / 0.38 / 0.22.
    // The ratio is the statistic to compare, not the absolute: it is taken inside one capture, so
    // it survives the colour space the capture was written in.
    eject.setContentTintColor(Some(&NSColor::tertiaryLabelColor()));
    eject.setHidden(true);
    unsafe {
        eject.setTarget(Some(target));
        eject.setAction(Some(sel!(ngSidebarEject:)));
    }
    cell.addSubview(&eject);
    *cell.ivars().eject.borrow_mut() = Some(eject);

    cell
}

/// A [`SidebarIcon::SystemResource`] as a template image sized for a `points`-tall icon box.
///
/// The image is built to hold the representations of **one point size**, because in this bundle a
/// point size and a pixel size are different questions: `SidebarAirDrop`'s 16pt@2x rep and its
/// 32pt@1x rep are both 32 pixels square and are *different drawings* — three arcs against four,
/// scoring 0.35 against each other where two reps of one drawing score 0.89. Gather by pixels and
/// a 32pt image ends up carrying both, leaving the drawing to whichever rep the scale factor
/// happens to select; nothing in a build or a screenshot diff would say so. Handing the whole
/// eight-rep image straight to an image view has the same defect for the same reason.
fn core_types_template(name: &SharedString, points: f64) -> Option<Retained<NSImage>> {
    thread_local! {
        /// The reps are shared with the bundle's own image, so an entry costs a handle, not pixels.
        static TEMPLATES: RefCell<HashMap<(SharedString, u32), Option<Retained<NSImage>>>> =
            RefCell::new(HashMap::new());
    }
    let key = (name.clone(), points as u32);
    if let Some(hit) = TEMPLATES.with_borrow(|cache| cache.get(&key).cloned()) {
        return hit;
    }
    let built = build_core_types_template(name, points);
    TEMPLATES.with_borrow_mut(|cache| cache.insert(key, built.clone()));
    built
}

/// Nearest available representation, in points, ties to the smaller.
fn nearest_rep_points(available: impl IntoIterator<Item = f64>, wanted: f64) -> Option<f64> {
    available.into_iter().fold(None, |held, candidate| {
        let better = held.is_none_or(|held: f64| {
            let (a, b) = ((candidate - wanted).abs(), (held - wanted).abs());
            a < b || (a == b && candidate < held)
        });
        Some(if better { candidate } else { held? })
    })
}

/// Load a [`CORE_TYPES_BUNDLE`] resource as a template image, taking the representations of the
/// point size nearest `points`.
///
/// Public because the same artwork is the right answer outside a sidebar row — a toolbar item for
/// AirDrop has the same problem and the same solution. See [`SidebarIcon::SystemResource`] for
/// when to reach for this instead of a symbol.
pub fn system_template(resource: &str, points: Pixels) -> Option<Retained<NSImage>> {
    build_core_types_template(resource, points.to_f64())
}

fn build_core_types_template(name: &str, points: f64) -> Option<Retained<NSImage>> {
    let bundle = NSBundle::bundleWithPath(&NSString::from_str(CORE_TYPES_BUNDLE))?;
    let source = bundle.imageForResource(&NSString::from_str(name))?;
    let reps = source.representations();
    let chosen = nearest_rep_points(reps.iter().map(|rep| rep.size().width), points)?;
    let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(chosen, chosen));
    for rep in reps.iter() {
        // Both scales of the chosen point size, so moving the window to another display still
        // finds a rep and does not fall back to resampling the one that is left.
        if rep.size().width == chosen {
            image.addRepresentation(&rep);
        }
    }
    // The bundle's images come through with `isTemplate` false, so nothing would tint them and a
    // black glyph is invisible in dark mode.
    image.setTemplate(true);
    Some(image)
}

fn configure_cell(cell: &SidebarCell, node: &Node, icon_points: f64) {
    if let Some(title) = unsafe { cell.textField() } {
        let wanted = NSString::from_str(&node.title);
        if title.stringValue() != wanted {
            title.setStringValue(&wanted);
        }
    }

    if let Some(view) = unsafe { cell.imageView() } {
        let image = match &node.icon {
            Some(SidebarIcon::Symbol(name)) => {
                NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(name),
                    None,
                )
            }
            Some(SidebarIcon::SystemResource(name)) => core_types_template(name, icon_points),
            None => None,
        };
        view.setImage(image.as_deref());
    }

    if let Some(badge) = cell.ivars().badge.borrow().as_ref() {
        match node.badge {
            Some(count) => {
                let text = NSString::from_str(&count.to_string());
                if badge.stringValue() != text {
                    badge.setStringValue(&text);
                }
                badge.setHidden(false);
            }
            None => badge.setHidden(true),
        }
    }

    if let Some(eject) = cell.ivars().eject.borrow().as_ref() {
        eject.setHidden(!node.eject);
    }

    cell.setNeedsLayout(true);
}

/// A free function because `#[unsafe(method_id(..))]` wraps the body's tail expression, so an
/// early `return None` inside the delegate method does not typecheck.
fn row_view(
    shared: &Rc<Shared>,
    outline: &NSOutlineView,
    item: &AnyObject,
    target: &AnyObject,
) -> Option<Retained<NSView>> {
    let mtm = MainThreadMarker::new().expect("outline view callbacks are main-thread");
    let model = shared.model.borrow();
    let node = model.node(Shared::key_of_item(item))?;

    let (identifier, with_icon) = if node.group {
        (ID_GROUP, false)
    } else if node.icon.is_some() {
        (ID_ICON, true)
    } else {
        (ID_PLAIN, false)
    };
    let name = NSString::from_str(identifier);

    let cell = match unsafe { outline.makeViewWithIdentifier_owner(&name, Some(outline)) } {
        Some(view) => unsafe { Retained::cast_unchecked::<SidebarCell>(view) },
        None => make_cell(mtm, identifier, with_icon, target),
    };
    configure_cell(
        &cell,
        node,
        SidebarRowSize::icon_points(outline.effectiveRowSizeStyle()),
    );
    Some(unsafe { Retained::cast_unchecked::<NSView>(cell) })
}

/// The bodies both source classes share.
///
/// Free functions rather than an inheritance chain: `objc2`'s `set_ivars` fills **one** class's
/// slot, so a subclass that declares its own ivars leaves the superclass's unset and every
/// inherited method reading it panics on the first callback. Two siblings over one set of bodies
/// has no such contract to get wrong.
mod source {
    use super::*;

    pub(super) fn children(shared: &Rc<Shared>, item: Option<&AnyObject>) -> NSInteger {
        let model = shared.model.borrow();
        match item {
            None => model.roots.len() as NSInteger,
            Some(item) => model
                .node(Shared::key_of_item(item))
                .map_or(0, |node| node.children.len() as NSInteger),
        }
    }

    pub(super) fn child(
        shared: &Rc<Shared>,
        index: NSInteger,
        item: Option<&AnyObject>,
    ) -> Retained<AnyObject> {
        let key = {
            let model = shared.model.borrow();
            let indices = match item {
                None => model.roots.clone(),
                Some(item) => model
                    .node(Shared::key_of_item(item))
                    .map(|node| node.children.clone())
                    .unwrap_or_default(),
            };
            indices
                .get(index as usize)
                .map(|&i| model.nodes[i].key)
                .unwrap_or(0)
        };
        shared.item(key)
    }

    pub(super) fn expandable(shared: &Rc<Shared>, item: &AnyObject) -> bool {
        shared
            .model
            .borrow()
            .node(Shared::key_of_item(item))
            .is_some_and(|node| !node.children.is_empty())
    }

    pub(super) fn is_group(shared: &Rc<Shared>, item: &AnyObject) -> bool {
        shared
            .model
            .borrow()
            .node(Shared::key_of_item(item))
            .is_some_and(|node| node.group)
    }

    pub(super) fn selectable(shared: &Rc<Shared>, item: &AnyObject) -> bool {
        shared
            .model
            .borrow()
            .node(Shared::key_of_item(item))
            .is_some_and(|node| !node.group)
    }

    pub(super) fn selection_changed(shared: &Rc<Shared>) {
        if shared.suppress.get() > 0 {
            return;
        }
        let Some(handler) = shared.on_select.borrow().clone() else {
            return;
        };
        // Read the id now, not in the task: the row may be gone by the time it runs.
        let id = shared.selected_id();
        let shared = shared.clone();
        shared
            .cx
            .borrow()
            .spawn(async move |cx| {
                let _ = cx.update(|window, app| handler(&id, window, app));
            })
            .detach();
    }

    pub(super) fn eject_clicked(shared: &Rc<Shared>, sender: &NSView) {
        let Some(outline) = shared.outline.borrow().clone() else {
            return;
        };
        let row = outline.rowForView(sender);
        let Some(item) = (row >= 0).then(|| outline.itemAtRow(row)).flatten() else {
            return;
        };
        let Some(id) = shared.model.borrow().id_for(Shared::key_of_item(&item)) else {
            return;
        };
        let Some(handler) = shared.on_eject.borrow().clone() else {
            return;
        };
        shared
            .cx
            .borrow()
            .spawn(async move |cx| {
                let _ = cx.update(|window, app| handler(&id, window, app));
            })
            .detach();
    }
}

/// Everything a source class does, so the two of them differ only by the drag methods.
///
/// A macro because `define_class!` parses its own body and cannot have another macro expanded
/// inside it, so sharing the eight shared methods means generating the whole class.
macro_rules! outline_source {
    ($(#[$meta:meta])* $class:ident, $name:literal, $($drag:tt)*) => {
define_class!(
    $(#[$meta])*
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = $name]
    #[ivars = Rc<Shared>]
    struct $class;

        unsafe impl NSObjectProtocol for $class {}

        impl $class {
            /// `setTarget:` is weak, and this object outlives every cell because the sidebar
            /// retains it, so the buttons can point straight here without a second sink.
            #[unsafe(method(ngSidebarEject:))]
            fn eject_clicked(&self, sender: &NSView) {
                source::eject_clicked(self.ivars(), sender)
            }
        }

        unsafe impl NSOutlineViewDataSource for $class {
            #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
            fn number_of_children(&self, _: &NSOutlineView, item: Option<&AnyObject>) -> NSInteger {
                source::children(self.ivars(), item)
            }

            #[unsafe(method_id(outlineView:child:ofItem:))]
            fn child(
                &self,
                _: &NSOutlineView,
                index: NSInteger,
                item: Option<&AnyObject>,
            ) -> Retained<AnyObject> {
                source::child(self.ivars(), index, item)
            }

            #[unsafe(method(outlineView:isItemExpandable:))]
            fn is_expandable(&self, _: &NSOutlineView, item: &AnyObject) -> bool {
                source::expandable(self.ivars(), item)
            }

            $($drag)*
        }

        unsafe impl NSControlTextEditingDelegate for $class {}

        unsafe impl NSOutlineViewDelegate for $class {
            #[unsafe(method(outlineView:isGroupItem:))]
            fn is_group_item(&self, _: &NSOutlineView, item: &AnyObject) -> bool {
                source::is_group(self.ivars(), item)
            }

            #[unsafe(method(outlineView:shouldSelectItem:))]
            fn should_select(&self, _: &NSOutlineView, item: &AnyObject) -> bool {
                source::selectable(self.ivars(), item)
            }

            #[unsafe(method_id(outlineView:viewForTableColumn:item:))]
            fn view_for_item(
                &self,
                outline: &NSOutlineView,
                _: Option<&NSTableColumn>,
                item: &AnyObject,
            ) -> Option<Retained<NSView>> {
                row_view(self.ivars(), outline, item, self.as_ref())
            }

            #[unsafe(method(outlineViewSelectionDidChange:))]
            fn selection_did_change(&self, _: &NSNotification) {
                source::selection_changed(self.ivars())
            }
        }
);
    };
}

outline_source!(OutlineSource, "NGSidebarSource",);

/// The pasteboard type a sidebar row travels under. Private on purpose: a row key means nothing
/// outside the sidebar that issued it, and a public type would invite other views to accept one.
const ROW_PASTEBOARD_TYPE: &str = "com.native-gpui.sidebar-row";

fn dragged_key(info: &ProtocolObject<dyn NSDraggingInfo>) -> Option<u64> {
    let text = info
        .draggingPasteboard()
        .stringForType(&NSString::from_str(ROW_PASTEBOARD_TYPE))?;
    text.to_string().parse().ok()
}

/// Resolve AppKit's proposal into a gap between two siblings, as `(parent, child index)`.
///
/// AppKit proposes a drop *onto* whichever row the pointer is over for most of that row's height,
/// and a sidebar never accepts one — so without this the insertion line would exist only in the
/// couple of points between rows and blink out everywhere else. A drop onto a group header lands
/// at the top of that group, which is the only reading of it that means anything.
fn drop_target(
    shared: &Rc<Shared>,
    outline: &NSOutlineView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
    item: Option<&AnyObject>,
    index: NSInteger,
) -> Option<(Option<u64>, usize)> {
    if index != NSOutlineViewDropOnItemIndex as NSInteger {
        return Some((item.map(Shared::key_of_item), index.max(0) as usize));
    }
    let key = Shared::key_of_item(item?);
    let model = shared.model.borrow();
    if model.node(key)?.group {
        return Some((Some(key), 0));
    }
    let parent = model.parent_key(key);
    let at = model.children_keys(parent).iter().position(|&k| k == key)?;
    let row = unsafe { outline.rowForItem(item) };
    let rect = outline.rectOfRow(row);
    let point = outline.convertPoint_fromView(info.draggingLocation(), None);
    // An outline view is flipped, so a larger y is further down the list.
    let below = point.y > rect.origin.y + rect.size.height / 2.;
    Some((parent, at + usize::from(below)))
}

/// Free functions for the same reason [`row_view`] is one: the method macros wrap a body's tail
/// expression, so an early `return` inside one does not typecheck against the wrapped type.
fn row_pasteboard_item(
    shared: &Rc<Shared>,
    item: &AnyObject,
) -> Option<Retained<ProtocolObject<dyn NSPasteboardWriting>>> {
    let key = Shared::key_of_item(item);
    if !shared
        .model
        .borrow()
        .node(key)
        .is_some_and(|node| !node.group)
    {
        return None;
    }
    let entry = NSPasteboardItem::new();
    entry.setString_forType(
        &NSString::from_str(&key.to_string()),
        &NSString::from_str(ROW_PASTEBOARD_TYPE),
    );
    Some(ProtocolObject::from_retained(entry))
}

fn validate_row_drop(
    shared: &Rc<Shared>,
    outline: &NSOutlineView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
    item: Option<&AnyObject>,
    index: NSInteger,
) -> NSDragOperation {
    let Some(source) = dragged_key(info) else {
        return NSDragOperation::empty();
    };
    let Some((parent, at)) = drop_target(shared, outline, info, item, index) else {
        return NSDragOperation::empty();
    };
    // Siblings only. Finder refuses a Favorite dropped into Locations, and a sidebar whose groups
    // mean different kinds of thing has nothing sensible to do with one.
    if shared.model.borrow().parent_key(source) != parent {
        return NSDragOperation::empty();
    }
    let parent_item = parent.map(|key| shared.item(key));
    unsafe { outline.setDropItem_dropChildIndex(parent_item.as_deref(), at as NSInteger) };
    NSDragOperation::Move
}

fn accept_row_drop(
    shared: &Rc<Shared>,
    outline: &NSOutlineView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
    item: Option<&AnyObject>,
    index: NSInteger,
) -> bool {
    let Some(source) = dragged_key(info) else {
        return false;
    };
    if index < 0 {
        return false;
    }
    let parent = item.map(Shared::key_of_item);
    let from = {
        let model = shared.model.borrow();
        if model.parent_key(source) != parent {
            return false;
        }
        match model
            .children_keys(parent)
            .iter()
            .position(|&k| k == source)
        {
            Some(at) => at,
            None => return false,
        }
    };
    // The drop index counts the row itself; both the model move and AppKit's count the list with
    // the row already lifted out, so a downward move loses one.
    let to = (index as usize).saturating_sub(usize::from(index as usize > from));
    if to == from {
        return true;
    }
    if !shared.model.borrow_mut().move_child(parent, from, to) {
        return false;
    }
    shared.writes.set(shared.writes.get() + 1);
    let parent_item = parent.map(|key| shared.item(key));
    unsafe {
        outline.moveItemAtIndex_inParent_toIndex_inParent(
            from as NSInteger,
            parent_item.as_deref(),
            to as NSInteger,
            parent_item.as_deref(),
        );
    }
    let Some(handler) = shared.on_reorder.borrow().clone() else {
        return true;
    };
    let moved = {
        let model = shared.model.borrow();
        SidebarMove {
            id: model.id_for(source).unwrap_or_default(),
            parent: parent.and_then(|key| model.id_for(key)),
            index: to,
        }
    };
    // Deferred for the same reason a selection is: GPUI may already hold its `App` borrow.
    shared
        .cx
        .borrow()
        .spawn(async move |cx| {
            let _ = cx.update(|window, app| handler(&moved, window, app));
        })
        .detach();
    true
}

outline_source!(
    /// [`OutlineSource`] plus row dragging.
    ///
    /// A second class rather than a flag on the one class, because AppKit reads
    /// `respondsToSelector:` and never asks what the method would have returned: defining
    /// `outlineView:pasteboardWriterForItem:` at all is what moves the selection commit from
    /// mouse-down to mouse-up. A `nil` return does not opt back out.
    ///
    /// A sibling and not a subclass. `objc2` fills exactly one class's ivar slot at `set_ivars`,
    /// so a subclass carrying its own leaves the superclass's uninitialised, and the first
    /// inherited callback panics reading it — invisible to every build and to `cargo test`,
    /// because it takes launching the thing.
    ReorderSource,
    "NGSidebarReorderSource",
    #[unsafe(method_id(outlineView:pasteboardWriterForItem:))]
    fn pasteboard_writer(
        &self,
        _: &NSOutlineView,
        item: &AnyObject,
    ) -> Option<Retained<ProtocolObject<dyn NSPasteboardWriting>>> {
        row_pasteboard_item(self.ivars(), item)
    }

    #[unsafe(method(outlineView:validateDrop:proposedItem:proposedChildIndex:))]
    fn validate_drop(
        &self,
        outline: &NSOutlineView,
        info: &ProtocolObject<dyn NSDraggingInfo>,
        item: Option<&AnyObject>,
        index: NSInteger,
    ) -> NSDragOperation {
        validate_row_drop(self.ivars(), outline, info, item, index)
    }

    #[unsafe(method(outlineView:acceptDrop:item:childIndex:))]
    fn accept_drop(
        &self,
        outline: &NSOutlineView,
        info: &ProtocolObject<dyn NSDraggingInfo>,
        item: Option<&AnyObject>,
        index: NSInteger,
    ) -> bool {
        accept_row_drop(self.ivars(), outline, info, item, index)
    }
);

define_class!(
    /// Legitimises the system sidebar-toggle item. The item itself is still inserted explicitly:
    /// vending it from `toolbarDefaultItemIdentifiers:` alone yields a toolbar with zero items.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSidebarToolbarDelegate"]
    struct SidebarToolbarDelegate;

    unsafe impl NSObjectProtocol for SidebarToolbarDelegate {}

    unsafe impl NSToolbarDelegate for SidebarToolbarDelegate {
        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_items(&self, _: &NSToolbar) -> Retained<NSArray<NSString>> {
            NSArray::from_slice(&[unsafe { NSToolbarToggleSidebarItemIdentifier }])
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_items(&self, _: &NSToolbar) -> Retained<NSArray<NSString>> {
            NSArray::from_slice(&[unsafe { NSToolbarToggleSidebarItemIdentifier }])
        }
    }
);

enum Plan {
    Nothing,
    /// First push, or a recovery: nothing to diff against.
    Reload,
    Content(Vec<u64>),
    Structural {
        content: Vec<u64>,
        /// Each parent (`None` = the virtual root) with the child list the **old** model had.
        /// Asking the outline view instead routes back through the data source, which already
        /// answers from the new model, so every structural change would produce zero deltas.
        parents: Vec<(Option<u64>, Vec<u64>)>,
    },
}

impl Plan {
    fn is_empty(&self) -> bool {
        matches!(self, Plan::Nothing)
    }
}

fn plan_diff(old: &Model, new: &Model) -> Plan {
    if old.nodes.is_empty() {
        return if new.nodes.is_empty() {
            Plan::Nothing
        } else {
            Plan::Reload
        };
    }

    let mut content = Vec::new();
    let mut structural = old.roots.len() != new.roots.len() || old.nodes.len() != new.nodes.len();

    if !structural {
        for key in new.preorder() {
            let (Some(old_node), Some(new_node)) = (old.node(key), new.node(key)) else {
                structural = true;
                break;
            };
            if old_node.content() != new_node.content() {
                content.push(key);
            }
        }
    }
    if !structural {
        structural = std::iter::once(None)
            .chain(new.preorder().into_iter().map(Some))
            .any(|parent| old.children_keys(parent) != new.children_keys(parent));
    }

    if structural {
        // The virtual root plus every node that kept its parent. A node whose parent changed is a
        // remove and an insert seen by those two parents.
        let mut parents = vec![(None, old.children_keys(None))];
        parents.extend(
            old.preorder()
                .into_iter()
                .filter(|&key| {
                    new.node(key).is_some() && old.parent_key(key) == new.parent_key(key)
                })
                .map(|key| (Some(key), old.children_keys(Some(key)))),
        );
        return Plan::Structural { content, parents };
    }
    if content.is_empty() {
        Plan::Nothing
    } else {
        Plan::Content(content)
    }
}

fn apply_diff(state: &SidebarState, plan: Plan) -> SidebarDiff {
    let outline = &state.views.outline;
    let shared = &state.shared;
    let mut diff = SidebarDiff::default();

    let (content, parents) = match plan {
        Plan::Nothing => {
            return SidebarDiff {
                unchanged: true,
                ..Default::default()
            };
        }
        Plan::Reload => {
            outline.reloadData();
            if shared.expand_groups.get() {
                expand_groups(state);
            }
            shared.writes.set(shared.writes.get() + 1);
            return SidebarDiff {
                full_reloads: 1,
                ..Default::default()
            };
        }
        Plan::Content(content) => (content, Vec::new()),
        Plan::Structural { content, parents } => (content, parents),
    };

    if !parents.is_empty() {
        let saved = SavedState::capture(state);
        outline.beginUpdates();
        for (parent, was) in &parents {
            let (inserted, removed, moved) = diff_children(state, *parent, was);
            diff.inserted += inserted;
            diff.removed += removed;
            diff.moved += moved;
        }
        outline.endUpdates();
        shared
            .writes
            .set(shared.writes.get() + (diff.inserted + diff.removed + diff.moved) as u64);

        if shared.expand_groups.get() {
            expand_groups(state);
        }

        // A row count that disagrees with the model means the index sets were wrong, and a reload
        // plus an explicit state restore is the only correct recovery.
        let expected = expected_rows(state);
        if outline.numberOfRows() != expected {
            outline.reloadData();
            if shared.expand_groups.get() {
                expand_groups(state);
            }
            saved.restore(state);
            diff.full_reloads += 1;
            shared.writes.set(shared.writes.get() + 1);
        }
    }

    for key in content {
        let item = shared.item(key);
        // With reloadChildren NO this redraws exactly that row and touches neither selection,
        // expansion nor scroll offset.
        unsafe { outline.reloadItem(Some(&item)) };
        diff.reloaded_items += 1;
    }
    shared
        .writes
        .set(shared.writes.get() + diff.reloaded_items as u64);

    diff
}

/// Remove, then reorder, then insert — each against the already-swapped model.
fn diff_children(state: &SidebarState, parent: Option<u64>, was: &[u64]) -> (usize, usize, usize) {
    let outline = &state.views.outline;
    let shared = &state.shared;
    let parent_item = parent.map(|key| shared.item(key));
    let parent_ref = parent_item.as_deref();

    let mut current: Vec<u64> = was.to_vec();
    let wanted = shared.model.borrow().children_keys(parent);
    if current == wanted {
        return (0, 0, 0);
    }

    let doomed: Vec<usize> = current
        .iter()
        .enumerate()
        .filter(|(_, key)| !wanted.contains(key))
        .map(|(index, _)| index)
        .collect();
    let removed = doomed.len();
    if removed > 0 {
        let set = NSMutableIndexSet::new();
        for &index in &doomed {
            set.addIndex(index);
        }
        unsafe {
            outline.removeItemsAtIndexes_inParent_withAnimation(
                &set,
                parent_ref,
                NSTableViewAnimationOptions::EffectNone,
            )
        };
        for &index in doomed.iter().rev() {
            current.remove(index);
        }
    }

    // Order the survivors before inserting, so insertion indices are the final ones.
    let survivors: Vec<u64> = wanted
        .iter()
        .copied()
        .filter(|k| current.contains(k))
        .collect();
    let mut moved = 0;
    for (target, key) in survivors.iter().enumerate() {
        if current.get(target) == Some(key) {
            continue;
        }
        let Some(from) = current.iter().position(|k| k == key) else {
            continue;
        };
        unsafe {
            outline.moveItemAtIndex_inParent_toIndex_inParent(
                from as NSInteger,
                parent_ref,
                target as NSInteger,
                parent_ref,
            )
        };
        let key = current.remove(from);
        current.insert(target, key);
        moved += 1;
    }

    let mut inserted = 0;
    let fresh = NSMutableIndexSet::new();
    for (index, key) in wanted.iter().enumerate() {
        if !current.contains(key) {
            fresh.addIndex(index);
            current.insert(index.min(current.len()), *key);
            inserted += 1;
        }
    }
    if inserted > 0 {
        unsafe {
            outline.insertItemsAtIndexes_inParent_withAnimation(
                &fresh,
                parent_ref,
                NSTableViewAnimationOptions::EffectNone,
            )
        };
    }

    (inserted, removed, moved)
}

/// Expand `key`'s ancestors, outermost first, so the item has a row.
fn reveal(state: &SidebarState, key: u64) {
    let outline = &state.views.outline;
    let mut ancestors = Vec::new();
    {
        let model = state.shared.model.borrow();
        let mut walk = model.parent_key(key);
        while let Some(parent) = walk {
            ancestors.push(parent);
            walk = model.parent_key(parent);
        }
    }
    for parent in ancestors.into_iter().rev() {
        let item = state.shared.item(parent);
        if !unsafe { outline.isItemExpanded(Some(&item)) } {
            state.shared.writes.set(state.shared.writes.get() + 1);
            unsafe { outline.expandItem(Some(&item)) };
        }
    }
}

fn expand_groups(state: &SidebarState) {
    let outline = &state.views.outline;
    let keys: Vec<u64> = {
        let model = state.shared.model.borrow();
        model
            .preorder()
            .into_iter()
            .filter(|&key| {
                model
                    .node(key)
                    .is_some_and(|node| node.group && !node.children.is_empty())
            })
            .collect()
    };
    for key in keys {
        let item = state.shared.item(key);
        if !unsafe { outline.isItemExpanded(Some(&item)) } {
            unsafe { outline.expandItem(Some(&item)) };
        }
    }
}

/// Rows AppKit should be showing: the model walked with AppKit's own expansion answers.
fn expected_rows(state: &SidebarState) -> NSInteger {
    let outline = &state.views.outline;
    let model = state.shared.model.borrow();
    let mut total = 0;
    let mut stack: Vec<u64> = model
        .roots
        .iter()
        .rev()
        .map(|&i| model.nodes[i].key)
        .collect();
    while let Some(key) = stack.pop() {
        total += 1;
        let Some(node) = model.node(key) else {
            continue;
        };
        if node.children.is_empty() {
            continue;
        }
        let item = state.shared.item(key);
        if unsafe { outline.isItemExpanded(Some(&item)) } {
            for &child in node.children.iter().rev() {
                stack.push(model.nodes[child].key);
            }
        }
    }
    total as NSInteger
}

struct SavedState {
    selected: Option<u64>,
    expanded: Vec<u64>,
    scroll: NSPoint,
}

impl SavedState {
    fn capture(state: &SidebarState) -> Self {
        let outline = &state.views.outline;
        let model = state.shared.model.borrow();
        let selected = {
            let row = outline.selectedRow();
            (row >= 0)
                .then(|| outline.itemAtRow(row))
                .flatten()
                .map(|item| Shared::key_of_item(&item))
        };
        let expanded = model
            .preorder()
            .into_iter()
            .filter(|&key| {
                let item = state.shared.item(key);
                unsafe { outline.isItemExpanded(Some(&item)) }
            })
            .collect();
        Self {
            selected,
            expanded,
            scroll: state.views.scroll.contentView().bounds().origin,
        }
    }

    fn restore(&self, state: &SidebarState) {
        let outline = &state.views.outline;
        for &key in &self.expanded {
            if state.shared.model.borrow().node(key).is_none() {
                continue;
            }
            let item = state.shared.item(key);
            unsafe { outline.expandItem(Some(&item)) };
        }
        if let Some(key) = self.selected {
            if state.shared.model.borrow().node(key).is_some() {
                let item = state.shared.item(key);
                let row = unsafe { outline.rowForItem(Some(&item)) };
                if row >= 0 {
                    state.shared.suppress.set(state.shared.suppress.get() + 1);
                    outline.selectRowIndexes_byExtendingSelection(
                        &NSIndexSet::indexSetWithIndex(row as usize),
                        false,
                    );
                    state.shared.suppress.set(state.shared.suppress.get() - 1);
                }
            }
        }
        let clip = state.views.scroll.contentView();
        clip.scrollToPoint(self.scroll);
        state.views.scroll.reflectScrolledClipView(&clip);
    }
}

fn build(window: &mut Window, cx: &mut App, options: SidebarOptions) -> NativeSidebar {
    let mtm = MainThreadMarker::new().expect("gpui runs its windows on the main thread");
    let gpui = gpui_view(window);
    let ns_window = gpui.window().expect("gpui's view is in a window");
    let old_content = unsafe { gpui.superview() }.expect("gpui's view has a superview");
    let content_size = ns_window
        .contentView()
        .map(|v| v.frame().size)
        .unwrap_or_else(|| old_content.frame().size);

    let shared = Rc::new(Shared {
        model: RefCell::new(Model::default()),
        interned: RefCell::new(HashMap::new()),
        on_select: RefCell::new(None),
        on_reorder: RefCell::new(None),
        on_eject: RefCell::new(None),
        suppress: Cell::new(0),
        writes: Cell::new(0),
        cx: RefCell::new(window.to_async(cx)),
        expand_groups: Cell::new(options.expand_groups),
        outline: RefCell::new(None),
    });
    // Two classes, one of which merely *has* the drag methods. See [`ReorderSource`].
    let (source, data_source, delegate) = match options.reorderable {
        true => {
            let this = ReorderSource::new(mtm, shared.clone());
            (
                Retained::into_super(this.clone()),
                ProtocolObject::from_retained(this.clone()),
                ProtocolObject::from_retained(this),
            )
        }
        false => {
            let this = OutlineSource::new(mtm, shared.clone());
            (
                Retained::into_super(this.clone()),
                ProtocolObject::from_retained(this.clone()),
                ProtocolObject::from_retained(this),
            )
        }
    };

    let strip = NSRect::new(
        NSPoint::ZERO,
        NSSize::new(options.thickness.to_f64(), content_size.height),
    );
    let outline = NSOutlineView::initWithFrame(NSOutlineView::alloc(mtm), strip);
    let column = NSTableColumn::initWithIdentifier(
        NSTableColumn::alloc(mtm),
        &NSString::from_str("NGSidebarColumn"),
    );
    // Both directions fail visibly: at `NSTableColumn`'s 100pt default a single-column outline
    // view never widens and truncates labels that would fit, while seeding the sidebar's thickness
    // overshoots because `.sourceList` adds 16pt of inset per side and scrolls the badge off.
    // Seeded small, the scroll view grows the table to its clip view and the column follows.
    column.setMinWidth(40.);
    column.setWidth(40.);
    column.setMaxWidth(4000.);
    column.setResizingMask(NSTableColumnResizingOptions::AutoresizingMask);
    outline.addTableColumn(&column);
    // Uniform autoresizing would not do: it only redistributes on a frame *change*, and a column
    // seeded narrower than the clip view produces no change to redistribute.
    outline.setColumnAutoresizingStyle(
        NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle,
    );
    unsafe { outline.setOutlineTableColumn(Some(&column)) };
    outline.setHeaderView(None);
    outline.setStyle(NSTableViewStyle::SourceList);
    // After `.sourceList`, never before: setting that style overwrites `rowHeight`,
    // `intercellSpacing` and `indentationPerLevel` with its own source-list seeds.
    outline.setRowSizeStyle(options.row_size.appkit());
    outline.setFloatsGroupRows(false);
    unsafe {
        outline.setDataSource(Some(&data_source));
        outline.setDelegate(Some(&delegate));
    }
    if options.reorderable {
        outline.registerForDraggedTypes(&NSArray::from_slice(&[&*NSString::from_str(
            ROW_PASTEBOARD_TYPE,
        )]));
        // Local only: a row key is meaningless to another process, and offering `Move` outside
        // the app would let a drag land somewhere that cannot answer for it.
        outline.setDraggingSourceOperationMask_forLocal(NSDragOperation::Move, true);
        outline.setDraggingSourceOperationMask_forLocal(NSDragOperation::empty(), false);
    }
    *shared.outline.borrow_mut() = Some(outline.clone());

    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), strip);
    scroll.setDocumentView(Some(&outline));
    scroll.setHasVerticalScroller(true);
    // Mandatory: an opaque scroll view would paint over the sidebar's material.
    scroll.setDrawsBackground(false);
    scroll.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );

    let sidebar_vc = NSViewController::init(NSViewController::alloc(mtm));
    sidebar_vc.setView(&scroll);
    // `behavior` is read-only, so this factory is the only way to get `Sidebar` — and with it the
    // material, full-height layout, traffic-light inset, collapse animation and spring loading.
    let sidebar_item = NSSplitViewItem::sidebarWithViewController(&sidebar_vc);
    sidebar_item.setMinimumThickness(options.min_thickness.to_f64());
    sidebar_item.setMaximumThickness(options.max_thickness.to_f64());
    sidebar_item.setCanCollapse(true);
    sidebar_item.setAllowsFullHeightLayout(true);

    let pane_shared = Rc::new(PaneShared {
        gpui: gpui.clone(),
        align: options.align,
        adopt_below: options.adopt_below_layer,
        intended_gpui: Cell::new(NSRect::new(
            NSPoint::new(f64::NAN, f64::NAN),
            NSSize::new(f64::NAN, f64::NAN),
        )),
        metrics: Cell::new(SidebarMetrics::default()),
        published: Cell::new(SidebarMetrics::default()),
        align_count: Cell::new(0),
        corrections: Cell::new(0),
        cx: RefCell::new(window.to_async(cx)),
    });

    let pane = ContentPane::new(
        mtm,
        NSRect::new(
            NSPoint::ZERO,
            NSSize::new(
                content_size.width - options.thickness.to_f64(),
                content_size.height,
            ),
        ),
        pane_shared.clone(),
    );
    let content_vc = NSViewController::init(NSViewController::alloc(mtm));
    content_vc.setView(&pane);
    let content_item = NSSplitViewItem::splitViewItemWithViewController(&content_vc);

    let split = NSSplitViewController::init(NSSplitViewController::alloc(mtm));
    split.addSplitViewItem(&sidebar_item);
    split.addSplitViewItem(&content_item);

    // Built whether or not it starts attached: `NSWindow.toolbar` is the only place a toolbar
    // lives, so a sidebar that never made one could never honour a later
    // [`NativeSidebar::set_toolbar`].
    let toolbar_delegate: Retained<SidebarToolbarDelegate> =
        unsafe { msg_send![SidebarToolbarDelegate::alloc(mtm), init] };
    let toolbar = NSToolbar::initWithIdentifier(
        NSToolbar::alloc(mtm),
        &NSString::from_str("NGSidebarToolbar"),
    );
    toolbar.setDelegate(Some(ProtocolObject::from_ref(&*toolbar_delegate)));
    // A fresh NSToolbar defaults to icon *and* label — the pre-unified look, 14pt taller.
    toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
    // `toggleSidebar:` walks the responder chain to the split controller, so no target wiring.
    unsafe {
        toolbar.insertItemWithItemIdentifier_atIndex(NSToolbarToggleSidebarItemIdentifier, 0)
    };
    ns_window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    if options.toolbar {
        ns_window.setToolbar(Some(&toolbar));
    }

    ns_window.setStyleMask(ns_window.styleMask() | NSWindowStyleMask::FullSizeContentView);
    ns_window.setTitlebarAppearsTransparent(true);

    // Collected before `setContentViewController:` replaces the old content view and orphans the
    // host's below-containers, which are direct subviews of it.
    let orphans: Vec<Retained<NSView>> = if options.adopt_below_layer {
        old_content
            .subviews()
            .iter()
            .filter(|view| view.isKindOfClass(Container::class()))
            .map(|view| view.retain())
            .collect()
    } else {
        Vec::new()
    };

    gpui.removeFromSuperview();
    match options.align {
        // GPUI's view arrives sized to the old content view by autoresizing. Inside the pane its
        // frame is computed, so autoresizing is a competing writer and must go.
        PaneAlign::WindowAligned => gpui.setAutoresizingMask(NSAutoresizingMaskOptions::empty()),
        PaneAlign::FillPane => gpui.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        ),
    }
    ns_window.setContentViewController(Some(&split));
    split.view().layoutSubtreeIfNeeded();
    let backdrop = options.backdrop.map(|material| {
        let host = split.view();
        let view = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), host.bounds());
        view.setMaterial(RawMaterial::from(material).appkit());
        view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        // Pinned Active, not the default: a backdrop that flattens the moment the window stops
        // being key would take the whole window with it.
        view.setState(NSVisualEffectState::Active);
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        // `NSSplitViewController`'s view is a plain container holding only the `NSSplitView`, so a
        // sibling below that spans the window — under both panes and the titlebar strip, clipped
        // by the window's rounded corners — and is not read as an arranged pane.
        let first = host.subviews().firstObject();
        host.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Below, first.as_deref());
        view
    });
    pane.addSubview(&gpui);
    for orphan in &orphans {
        orphan.removeFromSuperview();
        pane.addSubview_positioned_relativeTo(orphan, NSWindowOrderingMode::Below, Some(&gpui));
    }
    split.view().layoutSubtreeIfNeeded();
    pane_shared.align(&pane);
    // Without this, keyboard input is gone: `setContentViewController:` reset first responder.
    ns_window.makeFirstResponder(Some(&*gpui));

    if options.collapsed {
        sidebar_item.setCollapsed(true);
        split.view().layoutSubtreeIfNeeded();
    }

    // A toolbar, FullSizeContentView and a content view controller each move and resize the
    // window, so the content rect has to be put back.
    ns_window.setContentSize(content_size);
    split.view().layoutSubtreeIfNeeded();
    pane_shared.align(&pane);
    outline.sizeLastColumnToFit();

    let state = SidebarState {
        views: Views {
            outline,
            scroll,
            split,
            sidebar_item,
            pane,
            _source: source,
            toolbar,
            _toolbar_delegate: toolbar_delegate,
            _sidebar_vc: sidebar_vc,
            _content_vc: content_vc,
            _content_item: content_item,
            backdrop,
        },
        shared,
        pane_shared,
    };
    NativeSidebar(Rc::new(state))
}

impl OutlineSource {
    fn new(mtm: MainThreadMarker, shared: Rc<Shared>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(shared);
        unsafe { msg_send![super(this), init] }
    }
}

impl ReorderSource {
    fn new(mtm: MainThreadMarker, shared: Rc<Shared>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(shared);
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The eight `SidebarAirDrop` reps as the bundle ships them: 16/18/24/32 points, each at both
    /// scales.
    const AIRDROP_REP_POINTS: [f64; 8] = [16., 16., 18., 18., 24., 24., 32., 32.];

    #[test]
    fn a_medium_icon_box_resolves_to_the_three_arc_airdrop_rep() {
        // A Medium box is 19pt and the reps are 16/18/24/32, so the box picks the drawing. 30
        // resolving up to 32 is the arm that matters: 32 is the four-arc AirDrop, and it must be
        // reachable when something really is that big rather than clamped away.
        assert_eq!(
            nearest_rep_points(
                AIRDROP_REP_POINTS,
                SidebarRowSize::icon_points(NSTableViewRowSizeStyle::Medium)
            ),
            Some(18.)
        );
        assert_eq!(
            nearest_rep_points(
                AIRDROP_REP_POINTS,
                SidebarRowSize::icon_points(NSTableViewRowSizeStyle::Small)
            ),
            Some(16.)
        );
        assert_eq!(nearest_rep_points(AIRDROP_REP_POINTS, 30.), Some(32.));
        assert_eq!(nearest_rep_points([], 19.), None);
    }

    #[test]
    fn the_core_types_bundle_yields_a_sized_template() {
        let image = build_core_types_template("SidebarAirDrop", 19.)
            .expect("CoreTypes.bundle is world-readable and ships SidebarAirDrop");
        assert!(image.isTemplate());
        assert_eq!(image.size(), NSSize::new(18., 18.));
        let points: Vec<f64> = image
            .representations()
            .iter()
            .map(|rep| rep.size().width)
            .collect();
        assert_eq!(points, vec![18., 18.], "both scales of one point size");
        assert!(build_core_types_template("SidebarNoSuchThing", 19.).is_none());
    }
}
