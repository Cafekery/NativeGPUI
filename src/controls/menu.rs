//! `NSPopUpButton` and real `NSMenu`s — pop-up, pull-down, and context menus.
//!
//! [`native_popup`] is a pooled control whose menu is diffed in place; [`native_context_menu`]
//! is a transient surface that pools nothing but a coordinate-space anchor.

use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::ffi::CStr;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::panic::Location;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, DispatchPhase, Element, ElementId, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Modifiers, MouseButton, MouseDownEvent, Pixels,
    Size, Style, Window, px, size,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBox, NSBoxType, NSControlStateValueOff, NSControlStateValueOn, NSEventModifierFlags, NSImage,
    NSMenu, NSMenuItem, NSPopUpButton, NSTitlePosition, NSView,
};
use objc2_foundation::{MainThreadMarker, NSNumber, NSObject, NSPoint, NSRect, NSSize, NSString};

use crate::bridge::appkit::{Container, fitting_size_keyed};
use crate::bridge::host::{Layer, NativeDispatch, NativeHost, ValueKind, element_key, wire};
use crate::system::color::to_nscolor;

/// One row of a native menu.
///
/// A [`MenuEntry::Separator`] occupies an index exactly as it does in `NSMenu`, so every index
/// this module reports indexes the entry list directly.
///
/// Two kinds of row are never reported at all, and both are places a caller can be silently
/// wrong: see [`MenuEntry::submenu`] and [`MenuEntry::view`].
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum MenuEntry {
    Item {
        title: String,
        /// An SF Symbol name. macOS draws the glyph, at the menu's own metrics.
        symbol: Option<String>,
        enabled: bool,
        /// Draws macOS's own checkmark in the leading gutter every row already reserves.
        checked: bool,
        /// The chord drawn at the trailing edge. A label only, outside the menu bar: a transient
        /// menu is only up while it is tracking, and nothing is listening for the chord.
        key: Option<(String, Modifiers)>,
        /// Non-empty makes this row a submenu host.
        submenu: Vec<MenuEntry>,
    },
    Separator,
    /// A row the caller draws.
    View(MenuView),
}

impl MenuEntry {
    pub fn item(title: impl Into<String>) -> Self {
        Self::Item {
            title: title.into(),
            symbol: None,
            enabled: true,
            checked: false,
            key: None,
            submenu: Vec::new(),
        }
    }

    pub fn separator() -> Self {
        Self::Separator
    }

