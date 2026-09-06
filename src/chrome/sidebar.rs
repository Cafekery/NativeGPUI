//! A system-drawn macOS sidebar: an `NSSplitViewItem` shell installed around GPUI's view.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{AnyWindowHandle, App, AsyncWindowContext, Global, Pixels, SharedString, Window, px};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol, ProtocolObject};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBezierPath, NSBundleImageExtension, NSButton,
    NSCellImagePosition, NSColor, NSControl, NSControlTextEditingDelegate, NSDragOperation,
    NSDraggingContext, NSDraggingInfo, NSDraggingItem, NSDraggingSession, NSDraggingSource, NSFont,
    NSFontWeightRegular, NSGraphicsContext, NSImage, NSImageAlignment, NSImageScaling,
    NSImageSymbolConfiguration, NSImageView, NSLineBreakMode, NSMenu, NSMenuDelegate,
    NSOutlineView, NSOutlineViewDataSource, NSOutlineViewDelegate, NSOutlineViewDropOnItemIndex,
    NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardWriting, NSResponder, NSScreen,
    NSScrollView, NSSplitViewController, NSSplitViewItem, NSTableCellView, NSTableColumn,
    NSTableColumnResizingOptions, NSTableView, NSTableViewAnimationOptions,
    NSTableViewColumnAutoresizingStyle, NSTableViewRowSizeStyle, NSTableViewStyle, NSTextAlignment,
    NSTextField, NSToolbar, NSToolbarDelegate, NSToolbarDisplayMode,
    NSToolbarToggleSidebarItemIdentifier, NSUserInterfaceItemIdentification, NSView,
    NSViewController, NSVisualEffectBlendingMode, NSVisualEffectState, NSVisualEffectView,
    NSWindingRule, NSWindow, NSWindowOrderingMode, NSWindowStyleMask, NSWindowToolbarStyle,
    NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSBundle, NSIndexSet, NSInteger, NSMutableIndexSet, NSNotification,
    NSNumber, NSObject, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use objc2_quartz_core::CAAutoresizingMask;

use crate::Material;
use crate::bridge::appkit::{Container, gpui_view, rects_equal};
use crate::controls::MenuEntry;
use crate::materials::RawMaterial;
use crate::tokens;

/// Gap between a row's label and its trailing count badge, in points.
pub const BADGE_GAP: f64 = 6.0;

pub const CORE_TYPES_BUNDLE: &str = "/System/Library/CoreServices/CoreTypes.bundle";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SidebarIcon {
    Symbol(SharedString),
    SystemResource(SharedString),
    /// A small circle, as Finder draws a tag: filled in this sRGB colour when it has one, an
    /// outline when it does not. Unlike [`SidebarIcon::Symbol`], a filled dot keeps its colour
    /// when the row is unselected or the window isn't key, since the colour is the tag's identity.
    Dot(Option<(u8, u8, u8)>),
    /// Two overlapping rings, as Finder draws the row that opens every tag.
    Dots,
}

#[derive(Clone, Debug)]
pub struct SidebarItem {
    id: SharedString,
    title: SharedString,
    icon: Option<SidebarIcon>,
    badge: Option<i64>,
    eject: bool,
    drops: bool,
    group: bool,
    children: Vec<SidebarItem>,
}

pub fn sidebar_item(id: impl Into<SharedString>, title: impl Into<SharedString>) -> SidebarItem {
    SidebarItem {
        id: id.into(),
        title: title.into(),
        icon: None,
        badge: None,
        eject: false,
        drops: false,
        group: false,
        children: Vec::new(),
    }
}

pub fn sidebar_group(id: impl Into<SharedString>, title: impl Into<SharedString>) -> SidebarItem {
    SidebarItem {
        group: true,
        ..sidebar_item(id, title)
    }
}

impl SidebarItem {
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

    pub fn ejectable(mut self, ejectable: bool) -> Self {
        self.eject = ejectable;
        self
    }

