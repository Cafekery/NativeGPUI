//! `NSPopUpButton` and real `NSMenu`s — pop-up, pull-down, and context menus.
//!
//! [`native_popup`] is a pooled control whose menu is diffed in place; [`native_context_menu`]
//! is a transient surface that pools nothing but a coordinate-space anchor.

use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::ffi::CStr;
use std::hash::{Hash, Hasher};
use std::panic::Location;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, DispatchPhase, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, Pixels, Style, Window,
    px, size,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSImage, NSMenu, NSMenuItem, NSPopUpButton};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};

use crate::bridge::appkit::{Container, fitting_size_keyed};
use crate::bridge::host::{Layer, NativeDispatch, NativeHost, ValueKind, element_key, wire};

/// One row of a native menu.
///
/// A [`MenuEntry::Separator`] occupies an index exactly as it does in `NSMenu`, so every index
/// this module reports indexes the entry list directly.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum MenuEntry {
    Item {
        title: String,
        /// An SF Symbol name. macOS draws the glyph, at the menu's own metrics.
        symbol: Option<String>,
        enabled: bool,
    },
    Separator,
}

impl MenuEntry {
    pub fn item(title: impl Into<String>) -> Self {
        Self::Item {
            title: title.into(),
            symbol: None,
            enabled: true,
        }
    }

    pub fn separator() -> Self {
        Self::Separator
    }

    /// An unknown name resolves to no image rather than a placeholder, which is `NSImage`'s own
    /// behaviour.
    pub fn symbol(mut self, name: impl Into<String>) -> Self {
        if let Self::Item { symbol, .. } = &mut self {
            *symbol = Some(name.into());
        }
        self
    }

    pub fn disabled(mut self) -> Self {
        if let Self::Item { enabled, .. } = &mut self {
            *enabled = false;
        }
        self
    }

    fn is_separator(&self) -> bool {
        matches!(self, Self::Separator)
    }
}

fn entries_from_titles(titles: impl IntoIterator<Item = impl Into<String>>) -> Vec<MenuEntry> {
    titles.into_iter().map(MenuEntry::item).collect()
}

fn symbol_image(name: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
}

/// Compares without allocating: this runs for every row of every hosted menu every frame.
fn nsstring_is(text: &NSString, want: &str) -> bool {
    let ptr = text.UTF8String();
    if ptr.is_null() {
        return want.is_empty();
    }
    // SAFETY: `UTF8String` hands back a NUL-terminated buffer owned by the current autorelease
    // pool; it is read here and not retained.
    unsafe { CStr::from_ptr(ptr) }.to_bytes() == want.as_bytes()
}

/// Bring `menu` in line with `entries`, touching only what differs.
///
/// A diff rather than a rebuild because `removeAllItems` closes an open menu, so a component
/// that rebuilt every frame would break exactly while the user was using it. The symbol name
/// that produced an item's image is parked in its `representedObject`, there being no way to ask
/// an `NSImage` which SF Symbol it came from.
fn sync_menu(
    mtm: MainThreadMarker,
    menu: &NSMenu,
    entries: &[MenuEntry],
    action: Option<(&AnyObject, Sel)>,
) {
    let want = entries.len() as isize;
    while menu.numberOfItems() > want {
        menu.removeItemAtIndex(menu.numberOfItems() - 1);
    }

    for (index, entry) in entries.iter().enumerate() {
        let index = index as isize;
        if index < menu.numberOfItems() {
            let existing = menu.itemAtIndex(index).expect("index < numberOfItems");
            if existing.isSeparatorItem() != entry.is_separator() {
                menu.removeItemAtIndex(index);
                menu.insertItem_atIndex(&new_item(mtm, entry), index);
            }
        } else {
            menu.addItem(&new_item(mtm, entry));
        }

        let item = menu.itemAtIndex(index).expect("row exists by now");
        if item.tag() != index {
            item.setTag(index);
        }

        let MenuEntry::Item {
            title,
            symbol,
            enabled,
        } = entry
        else {
            continue;
        };

        if !nsstring_is(&item.title(), title) {
            item.setTitle(&NSString::from_str(title));
        }
        if item.isEnabled() != *enabled {
            item.setEnabled(*enabled);
        }

        let current = item
            .representedObject()
            .and_then(|object| object.downcast::<NSString>().ok());
        let matches = match (&current, symbol) {
            (Some(have), Some(want)) => nsstring_is(have, want),
            (None, None) => true,
            _ => false,
        };
        if !matches {
            item.setImage(symbol.as_deref().and_then(symbol_image).as_deref());
            let name = symbol.as_deref().map(NSString::from_str);
            let name: Option<&AnyObject> = name.as_deref().map(|name| name.as_ref());
            unsafe { item.setRepresentedObject(name) };
        }

        if let Some((target, selector)) = action {
            unsafe {
                item.setTarget(Some(target));
                item.setAction(Some(selector));
            }
        }
    }
}