    /// A row of your own `NSView`, the way Finder puts a strip of tag colours on one row.
    ///
    /// The row is never reported: this module wires no action to it, so whatever the view does
    /// with a click is all that happens. [`MenuView`] says what else it costs.
    pub fn view(view: MenuView) -> Self {
        Self::View(view)
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

    pub fn checked(mut self, on: bool) -> Self {
        if let Self::Item { checked, .. } = &mut self {
            *checked = on;
        }
        self
    }

    /// `equivalent` is the unmodified character, lowercase — AppKit derives the drawn glyph and
    /// adds ⇧ itself.
    pub fn key(mut self, equivalent: impl Into<String>, modifiers: Modifiers) -> Self {
        if let Self::Item { key, .. } = &mut self {
            *key = Some((equivalent.into(), modifiers));
        }
        self
    }

    /// Nests a real `NSMenu` under this row. AppKit draws the `›` chevron, opens the submenu on
    /// hover and sizes it, all for free.
    ///
    /// Indices stay flat, and a submenu's rows are therefore unreachable: they are given no
    /// action, so choosing one reports nothing rather than a number that would mean a different
    /// row of the outer list. Reporting one would need a path, and a caller holding the old
    /// single index would mis-route it in silence. The host row cannot report either —
    /// `setSubmenu:` installs AppKit's own `submenuAction:`, and this module leaves it there.
    pub fn submenu(mut self, entries: impl IntoIterator<Item = MenuEntry>) -> Self {
        if let Self::Item { submenu, .. } = &mut self {
            *submenu = entries.into_iter().collect();
        }
        self
    }

    fn is_separator(&self) -> bool {
        matches!(self, Self::Separator)
    }

    fn kind(&self) -> Kind {
        match self {
            Self::Item { .. } => Kind::Item,
            Self::Separator => Kind::Separator,
            Self::View(_) => Kind::View,
        }
    }
}

/// What an existing `NSMenuItem` is, so the in-place diff can tell when a row has to be replaced
/// rather than updated. A plain row and a view row are not interchangeable: an item with a view
/// ignores its own title, and one that keeps a stale view draws nothing else.
#[derive(PartialEq)]
enum Kind {
    Item,
    Separator,
    View,
}

fn item_kind(item: &NSMenuItem) -> Kind {
    if item.isSeparatorItem() {
        Kind::Separator
    } else if item.view().is_some() {
        Kind::View
    } else {
        Kind::Item
    }
}

/// A menu row built by the caller and handed to `NSMenuItem`'s own `view`.
///
/// What the caller gives up, and why a menu is the one place that is the right trade: this is a
/// plain retained `NSView`, not one of this crate's pooled elements. It has no GPUI bounds, takes
/// no part in GPUI layout, is not clipped by a content mask and is not reused across menus. None
/// of that is available to want — a menu is built outside any paint pass, so there is no element
/// tree to pool against and no bounds to inherit. What the row gets in exchange is everything
/// around it: macOS's own menu panel, its glass, its highlight, its metrics and its animation.
///
/// `build` runs only when the row is new. `key` is the row's identity for the in-place diff, so
/// re-rendering while the menu is open does not rebuild the view under the user.
#[derive(Clone)]
pub struct MenuView {
    key: u64,
    size: NSSize,
    build: Rc<dyn Fn(MainThreadMarker) -> Retained<NSView>>,
}

impl MenuView {
    /// `size` is applied to the view after `build` returns, and is not optional: an `NSMenuItem`
    /// whose view has a zero frame contributes zero height, so the row is present, pickable and
    /// completely invisible. A view wider than the widest text row widens the whole menu, and the
    /// menu's sizing pass never writes back to the view's frame, so this is the only thing that
    /// decides how much room the row is given.
    pub fn new(
        key: impl Hash,
        size: Size<Pixels>,
        build: impl Fn(MainThreadMarker) -> Retained<NSView> + 'static,
    ) -> Self {
        let size = NSSize::new(size.width.to_f64(), size.height.to_f64());
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        // Folded in, or a row that changed only its height would keep the old view's frame.
        size.width.to_bits().hash(&mut hasher);
        size.height.to_bits().hash(&mut hasher);
        Self {
            key: hasher.finish(),
            size,
            build: Rc::new(build),
        }
    }

    /// A strip of round colour swatches at Finder's tag metrics.
    ///
    /// Decorative: the swatches do not track the pointer and the row reports nothing.
    pub fn swatches(colors: impl IntoIterator<Item = Hsla>) -> Self {
        let colors: Vec<Hsla> = colors.into_iter().collect();
        let mut hasher = DefaultHasher::new();
        colors.hash(&mut hasher);
        let width = swatch_row_width(colors.len());
        Self::new(
            hasher.finish(),
            size(px(width as f32), px(SWATCH_ROW_HEIGHT as f32)),
            move |mtm| swatch_row(mtm, &colors),
        )
    }
}

impl PartialEq for MenuView {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for MenuView {}

impl Hash for MenuView {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

impl fmt::Debug for MenuView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MenuView")
            .field("key", &self.key)
            .field("size", &(self.size.width, self.size.height))
            .finish_non_exhaustive()
    }
}

const SWATCH_DIAMETER: f64 = 13.;
const SWATCH_GAP: f64 = 9.;
/// Lines the first swatch up with the leading edge of the symbol column beside it.
const SWATCH_INSET: f64 = 14.;
/// A plain row is 24pt tall in a menu at the default font; a swatch strip reads better with two
/// more, the way Finder's does.
const SWATCH_ROW_HEIGHT: f64 = 26.;

fn swatch_row_width(count: usize) -> f64 {
    let dots = match count {
        0 => 0.,
        n => n as f64 * SWATCH_DIAMETER + (n - 1) as f64 * SWATCH_GAP,
    };
    SWATCH_INSET * 2. + dots
}

/// An `NSBox` per dot rather than a layer per dot: `fillColor` takes an `NSColor`, so the swatch
/// is colour-managed the same way every other hosted control is, with no `CGColor` to own.
fn swatch_row(mtm: MainThreadMarker, colors: &[Hsla]) -> Retained<NSView> {
    let width = swatch_row_width(colors.len());
    let row = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::ZERO, NSSize::new(width, SWATCH_ROW_HEIGHT)),
    );
    let y = ((SWATCH_ROW_HEIGHT - SWATCH_DIAMETER) / 2.).round();
    for (index, color) in colors.iter().enumerate() {
        let x = SWATCH_INSET + index as f64 * (SWATCH_DIAMETER + SWATCH_GAP);
        let dot = NSBox::initWithFrame(
            NSBox::alloc(mtm),
            NSRect::new(
                NSPoint::new(x, y),
                NSSize::new(SWATCH_DIAMETER, SWATCH_DIAMETER),
            ),
        );
        dot.setBoxType(NSBoxType::Custom);
        dot.setTitlePosition(NSTitlePosition::NoTitle);
        dot.setBorderWidth(0.);
        dot.setCornerRadius(SWATCH_DIAMETER / 2.);
        dot.setFillColor(&to_nscolor(*color));
        row.addSubview(&dot);
    }
    row
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