    /// Whether files dropped on the row itself, rather than between rows, are the row's to keep.
    pub fn accepts_drops(mut self, accepts: bool) -> Self {
        self.drops = accepts;
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
    drops: bool,
    group: bool,
    parent: Option<usize>,
    children: Vec<usize>,
}

impl Node {
    fn content(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.title.hash(&mut hasher);
        self.icon.hash(&mut hasher);
        self.badge.hash(&mut hasher);
        self.eject.hash(&mut hasher);
        self.drops.hash(&mut hasher);
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
                drops: item.drops,
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

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
/// How GPUI's view is framed inside the content pane.
pub enum PaneAlign {
    #[default]
    WindowAligned,
    FillPane,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
/// Row height, icon box, label font and group-header font, as one `NSTableViewRowSizeStyle`.
pub enum SidebarRowSize {
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

    fn icon_points(style: NSTableViewRowSizeStyle) -> f64 {
        match style {
            NSTableViewRowSizeStyle::Small => 15.,
            NSTableViewRowSizeStyle::Large => 21.,
            _ => 19.,
        }
    }
}

#[derive(Clone, Debug)]
/// Configuration honoured on the first [`NativeSidebar::install`] for a window.
pub struct SidebarOptions {
    pub thickness: Pixels,
    pub min_thickness: Pixels,
    pub max_thickness: Pixels,
    pub toolbar: bool,
    pub collapsed: bool,
    pub expand_groups: bool,
    pub row_size: SidebarRowSize,
    pub align: PaneAlign,
    /// Re-parent [`crate::Layer::Below`] containers into the content pane and mirror GPUI's frame.
    pub adopt_below_layer: bool,
    pub reorderable: bool,
    /// Window-spanning vibrancy beneath both panes; only the sidebar strip is left showing it.
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

#[derive(Copy, Clone, PartialEq, Debug, Default)]
/// Insets the sidebar imposes on GPUI's layout, in GPUI-logical points.
pub struct SidebarMetrics {
    pub left: Pixels,
    pub top: Pixels,
    pub right: Pixels,
    pub bottom: Pixels,
    pub thickness: Pixels,
    pub collapsed: bool,
}

impl Global for SidebarMetrics {}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
/// What one [`NativeSidebar::update`] wrote to AppKit.
pub struct SidebarDiff {
    pub unchanged: bool,
    pub reloaded_items: usize,
    pub inserted: usize,
    pub removed: usize,
    pub moved: usize,
    pub full_reloads: usize,
}

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
    /// What `Default` resolved to; never -1.
    pub effective_row_size_style: isize,
    pub row_height: f64,
    pub indentation_per_level: f64,
    pub divider_thickness: f64,
    pub rows: isize,
    /// `NSVisualEffectMaterial` of the window-spanning backdrop, or -1 without one.
    pub backdrop_material: isize,
    pub toolbar_items: usize,
    pub scroll_draws_background: bool,
    pub outline_is_first_responder: bool,
}

#[derive(Copy, Clone, PartialEq, Debug)]
/// Where AppKit placed one row's parts: `row` and `cell` in the outline view, the rest in the cell.
pub struct RowLayout {
    pub row: NSRect,
    pub cell: NSRect,
    pub icon: NSRect,
    pub title: NSRect,
    pub badge: NSRect,
    pub level: isize,
    pub expanded: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
/// Files dropped on the sidebar: what they were, the group they landed in, and where in it.
pub struct SidebarDrop {
    pub paths: Vec<PathBuf>,
    pub group: SharedString,
    pub index: usize,
    /// The row the files landed on, when it is one that [`SidebarItem::accepts_drops`].
    pub onto: Option<SharedString>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SidebarMove {
    pub id: SharedString,
    pub parent: Option<SharedString>,
    pub index: usize,
}

type SelectHandler = Rc<dyn Fn(&Option<SharedString>, &mut Window, &mut App)>;
type ReorderHandler = Rc<dyn Fn(&SidebarMove, &mut Window, &mut App)>;
type EjectHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;
type DropHandler = Rc<dyn Fn(&SidebarDrop, &mut Window, &mut App)>;
type MenuHandler = Rc<dyn Fn(&SharedString) -> Vec<MenuEntry>>;
type MenuPickHandler = Rc<dyn Fn(&SharedString, usize, &mut Window, &mut App)>;

struct Shared {
    model: RefCell<Model>,
    interned: RefCell<HashMap<u64, Retained<NSNumber>>>,
    on_select: RefCell<Option<SelectHandler>>,
    on_reorder: RefCell<Option<ReorderHandler>>,
    on_eject: RefCell<Option<EjectHandler>>,
    on_drop_paths: RefCell<Option<DropHandler>>,
    on_menu: RefCell<Option<(MenuHandler, MenuPickHandler)>>,
    /// The row the open contextual menu was raised on, so its pick knows what it acts on.
    menu_row: RefCell<Option<SharedString>>,
    /// Non-zero while we drive the selection ourselves, so the round-trip does not echo.
    suppress: Cell<u32>,
    writes: Cell<u64>,
    cx: RefCell<AsyncWindowContext>,
    expand_groups: Cell<bool>,
    outline: RefCell<Option<Retained<NSOutlineView>>>,
}

impl Shared {
    /// One `NSNumber` per key: `NSOutlineView` keys expansion and selection on item identity.
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

struct Views {
    outline: Retained<NSOutlineView>,
    scroll: Retained<NSScrollView>,
    split: Retained<NSSplitViewController>,
    sidebar_item: Retained<NSSplitViewItem>,
    pane: Retained<ContentPane>,
    _source: Retained<NSObject>,
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

#[derive(Clone)]
/// A handle on the window's sidebar. Cheap to clone; every clone is the same sidebar.
pub struct NativeSidebar(Rc<SidebarState>);

#[derive(Default)]
struct Sidebars(HashMap<AnyWindowHandle, NativeSidebar>);
impl Global for Sidebars {}

impl NativeSidebar {
    /// Install the shell, or return the one already there; a second call ignores `options`.
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
    pub fn update(&self, items: &[SidebarItem]) -> SidebarDiff {
        let state = &*self.0;
        state.pane_shared.clone().align(&state.views.pane);

        let next = Model::build(items);
        // Model first, AppKit second: every delta call in `apply_diff` re-enters the data source.
        let plan = plan_diff(&state.shared.model.borrow(), &next);
        *state.shared.model.borrow_mut() = next;
        if plan.is_empty() {
            return SidebarDiff {
                unchanged: true,
                ..Default::default()
            };
        }
        apply_diff(state, plan)
    }

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

    /// Called when the *user* changes the selection, deferred to the next `App` update.
    pub fn on_select(
        &self,
        handler: impl Fn(&Option<SharedString>, &mut Window, &mut App) + 'static,
    ) {
        *self.0.shared.on_select.borrow_mut() = Some(Rc::new(handler));
    }

    /// The menu a right-click on a row drops, and what one of its rows runs when picked.
    pub fn on_menu(
        &self,
        entries: impl Fn(&SharedString) -> Vec<MenuEntry> + 'static,
        picked: impl Fn(&SharedString, usize, &mut Window, &mut App) + 'static,
    ) {
        *self.0.shared.on_menu.borrow_mut() = Some((Rc::new(entries), Rc::new(picked)));
    }

    /// Called after the user drags a row, and only with [`SidebarOptions::reorderable`].
    pub fn on_reorder(&self, handler: impl Fn(&SidebarMove, &mut Window, &mut App) + 'static) {
        *self.0.shared.on_reorder.borrow_mut() = Some(Rc::new(handler));
    }

    /// Called with the row's id when the user clicks its eject affordance; unmounts nothing.
    /// Files dropped on a group: the sidebar draws the insertion line, the app keeps the row.
    pub fn on_drop_paths(&self, handler: impl Fn(&SidebarDrop, &mut Window, &mut App) + 'static) {
        *self.0.shared.on_drop_paths.borrow_mut() = Some(Rc::new(handler));
    }

    pub fn on_eject(&self, handler: impl Fn(&SharedString, &mut Window, &mut App) + 'static) {
        *self.0.shared.on_eject.borrow_mut() = Some(Rc::new(handler));
    }

    /// Collapse or expand with AppKit's own animation.
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

    pub fn set_expand_groups(&self, expand: bool) {
        self.0.shared.expand_groups.set(expand);
        if expand {
            expand_groups(&self.0);
        }
    }

    pub fn expands_groups(&self) -> bool {
        self.0.shared.expand_groups.get()
    }

    /// Retint the window-spanning backdrop; a sidebar installed with `backdrop: None` ignores this.
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

    pub fn metrics(&self) -> SidebarMetrics {
        self.0.pane_shared.metrics.get()
    }

    pub fn align_count(&self) -> u64 {
        self.0.pane_shared.align_count.get()
    }

    /// Times the alignment found GPUI's frame changed by someone else and repaired it.
    pub fn frame_corrections(&self) -> u64 {
        self.0.pane_shared.corrections.get()
    }

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
            // The window's toolbar, not the retained one: only it knows if one is attached.
            toolbar_items: outline
                .window()
                .and_then(|w| w.toolbar())
                .map_or(0, |t| t.items().count()),
            scroll_draws_background: views.scroll.drawsBackground(),
            outline_is_first_responder: first_responder,
        }
    }

    /// Height of the screen whose bottom-left is Cocoa's global origin.
    pub fn screen_height(&self) -> f64 {
        let mtm = MainThreadMarker::new()
            .expect("a NativeSidebar is !Send, so it is only reachable on gpui's main thread");
        NSScreen::screens(mtm)
            .firstObject()
            .map_or(0., |screen| screen.frame().size.height)
    }

    pub fn window_frame_window(&self) -> Option<Retained<NSWindow>> {
        self.0.pane_shared.gpui.window()
    }

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

    /// The centre of a row in GPUI-logical points from the content view's top-left.
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
        // The pane's own `frame.origin.x` reads 0 while it sits a sidebar's width in.
        let pane_in_cv = pane.convertRect_toView(pane.bounds(), Some(&content_view));
        let safe = pane.safeAreaInsets();

        let wanted = match self.align {
            PaneAlign::WindowAligned => NSRect::new(
                NSPoint::new(-pane_in_cv.origin.x, -pane_in_cv.origin.y),
                cv_size,
            ),
            PaneAlign::FillPane => pane.bounds(),
        };
        // The split view's item wrapper clips, and so would hide the part of GPUI's view that
        // sits under the floating sidebar; unclipped, the sidebar's glass blurs it, as Finder's.
        if self.align == PaneAlign::WindowAligned {
            for view in [Some(pane.retain()), unsafe { pane.superview() }]
                .into_iter()
                .flatten()
            {
                if view.clipsToBounds() {
                    view.setClipsToBounds(false);
                }
            }
        }
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

    fn sync_below(&self, pane: &NSView, gpui_frame: NSRect) {
        for view in pane.subviews().iter() {
            if view.isKindOfClass(Container::class()) && !rects_equal(view.frame(), gpui_frame) {
                view.setFrame(gpui_frame);
            }
        }
    }
}

define_class!(
    /// Finder's sidebar never takes the keyboard: a click selects and the content keeps focus, so
    /// the selected row wears the source list's quiet pill, not the accent one of a focused table.
    #[unsafe(super(NSOutlineView, NSTableView, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSidebarOutline"]
    struct SidebarOutline;

    impl SidebarOutline {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            false
        }
    }
);

impl SidebarOutline {
    fn init_with_frame(this: objc2::rc::Allocated<Self>, frame: NSRect) -> Retained<Self> {
        unsafe { msg_send![this, initWithFrame: frame] }
    }
}

define_class!(
    /// Clips GPUI's oversized child to the pane and re-aligns it on every geometry change.
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

        // setFrame: is not implemented in terms of setFrameSize:/setFrameOrigin:; all four matter.
        #[unsafe(method(setFrame:))]
        fn set_frame(&self, frame: NSRect) {
            let _: () = unsafe { msg_send![super(self), setFrame: frame] };
            self.ivars().clone().align(self);
        }

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
    group: bool,
    badge: RefCell<Option<Retained<NSTextField>>>,
    eject: RefCell<Option<Retained<NSButton>>>,
    /// A tag row's own colour: what Finder draws its label in while the row is selected.
    tint: RefCell<Option<Retained<NSColor>>>,
}

impl CellIvars {
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
    #[unsafe(super(NSTableCellView))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSidebarCell"]
    #[ivars = CellIvars]
    struct SidebarCell;

    impl SidebarCell {
        #[unsafe(method(layout))]
        fn layout(&self) {
            let _: () = unsafe { msg_send![super(self), layout] };

            // AppKit restyles a group row's label as it displays, so both the drop and the
            // colour have to be re-applied after its own layout, not once at build time.
            if self.ivars().group {
                if let Some(title) = unsafe { self.textField() } {
                    let frame = title.frame();
                    title.setFrameOrigin(NSPoint::new(
                        frame.origin.x,
                        frame.origin.y - GROUP_BASELINE_DROP,
                    ));
                    title.setTextColor(Some(&NSColor::tertiaryLabelColor()));
                }
            }

            // Never hide the image view: that collapses AppKit's icon box and shifts the label.
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

// `NSTableView` keeps one recycling queue per identifier, so rows are split by shape.
const ID_GROUP: &str = "NGSidebarGroupRow";
const ID_ICON: &str = "NGSidebarIconRow";
const ID_PLAIN: &str = "NGSidebarPlainRow";

fn label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    field.setUsesSingleLineMode(true);
    field.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    field
}

const EJECT_POINTS: f64 = 10.0;
const TAG_DOT_DIAMETER: f64 = 12.5;
const TAG_DOT_LINE: f64 = 1.0;
const TAG_DOTS_OVERLAP: f64 = 4.5;
/// Finder sets its section headers on the row's baseline, where AppKit centres them.
const GROUP_BASELINE_DROP: f64 = 2.0;

fn make_cell(
    mtm: MainThreadMarker,
    identifier: &str,
    with_icon: bool,
    target: &AnyObject,
) -> Retained<SidebarCell> {
    let this = SidebarCell::alloc(mtm).set_ivars(CellIvars {
        group: identifier == ID_GROUP,
        badge: RefCell::new(None),
        eject: RefCell::new(None),
        tint: RefCell::new(None),
    });
    let cell: Retained<SidebarCell> = unsafe { msg_send![super(this), init] };
    cell.setIdentifier(Some(&NSString::from_str(identifier)));

    let title = label(mtm, "");
    cell.addSubview(&title);
    unsafe { cell.setTextField(Some(&title)) };

    if with_icon {
        let icon = NSImageView::new(mtm);
        icon.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        icon.setImageAlignment(NSImageAlignment::AlignCenter);
        cell.addSubview(&icon);
        unsafe { cell.setImageView(Some(&icon)) };
    }

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
            &NSString::from_str("eject.fill"),
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
    eject.setContentTintColor(Some(&NSColor::quaternaryLabelColor()));
    eject.setHidden(true);
    unsafe {
        eject.setTarget(Some(target));
        eject.setAction(Some(sel!(ngSidebarEject:)));
    }
    cell.addSubview(&eject);
    *cell.ivars().eject.borrow_mut() = Some(eject);

    cell
}

fn core_types_template(name: &SharedString, points: f64) -> Option<Retained<NSImage>> {
    thread_local! {
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

fn nearest_rep_points(available: impl IntoIterator<Item = f64>, wanted: f64) -> Option<f64> {
    available.into_iter().fold(None, |held, candidate| {
        let better = held.is_none_or(|held: f64| {
            let (a, b) = ((candidate - wanted).abs(), (held - wanted).abs());
            a < b || (a == b && candidate < held)
        });
        Some(if better { candidate } else { held? })
    })
}

/// A [`CORE_TYPES_BUNDLE`] resource as a template image, at the representation nearest `points`.
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
        // One point size only: 16pt@2x and 32pt@1x are both 32px but are different drawings.
        if rep.size().width == chosen {
            image.addRepresentation(&rep);
        }
    }
    // The bundle's images arrive with `isTemplate` false, so nothing would tint them.
    image.setTemplate(true);
    Some(image)
}

fn ring_path(at: NSRect) -> Retained<NSBezierPath> {
    let ring = NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
        NSPoint::new(
            at.origin.x + TAG_DOT_LINE / 2.,
            at.origin.y + TAG_DOT_LINE / 2.,
        ),
        NSSize::new(at.size.width - TAG_DOT_LINE, at.size.height - TAG_DOT_LINE),
    ));
    ring.setLineWidth(TAG_DOT_LINE);
    ring
}

/// Finder's tag dot: a disc in the tag's own colour when it has one, a template-tinted ring when
/// it does not, a pair of rings for every tag at once. Drawn rather than an SF Symbol, which the
/// cell would baseline-align, not centre.
fn tag_dot_image(icon: &SidebarIcon) -> Option<Retained<NSImage>> {
    let color = match icon {
        SidebarIcon::Dot(color) => *color,
        SidebarIcon::Dots => None,
        _ => return None,
    };
    let pair = matches!(icon, SidebarIcon::Dots);
    thread_local! {
        static DOTS: RefCell<HashMap<(Option<(u8, u8, u8)>, bool), Retained<NSImage>>> =
            RefCell::new(HashMap::new());
    }
    if let Some(hit) = DOTS.with_borrow(|cache| cache.get(&(color, pair)).cloned()) {
        return Some(hit);
    }
    let draw = block2::RcBlock::new(move |rect: NSRect| -> Bool {
        let disc = NSRect::new(rect.origin, NSSize::new(TAG_DOT_DIAMETER, TAG_DOT_DIAMETER));
        match color {
            Some((r, g, b)) => {
                NSColor::colorWithSRGBRed_green_blue_alpha(
                    r as f64 / 255.,
                    g as f64 / 255.,
                    b as f64 / 255.,
                    1.,
                )
                .setFill();
                NSBezierPath::bezierPathWithOvalInRect(disc).fill();
            }
            None => {
                NSColor::blackColor().setStroke();
                if pair {
                    let behind = NSRect::new(
                        NSPoint::new(disc.origin.x + TAG_DOTS_OVERLAP, disc.origin.y),
                        disc.size,
                    );
                    let clip = NSBezierPath::bezierPathWithRect(rect);
                    clip.appendBezierPathWithOvalInRect(disc);
                    clip.setWindingRule(NSWindingRule::EvenOdd);
                    NSGraphicsContext::saveGraphicsState_class();
                    clip.addClip();
                    ring_path(behind).stroke();
                    NSGraphicsContext::restoreGraphicsState_class();
                }
                ring_path(disc).stroke();
            }
        }
        Bool::YES
    });
    let width = TAG_DOT_DIAMETER + if pair { TAG_DOTS_OVERLAP } else { 0. };
    let image = NSImage::imageWithSize_flipped_drawingHandler(
        NSSize::new(width, TAG_DOT_DIAMETER),
        false,
        &draw,
    );
    image.setTemplate(color.is_none());
    DOTS.with_borrow_mut(|cache| cache.insert((color, pair), image.clone()));
    Some(image)
}

/// Finder draws a selected tag row's label in the tag's own colour, never the accent one.
fn tint_cell(cell: &SidebarCell, selected: bool) {
    let Some(title) = (unsafe { cell.textField() }) else {
        return;
    };
    let tint = cell.ivars().tint.borrow().clone().filter(|_| selected);
    title.setTextColor(Some(tint.as_deref().unwrap_or(&NSColor::labelColor())));
}

fn tint_selected_rows(outline: &NSOutlineView) {
    for row in 0..outline.numberOfRows() {
        let Some(view) = outline.viewAtColumn_row_makeIfNecessary(0, row, false) else {
            continue;
        };
        if view.isKindOfClass(SidebarCell::class()) {
            let cell = unsafe { Retained::cast_unchecked::<SidebarCell>(view) };
            tint_cell(&cell, outline.isRowSelected(row));
        }
    }
}

fn configure_cell(cell: &SidebarCell, node: &Node, icon_points: f64) {
    if let Some(title) = unsafe { cell.textField() } {
        let wanted = NSString::from_str(&node.title);
        if title.stringValue() != wanted {
            title.setStringValue(&wanted);
        }
    }

    *cell.ivars().tint.borrow_mut() = match node.icon {
        Some(SidebarIcon::Dot(Some((r, g, b)))) => {
            Some(NSColor::colorWithSRGBRed_green_blue_alpha(
                r as f64 / 255.,
                g as f64 / 255.,
                b as f64 / 255.,
                1.,
            ))
        }
        _ => None,
    };

    if let Some(view) = unsafe { cell.imageView() } {
        let image = match &node.icon {
            Some(SidebarIcon::Symbol(name)) => {
                NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(name),
                    None,
                )
            }
            Some(SidebarIcon::SystemResource(name)) => core_types_template(name, icon_points),
            Some(icon @ (SidebarIcon::Dot(_) | SidebarIcon::Dots)) => tag_dot_image(icon),
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
    let row = unsafe { outline.rowForItem(Some(item)) };
    tint_cell(&cell, outline.isRowSelected(row));
    Some(unsafe { Retained::cast_unchecked::<NSView>(cell) })
}

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
        if let Some(outline) = shared.outline.borrow().clone() {
            tint_selected_rows(&outline);
        }
        if shared.suppress.get() > 0 {
            return;
        }
        let Some(handler) = shared.on_select.borrow().clone() else {
            return;
        };
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

    /// Finder builds a sidebar row's menu when the click lands, not when the sidebar is made.
    pub(super) fn menu_needs_update(shared: &Rc<Shared>, menu: &NSMenu, target: &AnyObject) {
        let mtm = MainThreadMarker::new().expect("outline view callbacks are main-thread");
        let row = shared
            .outline
            .borrow()
            .as_ref()
            .map(|outline| outline.clickedRow())
            .filter(|&row| row >= 0)
            .and_then(|row| {
                let outline = shared.outline.borrow().clone()?;
                let item = outline.itemAtRow(row)?;
                shared.model.borrow().id_for(Shared::key_of_item(&item))
            });
        *shared.menu_row.borrow_mut() = row.clone();
        let entries = match (row, shared.on_menu.borrow().clone()) {
            (Some(row), Some((entries, _))) => entries(&row),
            _ => Vec::new(),
        };
        crate::controls::menu::sync_menu(
            mtm,
            menu,
            &entries,
            Some((target, sel!(ngSidebarMenuPick:))),
        );
    }

    pub(super) fn menu_picked(shared: &Rc<Shared>, sender: &AnyObject) {
        let index: NSInteger = unsafe { msg_send![sender, tag] };
        let Some(row) = shared.menu_row.borrow().clone() else {
            return;
        };
        let Some((_, picked)) = shared.on_menu.borrow().clone() else {
            return;
        };
        let index = index.max(0) as usize;
        shared
            .cx
            .borrow()
            .spawn(async move |cx| {
                let _ = cx.update(|window, app| picked(&row, index, window, app));
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
            #[unsafe(method(ngSidebarEject:))]
            fn eject_clicked(&self, sender: &NSView) {
                source::eject_clicked(self.ivars(), sender)
            }

            #[unsafe(method(ngSidebarMenuPick:))]
            fn menu_picked(&self, sender: &AnyObject) {
                source::menu_picked(self.ivars(), sender)
            }
        }

        unsafe impl NSMenuDelegate for $class {
            #[unsafe(method(menuNeedsUpdate:))]
            fn menu_needs_update(&self, menu: &NSMenu) {
                source::menu_needs_update(self.ivars(), menu, self.as_ref())
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

const ROW_PASTEBOARD_TYPE: &str = "com.native-gpui.sidebar-row";

fn dragged_key(info: &ProtocolObject<dyn NSDraggingInfo>) -> Option<u64> {
    let text = info
        .draggingPasteboard()
        .stringForType(&NSString::from_str(ROW_PASTEBOARD_TYPE))?;
    text.to_string().parse().ok()
}

/// The file URLs a drag carries, empty for any other drag.
fn dragged_paths(info: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<PathBuf> {
    let pasteboard = info.draggingPasteboard();
    let Some(items) = pasteboard.pasteboardItems() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let text = item.stringForType(unsafe { NSPasteboardTypeFileURL })?;
            let url = NSURL::URLWithString(&text)?;
            Some(PathBuf::from(url.path()?.to_string()))
        })
        .collect()
}

/// Where files land: a group and a row index in it. Never onto a row, which owns no files here.
fn file_drop_target(
    shared: &Rc<Shared>,
    outline: &NSOutlineView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
    item: Option<&AnyObject>,
    index: NSInteger,
) -> Option<(u64, usize)> {
    let (parent, at) = drop_target(shared, outline, info, item, index)?;
    let parent = parent?;
    shared
        .model
        .borrow()
        .node(parent)
        .filter(|node| node.group)
        .map(|_| (parent, at))
}

/// A row that takes files itself, as Finder's tags do: the drop lands on it, not between rows.
fn file_drop_row(shared: &Rc<Shared>, item: Option<&AnyObject>) -> Option<u64> {
    let key = Shared::key_of_item(item?);
    shared
        .model
        .borrow()
        .node(key)
        .filter(|node| node.drops)
        .map(|_| key)
}

fn validate_file_drop(
    shared: &Rc<Shared>,
    outline: &NSOutlineView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
    item: Option<&AnyObject>,
    index: NSInteger,
) -> NSDragOperation {
    if shared.on_drop_paths.borrow().is_none() || dragged_paths(info).is_empty() {
        return NSDragOperation::empty();
    }
    if let Some(key) = file_drop_row(shared, item) {
        unsafe {
            outline.setDropItem_dropChildIndex(
                Some(&shared.item(key)),
                NSOutlineViewDropOnItemIndex as NSInteger,
            )
        };
        return NSDragOperation::Copy;
    }
    let Some((group, at)) = file_drop_target(shared, outline, info, item, index) else {
        return NSDragOperation::empty();
    };
    unsafe { outline.setDropItem_dropChildIndex(Some(&shared.item(group)), at as NSInteger) };
    NSDragOperation::Generic
}

fn accept_file_drop(
    shared: &Rc<Shared>,
    outline: &NSOutlineView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
    item: Option<&AnyObject>,
    index: NSInteger,
) -> bool {
    let paths = dragged_paths(info);
    if paths.is_empty() {
        return false;
    }
    let onto = file_drop_row(shared, item);
    let Some((group, at)) = onto
        .and_then(|key| {
            let model = shared.model.borrow();
            let parent = model.parent_key(key)?;
            let at = model
                .children_keys(Some(parent))
                .iter()
                .position(|&k| k == key)?;
            Some((parent, at))
        })
        .or_else(|| file_drop_target(shared, outline, info, item, index))
    else {
        return false;
    };
    let Some(handler) = shared.on_drop_paths.borrow().clone() else {
        return false;
    };
    let model_ids = {
        let model = shared.model.borrow();
        (model.id_for(group), onto.and_then(|key| model.id_for(key)))
    };
    let (Some(group), onto) = model_ids else {
        return false;
    };
    let dropped = SidebarDrop {
        paths,
        group,
        index: at,
        onto,
    };
    shared
        .cx
        .borrow()
        .spawn(async move |cx| {
            let _ = cx.update(|window, app| handler(&dropped, window, app));
        })
        .detach();
    true
}

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
        return validate_file_drop(shared, outline, info, item, index);
    };
    let Some((parent, at)) = drop_target(shared, outline, info, item, index) else {
        return NSDragOperation::empty();
    };
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
        return accept_file_drop(shared, outline, info, item, index);
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
    // The drop index counts the row itself; both moves count the list with the row lifted out.
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
/// Separate class: defining `pasteboardWriterForItem:` at all moves selection commit to mouse-up.
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
);

/// Hands the drag GPUI is running to AppKit as file URLs: a hosted `NSOutlineView` is a dragging
/// destination and sees nothing of GPUI's own drags.
pub fn drag_files(window: &mut Window, paths: &[PathBuf]) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let view = gpui_view(window);
    let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else {
        return false;
    };
    let at = event.locationInWindow();
    let workspace = NSWorkspace::sharedWorkspace();
    let items: Vec<Retained<NSDraggingItem>> = paths
        .iter()
        .filter_map(|path| {
            let text = NSString::from_str(path.to_str()?);
            let url = NSURL::fileURLWithPath(&text);
            let item = NSDraggingItem::initWithPasteboardWriter(
                NSDraggingItem::alloc(),
                ProtocolObject::from_ref(&*url),
            );
            let frame = NSRect::new(
                NSPoint::new(at.x - DRAG_ICON / 2., at.y - DRAG_ICON / 2.),
                NSSize::new(DRAG_ICON, DRAG_ICON),
            );
            let icon = workspace.iconForFile(&text);
            unsafe { item.setDraggingFrame_contents(frame, Some(&icon)) };
            Some(item)
        })
        .collect();
    if items.is_empty() {
        return false;
    }
    let source = FileDragSource::new(mtm);
    view.beginDraggingSessionWithItems_event_source(
        &NSArray::from_retained_slice(&items),
        &event,
        ProtocolObject::from_ref(&*source),
    );
    // AppKit does not keep the source alive; the session ends when the next one starts.
    DRAG_SOURCE.with(|held| *held.borrow_mut() = Some(source));
    true
}

const DRAG_ICON: f64 = 32.;

thread_local! {
    static DRAG_SOURCE: RefCell<Option<Retained<FileDragSource>>> = const { RefCell::new(None) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSidebarFileDragSource"]
    struct FileDragSource;

    unsafe impl NSObjectProtocol for FileDragSource {}

    unsafe impl NSDraggingSource for FileDragSource {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operation_mask(
            &self,
            _: &NSDraggingSession,
            context: NSDraggingContext,
        ) -> NSDragOperation {
            match context {
                NSDraggingContext::WithinApplication => {
                    NSDragOperation::Generic | NSDragOperation::Move | NSDragOperation::Copy
                }
                _ => NSDragOperation::Copy,
            }
        }
    }
);

impl FileDragSource {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

define_class!(
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
    Reload,
    Content(Vec<u64>),
    Structural {
        content: Vec<u64>,
        /// The child list the *old* model had; the data source already answers from the new one.
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
        unsafe { outline.reloadItem(Some(&item)) };
        diff.reloaded_items += 1;
    }
    shared
        .writes
        .set(shared.writes.get() + diff.reloaded_items as u64);

    diff
}

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
        on_drop_paths: RefCell::new(None),
        on_menu: RefCell::new(None),
        menu_row: RefCell::new(None),
        suppress: Cell::new(0),
        writes: Cell::new(0),
        cx: RefCell::new(window.to_async(cx)),
        expand_groups: Cell::new(options.expand_groups),
        outline: RefCell::new(None),
    });
    let (source, data_source, delegate, menu_delegate) = match options.reorderable {
        true => {
            let this = ReorderSource::new(mtm, shared.clone());
            (
                Retained::into_super(this.clone()),
                ProtocolObject::from_retained(this.clone()),
                ProtocolObject::from_retained(this.clone()),
                ProtocolObject::from_retained(this),
            )
        }
        false => {
            let this = OutlineSource::new(mtm, shared.clone());
            (
                Retained::into_super(this.clone()),
                ProtocolObject::from_retained(this.clone()),
                ProtocolObject::from_retained(this.clone()),
                ProtocolObject::from_retained(this),
            )
        }
    };

    let strip = NSRect::new(
        NSPoint::ZERO,
        NSSize::new(options.thickness.to_f64(), content_size.height),
    );
    let outline: Retained<NSOutlineView> = Retained::into_super(SidebarOutline::init_with_frame(
        SidebarOutline::alloc(mtm),
        strip,
    ));
    let column = NSTableColumn::initWithIdentifier(
        NSTableColumn::alloc(mtm),
        &NSString::from_str("NGSidebarColumn"),
    );
    // Seeded narrow: the scroll view grows the last column out to its clip view.
    column.setMinWidth(40.);
    column.setWidth(40.);
    column.setMaxWidth(4000.);
    column.setResizingMask(NSTableColumnResizingOptions::AutoresizingMask);
    outline.addTableColumn(&column);
    // LastColumnOnly, not Uniform: uniform redistributes only on a frame change, and there is none.
    outline.setColumnAutoresizingStyle(
        NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle,
    );
    unsafe { outline.setOutlineTableColumn(Some(&column)) };
    outline.setHeaderView(None);
    outline.setStyle(NSTableViewStyle::SourceList);
    // After `.sourceList`, never before: that style overwrites `rowHeight` and the indentation.
    outline.setRowSizeStyle(options.row_size.appkit());
    outline.setFloatsGroupRows(false);
    unsafe {
        outline.setDataSource(Some(&data_source));
        outline.setDelegate(Some(&delegate));
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        menu.setDelegate(Some(&menu_delegate));
        outline.setMenu(Some(&menu));
    }
    let mut dragged_types = vec![unsafe { NSPasteboardTypeFileURL }.retain()];
    if options.reorderable {
        dragged_types.push(NSString::from_str(ROW_PASTEBOARD_TYPE));
        outline.setDraggingSourceOperationMask_forLocal(NSDragOperation::Move, true);
        outline.setDraggingSourceOperationMask_forLocal(NSDragOperation::empty(), false);
    }
    outline.registerForDraggedTypes(&NSArray::from_retained_slice(&dragged_types));
    *shared.outline.borrow_mut() = Some(outline.clone());

    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), strip);
    scroll.setDocumentView(Some(&outline));
    scroll.setHasVerticalScroller(true);
    // An opaque scroll view would paint over the sidebar's material.
    scroll.setDrawsBackground(false);
    scroll.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );

    let sidebar_vc = NSViewController::init(NSViewController::alloc(mtm));
    sidebar_vc.setView(&scroll);
    // `behavior` is read-only, so this factory is the only way to get `Sidebar`.
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

    let toolbar_delegate: Retained<SidebarToolbarDelegate> =
        unsafe { msg_send![SidebarToolbarDelegate::alloc(mtm), init] };
    let toolbar = NSToolbar::initWithIdentifier(
        NSToolbar::alloc(mtm),
        &NSString::from_str("NGSidebarToolbar"),
    );
    toolbar.setDelegate(Some(ProtocolObject::from_ref(&*toolbar_delegate)));
    toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
    unsafe {
        toolbar.insertItemWithItemIdentifier_atIndex(NSToolbarToggleSidebarItemIdentifier, 0)
    };
    ns_window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    if options.toolbar {
        ns_window.setToolbar(Some(&toolbar));
    }

    ns_window.setStyleMask(ns_window.styleMask() | NSWindowStyleMask::FullSizeContentView);
    ns_window.setTitlebarAppearsTransparent(true);

    // Collected before `setContentViewController:` orphans the host's below-containers.
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
        // Pinned Active: otherwise the backdrop flattens the moment the window stops being key.
        view.setState(NSVisualEffectState::Active);
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let first = host.subviews().firstObject();
        host.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Below, first.as_deref());
        view
    });
    pane.addSubview(&gpui);
    // GPUI's Metal layer autoresizes with its superlayer, which is right only while the view fills
    // its superview. Here the view is wider than the pane, so AppKit sizes the layer from the view
    // and CA then adds the pane's resize delta on top, and every frame is displayed scaled.
    if let Some(layer) = gpui.layer() {
        layer.setAutoresizingMask(CAAutoresizingMask::empty());
    }
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

    // A toolbar, FullSizeContentView and a content view controller each resize the window.
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

    const AIRDROP_REP_POINTS: [f64; 8] = [16., 16., 18., 18., 24., 24., 32., 32.];

    #[test]
    fn a_medium_icon_box_resolves_to_the_three_arc_airdrop_rep() {
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