fn new_item(mtm: MainThreadMarker, entry: &MenuEntry) -> Retained<NSMenuItem> {
    match entry {
        MenuEntry::Separator => NSMenuItem::separatorItem(mtm),
        MenuEntry::Item { .. } => NSMenuItem::new(mtm),
    }
}

fn popup_fitting(entries: &[MenuEntry], pull_down: bool) -> NSSize {
    let mut hasher = DefaultHasher::new();
    entries.hash(&mut hasher);
    pull_down.hash(&mut hasher);

    fitting_size_keyed("NSPopUpButton", hasher.finish(), |mtm| {
        let probe = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::ZERO,
            pull_down,
        );
        probe.setAutoenablesItems(false);
        if let Some(menu) = probe.menu() {
            sync_menu(mtm, &menu, entries, None);
        }
        probe.fittingSize()
    })
}

/// `selected` is the index you own — wire [`NativePopup::on_change`] to keep it.
pub fn native_popup(id: impl Into<ElementId>, selected: usize) -> NativePopup {
    NativePopup {
        id: id.into(),
        selected,
        entries: Vec::new(),
        pull_down: false,
        enabled: true,
        on_change: None,
    }
}

pub struct NativePopup {
    id: ElementId,
    selected: usize,
    entries: Vec<MenuEntry>,
    pull_down: bool,
    enabled: bool,
    on_change: Option<NativeDispatch>,
}

impl NativePopup {
    /// Plain rows, all enabled.
    pub fn items(mut self, items: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.entries = entries_from_titles(items);
        self
    }

    /// Separators, disabled rows, SF Symbols.
    pub fn entries(mut self, entries: impl IntoIterator<Item = MenuEntry>) -> Self {
        self.entries = entries.into_iter().collect();
        self
    }

    /// The pull-down style. AppKit lifts entry 0 onto the button face and hides it from the
    /// list, so a pull-down's pickable rows start at index 1.
    pub fn pull_down(mut self, pull_down: bool) -> Self {
        self.pull_down = pull_down;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(move |value, window, cx| {
            let Ok(index) = value.downcast::<isize>() else {
                return;
            };
            if *index >= 0 {
                handler(&(*index as usize), window, cx);
            }
        }));
        self
    }
}

impl Element for NativePopup {
    type RequestLayoutState = ();
    type PrepaintState = ();

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
        let fitting = popup_fitting(&self.entries, self.pull_down);
        let mut style = Style::default();
        style.size = size(
            px(fitting.width as f32).into(),
            px(fitting.height as f32).into(),
        );
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let key = element_key(id);
        let mask = window.content_mask().bounds;
        let host = NativeHost::get(window, cx);
        let pull_down = self.pull_down;

        let popup = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
            let popup = NSPopUpButton::initWithFrame_pullsDown(
                NSPopUpButton::alloc(mtm),
                NSRect::ZERO,
                pull_down,
            );
            // Left on, `NSMenu`'s validation re-enables every item just before the menu opens
            // and `MenuEntry::disabled` silently does nothing.
            popup.setAutoenablesItems(false);
            wire(&popup, target, key);
            popup
        });

        // `setPullsDown:` reverses the item array, so the by-index diff has to run after it.
        if popup.pullsDown() != self.pull_down {
            popup.setPullsDown(self.pull_down);
        }
        if let Some(menu) = popup.menu() {
            sync_menu(popup.mtm(), &menu, &self.entries, None);
        }

        let wanted = self.selected as isize;
        let selectable = self
            .entries
            .get(self.selected)
            .is_some_and(|entry| !entry.is_separator());
        if selectable && popup.indexOfSelectedItem() != wanted {
            popup.selectItemAtIndex(wanted);
        }
        if popup.isEnabled() != self.enabled {
            popup.setEnabled(self.enabled);
        }

        // The selection is `indexOfSelectedItem`, not the control value: `integerValue` is the
        // cell's state and reports the same number for every row. Read inside the action,
        // because a paint between the action and the deferred handler would push the app's
        // stale `selected` back with `selectItemAtIndex`.
        host.on_event(
            key,
            Rc::new(|sender| {
                let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
                Box::new(index)
            }),
            self.on_change.take(),
        );
    }
}

impl IntoElement for NativePopup {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Where a context menu's top-left goes.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum MenuOrigin {
    /// At the pointer, as macOS does everywhere else.
    #[default]
    Pointer,
    /// At the element's own top-left corner, for a "more" affordance or a disclosure control.
    ElementTopLeft,
}

struct SinkIvars {
    picked: Rc<Cell<isize>>,
}

define_class!(
    /// Records which row a transient menu chose, and nothing else. `NSMenuItem::setTarget:` is
    /// weak, so this has to be held in a local for the life of the menu. It deliberately does
    /// not touch GPUI: the action fires inside the nested tracking loop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGMenuSink"]
    #[ivars = SinkIvars]
    struct MenuSink;

    impl MenuSink {
        #[unsafe(method(ngMenuPick:))]
        fn ng_menu_pick(&self, sender: *mut AnyObject) {
            let tag: isize = unsafe { msg_send![sender, tag] };
            self.ivars().picked.set(tag);
        }
    }
);