/// Brings `menu` in line with `entries`, touching only what differs: `removeAllItems` closes an
/// open menu, so a rebuild would break the menu exactly while the user was in it. An item's
/// symbol name is parked in its `representedObject`, there being no way to ask an `NSImage`
/// which SF Symbol it came from.
///
/// Every item's tag is its index into `entries`, separators counted, and `action` is installed on
/// exactly the rows that can report one: not on a submenu host, not on a submenu's own rows, and
/// not on a view row.
pub(crate) fn sync_menu(
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
            if item_kind(&existing) != entry.kind() {
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
            checked,
            key,
            submenu,
        } = entry
        else {
            if let MenuEntry::View(view) = entry {
                sync_view(mtm, &item, view);
            }
            continue;
        };

        if !nsstring_is(&item.title(), title) {
            item.setTitle(&NSString::from_str(title));
        }
        if item.isEnabled() != *enabled {
            item.setEnabled(*enabled);
        }

        let want_state = if *checked {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        };
        if item.state() != want_state {
            item.setState(want_state);
        }

        // The mask is only written alongside a character: `NSMenuItem`'s own default is ⌘, so
        // clearing it for a row that draws no chord would be a write that changes nothing
        // visible.
        if let Some((equivalent, modifiers)) = key {
            if !nsstring_is(&item.keyEquivalent(), equivalent) {
                item.setKeyEquivalent(&NSString::from_str(equivalent));
            }
            let mask = event_mask(*modifiers);
            if item.keyEquivalentModifierMask() != mask {
                item.setKeyEquivalentModifierMask(mask);
            }
        } else if !nsstring_is(&item.keyEquivalent(), "") {
            item.setKeyEquivalent(&NSString::from_str(""));
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

        if submenu.is_empty() {
            if item.hasSubmenu() {
                item.setSubmenu(None);
            }
            if let Some((target, selector)) = action {
                unsafe {
                    item.setTarget(Some(target));
                    item.setAction(Some(selector));
                }
            }
        } else {
            sync_submenu(mtm, &item, submenu);
        }
    }
}

/// A nested `NSMenu` does not inherit `autoenablesItems` from the menu it hangs off, and
/// automatic validation disables every row whose action is nil — so a submenu built here comes up
/// entirely greyed out unless the flag is set on the submenu itself.
///
/// The host row is handed to AppKit rather than kept: `setSubmenu:` installs `submenuAction:`
/// *only over a nil action*, so the caller's action has to be cleared first. Left in place it
/// keeps firing, so the row reports its own index and closes the menu
/// instead of opening the submenu — which is what a row that gained a submenu after a frame as
/// an ordinary row would do. Both writes happen only on the frame the submenu is attached:
/// attaching one every frame would rebuild the nested menu under a user already in it.
fn sync_submenu(mtm: MainThreadMarker, item: &NSMenuItem, entries: &[MenuEntry]) {
    let sub = match item.submenu() {
        Some(sub) => sub,
        None => {
            unsafe {
                item.setTarget(None);
                item.setAction(None);
            }
            let sub = NSMenu::new(mtm);
            item.setSubmenu(Some(&sub));
            sub
        }
    };
    if sub.autoenablesItems() {
        sub.setAutoenablesItems(false);
    }
    sync_menu(mtm, &sub, entries, None);
}

