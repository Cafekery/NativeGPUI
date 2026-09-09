mod cell;
mod diff;
mod dragdrop;
mod model;
mod source;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::Hash;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::StackBlock;
use gpui::{AnyWindowHandle, App, AsyncWindowContext, Global, Pixels, SharedString, Window, px};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{ClassType, DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezierPath, NSControl, NSControlTextEditingDelegate,
    NSDragOperation, NSDraggingInfo, NSMenu, NSMenuDelegate, NSOutlineView,
    NSOutlineViewDataSource, NSOutlineViewDelegate, NSPasteboardTypeFileURL, NSPasteboardWriting,
    NSResponder, NSScreen, NSScrollView, NSSplitViewController, NSSplitViewItem, NSTableColumn,
    NSTableColumnResizingOptions, NSTableView, NSTableViewColumnAutoresizingStyle,
    NSTableViewRowSizeStyle, NSTableViewStyle, NSToolbar, NSToolbarDelegate, NSToolbarDisplayMode,
    NSToolbarToggleSidebarItemIdentifier, NSView, NSViewController, NSVisualEffectBlendingMode,
    NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowDidBecomeKeyNotification,
    NSWindowDidResignKeyNotification, NSWindowDidUpdateNotification, NSWindowOrderingMode,
    NSWindowStyleMask, NSWindowToolbarStyle,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSIndexSet, NSInteger, NSNotification, NSNotificationCenter,
    NSNumber, NSObject, NSPoint, NSRect, NSSize, NSString,
};
use objc2_quartz_core::CAAutoresizingMask;

use crate::Material;
use crate::bridge::appkit::{Container, gpui_view, rects_equal};
use crate::controls::MenuEntry;
use crate::materials::RawMaterial;
use crate::tokens;

pub use self::cell::system_template;
use self::cell::{SidebarCell, label_fades_itself, row_view, tint_selected_rows};
use self::diff::{SavedState, apply_diff, expand_groups, plan_diff, reveal};
pub use self::dragdrop::drag_files;
use self::dragdrop::{
    ROW_PASTEBOARD_TYPE, accept_row_drop, row_pasteboard_item, validate_row_drop,
};
use self::model::Model;

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

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
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
            _ => 17.,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SidebarOptions {
    pub thickness: Pixels,
    pub min_thickness: Pixels,
    /// The widest a drag may make the sidebar; at or below `min_thickness` the only cap is what
    /// `content_min_thickness` leaves.
    pub max_thickness: Pixels,
    pub toolbar: bool,
    pub collapsed: bool,
    pub expand_groups: bool,
    pub row_size: SidebarRowSize,
    pub align: PaneAlign,
    /// Re-parent [`crate::Layer::Below`] containers into the content pane and mirror GPUI's frame.
    pub adopt_below_layer: bool,
    pub reorderable: bool,
    /// Smallest the content pane may be squeezed to, which is what caps a drag on the divider.
    pub content_min_thickness: Pixels,
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
            content_min_thickness: px(0.),
            backdrop: Some(Material::Card),
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
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

impl Drop for SidebarState {
    fn drop(&mut self) {
        let center = NSNotificationCenter::defaultCenter();
        for observer in self.observers.drain(..) {
            unsafe { center.removeObserver(observer.as_ref()) };
        }
    }
}

/// Keeps AppKit's toolbar hairline hidden: the pocket that draws it is built with the window's
/// first frame, after every layout the pane has run.
fn observe_pocket_hairline(
    window: &NSWindow,
    pane: &NSView,
) -> Retained<ProtocolObject<dyn NSObjectProtocol>> {
    let pane = pane.retain();
    let block =
        StackBlock::new(move |_: NonNull<NSNotification>| hide_pocket_hairline(&pane)).copy();
    unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSWindowDidUpdateNotification),
            Some(window),
            None,
            &block,
        )
    }
}

fn observe_key_state(
    window: &NSWindow,
    shared: &Rc<Shared>,
) -> Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>> {
    if !label_fades_itself() {
        return Vec::new();
    }
    let center = NSNotificationCenter::defaultCenter();
    unsafe {
        [
            NSWindowDidBecomeKeyNotification,
            NSWindowDidResignKeyNotification,
        ]
    }
    .into_iter()
    .map(|name| {
        let shared = shared.clone();
        let block = StackBlock::new(move |_: NonNull<NSNotification>| {
            if let Some(outline) = shared.outline.borrow().clone() {
                tint_selected_rows(&outline);
            }
        })
        .copy();
        unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(name),
                Some(window),
                None,
                &block,
            )
        }
    })
    .collect()
}

#[derive(Clone)]
pub struct NativeSidebar(Rc<SidebarState>);

#[derive(Default)]
struct Sidebars(HashMap<AnyWindowHandle, NativeSidebar>);
impl Global for Sidebars {}

impl NativeSidebar {
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