impl MenuSink {
    fn new(mtm: MainThreadMarker, picked: Rc<Cell<isize>>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SinkIvars { picked });
        unsafe { msg_send![super(this), init] }
    }
}

/// Build, show and tear down a transient menu, returning the chosen index. `location` is in
/// `anchor`'s own coordinate space. Blocks in AppKit's menu tracking loop until the menu
/// closes, so the caller must not be holding a GPUI borrow.
fn show_menu(anchor: &Container, location: NSPoint, entries: &[MenuEntry]) -> Option<usize> {
    let mtm = anchor.mtm();
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);

    let picked = Rc::new(Cell::new(-1));
    let sink = MenuSink::new(mtm, picked.clone());
    let target: &AnyObject = sink.as_ref();
    sync_menu(mtm, &menu, entries, Some((target, sel!(ngMenuPick:))));

    // `positioningItem: nil` puts the menu's own top-left at `location`.
    menu.popUpMenuPositioningItem_atLocation_inView(None, location, Some(anchor));

    drop(sink);
    let index = picked.get();
    (index >= 0).then_some(index as usize)
}

/// Attach a real `NSMenu` to a GPUI-drawn region, shown on right-click or control-click.
///
/// Nothing persistent is pooled but a `Container` anchor sized to the element, which exists
/// only to carry a coordinate space: the host writes its frame from GPUI's own logical, y-down,
/// window-relative numbers, so the location AppKit is handed is a plain subtraction with no
/// `convertPoint:` and no flip.
pub fn native_context_menu(id: impl Into<ElementId>, child: impl IntoElement) -> NativeContextMenu {
    NativeContextMenu {
        id: id.into(),
        child: child.into_any_element(),
        entries: Vec::new(),
        origin: MenuOrigin::default(),
        on_select: None,
    }
}

pub struct NativeContextMenu {
    id: ElementId,
    child: AnyElement,
    entries: Vec<MenuEntry>,
    origin: MenuOrigin,
    on_select: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
}

impl NativeContextMenu {
    pub fn items(mut self, items: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.entries = entries_from_titles(items);
        self
    }

    pub fn entries(mut self, entries: impl IntoIterator<Item = MenuEntry>) -> Self {
        self.entries = entries.into_iter().collect();
        self
    }

    pub fn origin(mut self, origin: MenuOrigin) -> Self {
        self.origin = origin;
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(move |index, window, cx| {
            handler(&index, window, cx)
        }));
        self
    }
}

impl Element for NativeContextMenu {
    type RequestLayoutState = ();
    type PrepaintState = ();

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
        // The child's own layout node, so the bounds this element paints with are the child's.
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
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let key = element_key(id);
        let mask = window.content_mask().bounds;
        let host = NativeHost::get(window, cx);
        // The zero rect is never the anchor's frame: `place` overwrites it with `bounds` on this
        // very call, which is what makes the anchor's local origin the element's top-left.
        let anchor = host.place(key, Layer::Above, bounds, mask, |mtm, _| {
            Container::new(mtm, NSRect::ZERO, false)
        });
        // Nothing dispatches to this key; clear any stale handler a reused pool slot carries.
        host.on_change(key, ValueKind::None, None);

        if let Some(handler) = self.on_select.take() {
            if !self.entries.is_empty() {
                let entries = std::mem::take(&mut self.entries);
                let origin = self.origin;
                let async_cx = window.to_async(cx);
                window.on_mouse_event(move |event: &MouseDownEvent, phase, _, _| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let contextual = event.button == MouseButton::Right
                        || (event.button == MouseButton::Left && event.modifiers.control);
                    if !contextual
                        || !bounds.contains(&event.position)
                        || !mask.contains(&event.position)
                    {
                        return;
                    }

                    let location = match origin {
                        MenuOrigin::Pointer => NSPoint::new(
                            (event.position.x - bounds.origin.x).to_f64(),
                            (event.position.y - bounds.origin.y).to_f64(),
                        ),
                        MenuOrigin::ElementTopLeft => NSPoint::new(0., 0.),
                    };

                    let anchor = anchor.clone();
                    let entries = entries.clone();
                    let handler = handler.clone();
                    // Deferred because `show_menu` blocks in a tracking loop that services both
                    // AppKit and GPUI's display link: entering it under GPUI's `App` borrow
                    // panics on the next frame.
                    async_cx
                        .spawn(async move |cx| {
                            let Some(index) = show_menu(&anchor, location, &entries) else {
                                return;
                            };
                            let _ = cx.update(|window, app| handler(index, window, app));
                        })
                        .detach();
                });
            }
        }

        self.child.paint(window, cx);
    }
}

impl IntoElement for NativeContextMenu {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}