/// The row's identity is its [`MenuView`] key, parked in `representedObject` the way a symbol name
/// is: there is no asking an `NSView` which builder made it. An `NSNumber` rather than a string so
/// the compare allocates nothing on the frames where the row has not changed.
fn sync_view(mtm: MainThreadMarker, item: &NSMenuItem, view: &MenuView) {
    let current = item
        .representedObject()
        .and_then(|object| object.downcast::<NSNumber>().ok())
        .map(|number| number.unsignedLongLongValue());
    if current == Some(view.key) {
        return;
    }

    let built = (view.build)(mtm);
    built.setFrameSize(view.size);
    item.setView(Some(&built));
    let key = NSNumber::new_u64(view.key);
    unsafe { item.setRepresentedObject(Some(key.as_ref())) };
}

fn event_mask(modifiers: Modifiers) -> NSEventModifierFlags {
    let mut mask = NSEventModifierFlags::empty();
    if modifiers.platform {
        mask |= NSEventModifierFlags::Command;
    }
    if modifiers.shift {
        mask |= NSEventModifierFlags::Shift;
    }
    if modifiers.alt {
        mask |= NSEventModifierFlags::Option;
    }
    if modifiers.control {
        mask |= NSEventModifierFlags::Control;
    }
    if modifiers.function {
        mask |= NSEventModifierFlags::Function;
    }
    mask
}