    /// Only folders insert; a file dropped between rows is refused, as Finder refuses it.
    pub fn on_drop_paths(&self, handler: impl Fn(&SidebarDrop, &mut Window, &mut App) + 'static) {
        *self.0.shared.on_drop_paths.borrow_mut() = Some(Rc::new(handler));
    }

    /// Called with the row's id when the user clicks its eject affordance; unmounts nothing.
    pub fn on_eject(&self, handler: impl Fn(&SharedString, &mut Window, &mut App) + 'static) {
        *self.0.shared.on_eject.borrow_mut() = Some(Rc::new(handler));
    }

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
    unbacked: bool,
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
        // The split item's wrapper, not the pane: a divider drag moves the wrapper and resizes
        // the pane it holds a turn later, and insets read off that lag make the content's far
        // edge shimmer for a refresh at every step of the drag.
        let host = unsafe { pane.superview() }.unwrap_or_else(|| pane.retain());
        // The pane's own `frame.origin.x` reads 0 while it sits a sidebar's width in.
        let pane_in_cv = host.convertRect_toView(host.bounds(), Some(&content_view));
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
        let same = self.published.get() == metrics;
        if same && in_place {
            return;
        }
        self.published.set(metrics);
        // A divider drag runs AppKit's own event loop, in which gpui's display link never ticks,
        // and the pane has already moved: the frame the new metrics lay out is drawn here, inside
        // the move, so it reaches the screen in the transaction that carries the divider.
        let landed = self
            .cx
            .try_borrow_mut()
            .is_ok_and(|mut cx| publish(&mut cx, metrics));
        if landed {
            if !same {
                draw_now(&self.gpui);
            }
            return;
        }
        // gpui holds its app while it draws, and its own resize handler drops a size it cannot
        // take then: a window manager snapping the window as it opens lands inside the first
        // draw.
        let gpui = self.gpui.clone();
        self.cx
            .borrow()
            .spawn(async move |cx| {
                let _ = cx.update(|window, app| {
                    app.set_global(metrics);
                    window.bounds_changed(app);
                });
                draw_now(&gpui);
            })
            .detach();
    }

    fn sync_below(&self, pane: &NSView, gpui_frame: NSRect) {
        for view in pane.subviews().iter() {
            if view.isKindOfClass(Container::class()) && !rects_equal(view.frame(), gpui_frame) {
                view.setFrame(gpui_frame);
            }
        }
    }
}

fn publish(cx: &mut AsyncWindowContext, metrics: SidebarMetrics) -> bool {
    cx.update(|window, app| {
        app.set_global(metrics);
        window.refresh();
    })
    .is_ok()
}

fn draw_now(gpui: &NSView) {
    if let Some(layer) = gpui.layer() {
        layer.setNeedsDisplay();
        layer.displayIfNeeded();
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
            if !self.ivars().unbacked {
                return;
            }
            tokens::WINDOW_BACKGROUND.ns_color().setFill();
            NSBezierPath::fillRect(dirty);
        }
    }
);

/// AppKit lays a hairline across the top of a pane it cannot track as a scroll view, under the
/// toolbar background a non-transparent titlebar draws. Finder's window has none.
fn hide_pocket_hairline(pane: &NSView) {
    let Some(split) = (unsafe { pane.superview().and_then(|wrapper| wrapper.superview()) }) else {
        return;
    };
    fn hide_rules(view: &NSView) {
        for sub in view.subviews().iter() {
            if sub.frame().size.height <= 1.
                && sub
                    .class()
                    .name()
                    .to_string_lossy()
                    .contains("FillColorView")
            {
                sub.setHidden(true);
            } else {
                hide_rules(&sub);
            }
        }
    }
    for view in split.subviews().iter() {
        if view
            .class()
            .name()
            .to_string_lossy()
            .contains("NSTitlebarBackgroundView")
        {
            hide_rules(&view);
        }
    }
}

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
    if options.max_thickness > options.min_thickness {
        sidebar_item.setMaximumThickness(options.max_thickness.to_f64());
    }
    sidebar_item.setCanCollapse(true);
    sidebar_item.setAllowsFullHeightLayout(true);

    let pane_shared = Rc::new(PaneShared {
        gpui: gpui.clone(),
        align: options.align,
        adopt_below: options.adopt_below_layer,
        unbacked: options.backdrop.is_none(),
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
    if options.content_min_thickness > px(0.) {
        content_item.setMinimumThickness(options.content_min_thickness.to_f64());
    }

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
    // Under a transparent titlebar AppKit draws no toolbar background, and toolbar item glass with
    // nothing to sit on renders darker than the window behind it.
    ns_window.setTitlebarAppearsTransparent(false);

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

    let mut observers = observe_key_state(&ns_window, &shared);
    observers.push(observe_pocket_hairline(&ns_window, &pane));
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
        observers,
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