fn new_item(mtm: MainThreadMarker, entry: &MenuEntry) -> Retained<NSMenuItem> {
    match entry {
        MenuEntry::Separator => NSMenuItem::separatorItem(mtm),
        MenuEntry::Item { .. } | MenuEntry::View(_) => NSMenuItem::new(mtm),
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
        // A pop-up's tick on the selected row *is* that item's `state`, the same field
        // [`sync_menu`] writes from [`MenuEntry::checked`], so the diff clears it every frame —
        // and `selectItemAtIndex:` will not put it back, because re-selecting the index the cell
        // already holds is a no-op. The tick survives anyway: the cell re-asserts it when it
        // displays, which is measurable only by removing these lines and finding the tick still
        // there. They stay because that recovery is undocumented and its failure is silent, and
        // this makes the pop-up state its own invariant instead of inheriting it. A pull-down is
        // the other way round — AppKit gives one no state at all, so its rows are right precisely
        // because the diff left them Off.
        if selectable && !self.pull_down {
            let selected = popup.menu().and_then(|menu| menu.itemAtIndex(wanted));
            if let Some(item) = selected {
                if item.state() != NSControlStateValueOn {
                    item.setState(NSControlStateValueOn);
                }
            }
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

    /// The chosen row's index into the slice given to [`Self::entries`], separators counted.
    ///
    /// Three rows never arrive here, and nothing else does either. A submenu's own rows carry no
    /// action, so a pick inside one reports nothing at all rather than a number that would name a
    /// different row of the outer list. A submenu host is not clickable — AppKit's
    /// `submenuAction:` owns that row and opens the submenu instead. A [`MenuEntry::View`] row has
    /// no action either; whatever the caller's view does with the click is all that happens.
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::hsla;

    fn entry_hash(entry: &MenuEntry) -> u64 {
        let mut hasher = DefaultHasher::new();
        entry.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn swatch_row_is_wide_enough_for_every_dot_and_both_insets() {
        assert_eq!(swatch_row_width(0), SWATCH_INSET * 2.);
        assert_eq!(swatch_row_width(1), SWATCH_INSET * 2. + SWATCH_DIAMETER);
        assert_eq!(
            swatch_row_width(7),
            SWATCH_INSET * 2. + 7. * SWATCH_DIAMETER + 6. * SWATCH_GAP
        );
        // The last dot's trailing edge has to land inside the row, or the strip is clipped.
        let last = SWATCH_INSET + 6. * (SWATCH_DIAMETER + SWATCH_GAP) + SWATCH_DIAMETER;
        assert!(last <= swatch_row_width(7));
    }

    #[test]
    fn every_modifier_reaches_the_mask() {
        assert_eq!(
            event_mask(Modifiers::default()),
            NSEventModifierFlags::empty()
        );
        for (modifiers, flag) in [
            (Modifiers::command(), NSEventModifierFlags::Command),
            (Modifiers::shift(), NSEventModifierFlags::Shift),
            (Modifiers::alt(), NSEventModifierFlags::Option),
            (Modifiers::control(), NSEventModifierFlags::Control),
            (
                Modifiers {
                    function: true,
                    ..Modifiers::default()
                },
                NSEventModifierFlags::Function,
            ),
        ] {
            assert_eq!(event_mask(modifiers), flag);
        }
        assert_eq!(
            event_mask(Modifiers::command_shift()),
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift
        );
    }

    #[test]
    fn a_view_row_is_its_own_kind_so_the_diff_replaces_the_item() {
        let view = MenuView::swatches([hsla(0., 1., 0.5, 1.)]);
        assert!(MenuEntry::item("Sort By").kind() == Kind::Item);
        assert!(MenuEntry::separator().kind() == Kind::Separator);
        assert!(MenuEntry::view(view).kind() == Kind::View);
    }

    #[test]
    fn a_submenu_does_not_change_a_rows_kind() {
        // The diff updates such a row in place; only the nested menu below it is rebuilt.
        let host = MenuEntry::item("Sort By").submenu([MenuEntry::item("Name")]);
        assert!(host.kind() == Kind::Item);
        assert!(!host.is_separator());
    }

    #[test]
    fn submenu_contents_reach_the_entry_identity() {
        let one = MenuEntry::item("Sort By").submenu([MenuEntry::item("Name")]);
        let other = MenuEntry::item("Sort By").submenu([MenuEntry::item("Kind")]);
        assert_ne!(one, other);
        assert_ne!(entry_hash(&one), entry_hash(&other));
        // A pooled popup keys its cached fitting size on this hash, so a submenu that changed
        // without changing the hash would be measured at the old size forever.
        assert_ne!(one, MenuEntry::item("Sort By"));
    }

    #[test]
    fn checked_and_key_reach_the_entry_identity() {
        let plain = MenuEntry::item("Use Groups");
        assert_ne!(plain, plain.clone().checked(true));
        assert_ne!(entry_hash(&plain), entry_hash(&plain.clone().checked(true)));
        let chord = plain.clone().key("j", Modifiers::command());
        assert_ne!(plain, chord);
        assert_ne!(chord, plain.clone().key("j", Modifiers::command_shift()));
    }

    #[test]
    fn a_view_rows_identity_covers_its_size_as_well_as_its_key() {
        let build = |_: MainThreadMarker| unreachable!("the probe never builds in a unit test");
        let small = MenuView::new("tags", size(px(190.), px(26.)), build);
        let tall = MenuView::new("tags", size(px(190.), px(40.)), build);
        let wide = MenuView::new("tags", size(px(220.), px(26.)), build);
        let same = MenuView::new("tags", size(px(190.), px(26.)), build);
        assert_eq!(small, same);
        assert_ne!(small, tall);
        assert_ne!(small, wide);
        assert_ne!(
            small,
            MenuView::new("other", size(px(190.), px(26.)), build)
        );
    }

    #[test]
    fn swatch_rows_differ_by_colour_and_by_count() {
        let red = hsla(0., 1., 0.5, 1.);
        let blue = hsla(0.6, 1., 0.5, 1.);
        assert_eq!(MenuView::swatches([red]), MenuView::swatches([red]));
        assert_ne!(MenuView::swatches([red]), MenuView::swatches([blue]));
        assert_ne!(MenuView::swatches([red]), MenuView::swatches([red, blue]));
    }

    #[test]
    fn builders_leave_the_other_variants_alone() {
        // Every builder is a no-op on a separator, so a mis-chained call cannot turn one into an
        // item that occupies the same index.
        for entry in [
            MenuEntry::separator().symbol("tag"),
            MenuEntry::separator().checked(true),
            MenuEntry::separator().key("j", Modifiers::command()),
            MenuEntry::separator().submenu([MenuEntry::item("Name")]),
            MenuEntry::separator().disabled(),
        ] {
            assert_eq!(entry, MenuEntry::separator());
        }
    }
}
