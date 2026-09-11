use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::ffi::CStr;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::panic::Location;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, DispatchPhase, Element, ElementId, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Length, Modifiers, MouseButton, MouseDownEvent,
    Pixels, Point, Size, Style, Window, px, size,
};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, Bool, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityElement, NSAccessibilityRadioButtonRole, NSBezierPath, NSBox,
    NSBoxType, NSColor, NSControlSize, NSControlStateValueOff, NSControlStateValueOn, NSEvent,
    NSEventModifierFlags, NSImage, NSMenu, NSMenuItem, NSPopUpButton, NSTitlePosition, NSView,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSNumber, NSObject, NSPoint, NSRect, NSSize, NSString,
};

use crate::bridge::appkit::{Container, fitting_size_keyed};
use crate::bridge::host::ControlSize;
use crate::bridge::host::{
    Layer, NativeDispatch, NativeHost, Slot, ValueKind, element_key, hosted_element, wire,
};
use crate::controls::button::ButtonBezel;
use crate::controls::fitted;
use crate::system::color::to_nscolor;

/// One row of a native menu. A separator occupies an index as it does in `NSMenu`; view rows are
/// never reported.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum MenuEntry {
    Item {
        title: String,
        /// An SF Symbol name.
        symbol: Option<String>,
        /// A filled disc in this sRGB colour, drawn where the symbol would go.
        dot: Option<(u8, u8, u8)>,
        enabled: bool,
        checked: bool,
        /// Drawn at the trailing edge; a label only, since a transient menu is not listening.
        key: Option<(String, Modifiers)>,
        /// Stands in for the row above it while ⌥ is held.
        alternate: bool,
        submenu: Vec<MenuEntry>,
    },
    Separator,
    View(MenuView),
}

impl MenuEntry {
    pub fn item(title: impl Into<String>) -> Self {
        Self::Item {
            title: title.into(),
            symbol: None,
            dot: None,
            enabled: true,
            checked: false,
            key: None,
            alternate: false,
            submenu: Vec::new(),
        }
    }

    pub fn separator() -> Self {
        Self::Separator
    }

    pub fn view(view: MenuView) -> Self {
        Self::View(view)
    }

    pub fn symbol(mut self, name: impl Into<String>) -> Self {
        if let Self::Item { symbol, .. } = &mut self {
            *symbol = Some(name.into());
        }
        self
    }

    pub fn dot(mut self, color: (u8, u8, u8)) -> Self {
        if let Self::Item { dot, .. } = &mut self {
            *dot = Some(color);
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

    /// `equivalent` is the unmodified lowercase character.
    pub fn key(mut self, equivalent: impl Into<String>, modifiers: Modifiers) -> Self {
        if let Self::Item { key, .. } = &mut self {
            *key = Some((equivalent.into(), modifiers));
        }
        self
    }

    /// Shows in place of the row above it while ⌥ is held, as Finder's Always Open With does. Only
    /// a row without a key equivalent can stand in this way.
    pub fn option_alternate(mut self) -> Self {
        if let Self::Item { alternate, .. } = &mut self {
            *alternate = true;
        }
        self
    }

    /// Nests a real `NSMenu` under this row. Its rows report like any other, by their depth-first
    /// position in the whole tree.
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

/// A menu row the caller builds: a plain retained `NSView`, outside GPUI layout and the pool.
/// `build` runs only when the row is new; `key` is its identity for the in-place diff.
#[derive(Clone)]
pub struct MenuView {
    key: u64,
    size: NSSize,
    build: Rc<dyn Fn(MainThreadMarker) -> Retained<NSView>>,
}

struct SwatchIvars {
    count: usize,
    pick: Rc<dyn Fn(usize)>,
}

define_class!(
    /// Finder's tag row: the dots pick, then the menu closes as if the row itself was chosen.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSwatchRow"]
    #[ivars = SwatchIvars]
    struct SwatchRow;

    impl SwatchRow {
        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let step = SWATCH_DIAMETER + SWATCH_GAP;
            self.choose(((point.x - SWATCH_INSET).max(0.) / step) as usize);
        }
    }
);

impl SwatchRow {
    fn choose(&self, index: usize) {
        if index >= self.ivars().count {
            return;
        }
        (self.ivars().pick)(index);
        let Some(item) = self.enclosingMenuItem() else {
            return;
        };
        if let (Some(target), Some(action)) = (item.target(), item.action()) {
            let _: () = unsafe { msg_send![&*target, performSelector: action, withObject: &*item] };
        }
        if let Some(menu) = unsafe { item.menu() } {
            menu.cancelTracking();
        }
    }
}

struct DotIvars {
    row: Weak<SwatchRow>,
    index: usize,
}

define_class!(
    /// One dot of the tag row as VoiceOver sees it: a radio button named for what picking it does,
    /// as Finder's are, which a press picks as a click would.
    #[unsafe(super(NSAccessibilityElement, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSwatchDot"]
    #[ivars = DotIvars]
    struct SwatchDot;

    impl SwatchDot {
        #[unsafe(method(accessibilityPerformPress))]
        fn perform_press(&self) -> Bool {
            if let Some(row) = self.ivars().row.load() {
                row.choose(self.ivars().index);
            }
            Bool::YES
        }
    }
);

impl MenuView {
    /// `size` is applied after `build`; a zero-frame view row is present, pickable and invisible.
    pub fn new(
        key: impl Hash,
        size: Size<Pixels>,
        build: impl Fn(MainThreadMarker) -> Retained<NSView> + 'static,
    ) -> Self {
        let size = NSSize::new(size.width.to_f64(), size.height.to_f64());
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        size.width.to_bits().hash(&mut hasher);
        size.height.to_bits().hash(&mut hasher);
        Self {
            key: hasher.finish(),
            size,
            build: Rc::new(build),
        }
    }

    /// A strip of round colour swatches at Finder's tag metrics, reporting the dot that was hit.
    /// Each label is what VoiceOver names its dot, such as Finder's "Add “Red” label".
    pub fn swatches_picking(
        swatches: impl IntoIterator<Item = (Hsla, String)>,
        pick: impl Fn(usize) + 'static,
    ) -> Self {
        let (colors, labels): (Vec<Hsla>, Vec<String>) = swatches.into_iter().unzip();
        let mut hasher = DefaultHasher::new();
        colors.hash(&mut hasher);
        labels.hash(&mut hasher);
        let width = swatch_row_width(colors.len());
        let pick: Rc<dyn Fn(usize)> = Rc::new(pick);
        Self::new(
            hasher.finish(),
            size(px(width as f32), px(SWATCH_ROW_HEIGHT as f32)),
            move |mtm| swatch_row_picking(mtm, &colors, &labels, pick.clone()),
        )
    }

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
const SWATCH_INSET: f64 = 14.;
const SWATCH_ROW_HEIGHT: f64 = 26.;

fn swatch_row_width(count: usize) -> f64 {
    let dots = match count {
        0 => 0.,
        n => n as f64 * SWATCH_DIAMETER + (n - 1) as f64 * SWATCH_GAP,
    };
    SWATCH_INSET * 2. + dots
}

fn swatch_row_picking(
    mtm: MainThreadMarker,
    colors: &[Hsla],
    labels: &[String],
    pick: Rc<dyn Fn(usize)>,
) -> Retained<NSView> {
    let row = swatch_row(mtm, colors);
    let host = SwatchRow::alloc(mtm).set_ivars(SwatchIvars {
        count: colors.len(),
        pick,
    });
    let host: Retained<SwatchRow> = unsafe { msg_send![super(host), initWithFrame: row.frame()] };
    for view in row.subviews().iter() {
        host.addSubview(&view);
    }
    // The dots stand in for the row's own children, so VoiceOver reads radio buttons, not boxes.
    let parent: &AnyObject = host.as_ref();
    let dots: Vec<Retained<AnyObject>> = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let dot = SwatchDot::alloc(mtm).set_ivars(DotIvars {
                row: Weak::new(&*host),
                index,
            });
            let dot: Retained<SwatchDot> = unsafe { msg_send![super(dot), init] };
            unsafe {
                dot.setAccessibilityRole(Some(NSAccessibilityRadioButtonRole));
                dot.setAccessibilityLabel(Some(&NSString::from_str(label)));
                dot.setAccessibilityParent(Some(parent));
                dot.setAccessibilityFrameInParentSpace(dot_frame(index));
                Retained::cast_unchecked(dot)
            }
        })
        .collect();
    unsafe { host.setAccessibilityChildren(Some(&NSArray::from_retained_slice(&dots))) };
    Retained::into_super(host)
}

fn dot_frame(index: usize) -> NSRect {
    let x = SWATCH_INSET + index as f64 * (SWATCH_DIAMETER + SWATCH_GAP);
    let y = ((SWATCH_ROW_HEIGHT - SWATCH_DIAMETER) / 2.).round();
    NSRect::new(
        NSPoint::new(x, y),
        NSSize::new(SWATCH_DIAMETER, SWATCH_DIAMETER),
    )
}

fn swatch_row(mtm: MainThreadMarker, colors: &[Hsla]) -> Retained<NSView> {
    let width = swatch_row_width(colors.len());
    let row = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::ZERO, NSSize::new(width, SWATCH_ROW_HEIGHT)),
    );
    for (index, color) in colors.iter().enumerate() {
        let dot = NSBox::initWithFrame(NSBox::alloc(mtm), dot_frame(index));
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

fn image_key(symbol: Option<&str>, dot: Option<(u8, u8, u8)>) -> Option<String> {
    match (symbol, dot) {
        (Some(name), _) => Some(name.to_owned()),
        (None, Some((r, g, b))) => Some(format!("dot {r} {g} {b}")),
        (None, None) => None,
    }
}

fn menu_image(symbol: Option<&str>, dot: Option<(u8, u8, u8)>) -> Option<Retained<NSImage>> {
    match (symbol, dot) {
        (Some(name), _) => symbol_image(name),
        (None, Some(color)) => Some(dot_image(color)),
        (None, None) => None,
    }
}

/// A tag's disc, drawn rather than an SF Symbol, which a menu row would baseline-align.
fn dot_image(color: (u8, u8, u8)) -> Retained<NSImage> {
    thread_local! {
        static DOTS: RefCell<HashMap<(u8, u8, u8), Retained<NSImage>>> =
            RefCell::new(HashMap::new());
    }
    if let Some(hit) = DOTS.with_borrow(|cache| cache.get(&color).cloned()) {
        return hit;
    }
    let (r, g, b) = color;
    let draw = block2::RcBlock::new(move |rect: NSRect| -> Bool {
        NSColor::colorWithSRGBRed_green_blue_alpha(
            r as f64 / 255.,
            g as f64 / 255.,
            b as f64 / 255.,
            1.,
        )
        .setFill();
        NSBezierPath::bezierPathWithOvalInRect(rect).fill();
        Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(
        NSSize::new(SWATCH_DIAMETER, SWATCH_DIAMETER),
        false,
        &draw,
    );
    DOTS.with_borrow_mut(|cache| cache.insert(color, image.clone()));
    image
}

fn nsstring_is(text: &NSString, want: &str) -> bool {
    let ptr = text.UTF8String();
    if ptr.is_null() {
        return want.is_empty();
    }
    unsafe { CStr::from_ptr(ptr) }.to_bytes() == want.as_bytes()
}

/// Brings `menu` in line with `entries` in place: `removeAllItems` would close an open menu.
pub(crate) fn sync_menu(
    mtm: MainThreadMarker,
    menu: &NSMenu,
    entries: &[MenuEntry],
    action: Option<(&AnyObject, Sel)>,
) {
    sync_level(mtm, menu, entries, action, &mut 0);
}

fn sync_level(
    mtm: MainThreadMarker,
    menu: &NSMenu,
    entries: &[MenuEntry],
    action: Option<(&AnyObject, Sel)>,
    next: &mut isize,
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
        let tag = *next;
        *next += 1;
        if item.tag() != tag {
            item.setTag(tag);
        }

        let MenuEntry::Item {
            title,
            symbol,
            dot,
            enabled,
            checked,
            key,
            alternate,
            submenu,
        } = entry
        else {
            if let MenuEntry::View(view) = entry {
                sync_view(mtm, &item, view);
                // A row view fires its item's action itself, so it needs the target a title gets.
                if let Some((target, selector)) = action {
                    unsafe {
                        item.setTarget(Some(target));
                        item.setAction(Some(selector));
                    }
                }
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

        // An ⌥ row pairs with the row above it by their shared empty key; the mask tells them apart.
        let (equivalent, mask) = match key {
            Some((equivalent, modifiers)) => (equivalent.as_str(), event_mask(*modifiers)),
            None if *alternate => ("", NSEventModifierFlags::Option),
            None => ("", NSEventModifierFlags::Command),
        };
        if !nsstring_is(&item.keyEquivalent(), equivalent) {
            item.setKeyEquivalent(&NSString::from_str(equivalent));
        }
        if item.keyEquivalentModifierMask() != mask {
            item.setKeyEquivalentModifierMask(mask);
        }
        if item.isAlternate() != *alternate {
            item.setAlternate(*alternate);
        }

        // What the image is, is parked in `representedObject`; an NSImage cannot say which it is.
        let current = item
            .representedObject()
            .and_then(|object| object.downcast::<NSString>().ok());
        let want = image_key(symbol.as_deref(), *dot);
        let matches = match (&current, &want) {
            (Some(have), Some(want)) => nsstring_is(have, want),
            (None, None) => true,
            _ => false,
        };
        if !matches {
            item.setImage(menu_image(symbol.as_deref(), *dot).as_deref());
            let key = want.map(|key| NSString::from_str(&key));
            let key: Option<&AnyObject> = key.as_deref().map(|key| key.as_ref());
            unsafe { item.setRepresentedObject(key) };
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
            sync_submenu(mtm, &item, submenu, action, next);
        }
    }
}

fn sync_submenu(
    mtm: MainThreadMarker,
    item: &NSMenuItem,
    entries: &[MenuEntry],
    action: Option<(&AnyObject, Sel)>,
    next: &mut isize,
) {
    let sub = match item.submenu() {
        Some(sub) => sub,
        None => {
            // `setSubmenu:` installs `submenuAction:` only over a nil action.
            unsafe {
                item.setTarget(None);
                item.setAction(None);
            }
            let sub = NSMenu::new(mtm);
            item.setSubmenu(Some(&sub));
            sub
        }
    };
    // A nested menu does not inherit this, and automatic validation greys every nil-action row.
    if sub.autoenablesItems() {
        sub.setAutoenablesItems(false);
    }
    sync_level(mtm, &sub, entries, action, next);
}

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

/// `selected` is the index you own; wire `on_change` to keep it.
pub fn native_popup(id: impl Into<ElementId>, selected: usize) -> NativePopup {
    NativePopup {
        id: id.into(),
        selected,
        entries: Vec::new(),
        pull_down: false,
        bezel: ButtonBezel::Automatic,
        control_size: ControlSize::default(),
        width: None,
        enabled: true,
        on_change: None,
    }
}

pub struct NativePopup {
    id: ElementId,
    selected: usize,
    entries: Vec<MenuEntry>,
    pull_down: bool,
    bezel: ButtonBezel,
    control_size: ControlSize,
    width: Option<Length>,
    enabled: bool,
    on_change: Option<NativeDispatch>,
}

impl NativePopup {
    pub fn items(mut self, items: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.entries = entries_from_titles(items);
        self
    }

    pub fn entries(mut self, entries: impl IntoIterator<Item = MenuEntry>) -> Self {
        self.entries = entries.into_iter().collect();
        self
    }

    /// AppKit lifts entry 0 onto a pull-down's face, so its pickable rows start at 1.
    pub fn pull_down(mut self, pull_down: bool) -> Self {
        self.pull_down = pull_down;
        self
    }

    pub fn bezel_style(mut self, bezel: ButtonBezel) -> Self {
        self.bezel = bezel;
        self
    }

    pub fn control_size(mut self, control_size: ControlSize) -> Self {
        self.control_size = control_size;
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = Some(width.into());
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

    fn layout(&self) -> Style {
        let mut hasher = DefaultHasher::new();
        self.entries.hash(&mut hasher);
        self.pull_down.hash(&mut hasher);
        self.bezel.hash(&mut hasher);
        self.control_size.hash(&mut hasher);
        let (entries, pull_down, bezel, control_size) =
            (&self.entries, self.pull_down, self.bezel, self.control_size);
        let fitted = fitted(fitting_size_keyed(
            "NSPopUpButton",
            hasher.finish(),
            |mtm| {
                let probe = NSPopUpButton::initWithFrame_pullsDown(
                    NSPopUpButton::alloc(mtm),
                    NSRect::ZERO,
                    pull_down,
                );
                probe.setAutoenablesItems(false);
                probe.setBezelStyle(bezel.to_native());
                probe.setControlSize(NSControlSize::from(control_size));
                if let Some(menu) = probe.menu() {
                    sync_menu(mtm, &menu, entries, None);
                }
                probe.fittingSize()
            },
        ));
        match self.width {
            Some(width) => Style {
                size: size(width, fitted.size.height),
                ..fitted
            },
            None => fitted,
        }
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let pull_down = self.pull_down;
        let popup = slot.place(Layer::Above, |mtm, target| {
            let popup = NSPopUpButton::initWithFrame_pullsDown(
                NSPopUpButton::alloc(mtm),
                NSRect::ZERO,
                pull_down,
            );
            // Left on, validation re-enables every item as the menu opens.
            popup.setAutoenablesItems(false);
            wire(&popup, target, slot.key);
            popup
        });

        let control_size = NSControlSize::from(self.control_size);
        if popup.controlSize() != control_size {
            popup.setControlSize(control_size);
        }
        // `setPullsDown:` reverses the item array, so the diff runs after it.
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
        // The tick on a pop-up's selected row is that item's `state`, which the diff above just
        // reset from `checked`.
        if selectable && !self.pull_down {
            if let Some(item) = popup.menu().and_then(|menu| menu.itemAtIndex(wanted)) {
                if item.state() != NSControlStateValueOn {
                    item.setState(NSControlStateValueOn);
                }
            }
        }
        let bezel = self.bezel.to_native();
        if popup.bezelStyle() != bezel {
            popup.setBezelStyle(bezel);
        }
        if popup.isEnabled() != self.enabled {
            popup.setEnabled(self.enabled);
        }

        // `integerValue` is the cell state, the same for every row; the selection is this.
        slot.on_event(
            Rc::new(|sender| {
                let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
                Box::new(index)
            }),
            self.on_change.take(),
        );
    }
}

hosted_element!(NativePopup);

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum MenuOrigin {
    #[default]
    Pointer,
    ElementTopLeft,
}

struct SinkIvars {
    picked: Rc<Cell<isize>>,
}

define_class!(
    /// Records which row a transient menu chose. Held in a local for the menu's lifetime, since
    /// `NSMenuItem::setTarget:` is weak.
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

thread_local! {
    static SHOWING: Cell<bool> = const { Cell::new(false) };
}

/// Blocks in AppKit's menu tracking loop until the menu closes; the caller must not hold a GPUI
/// borrow.
fn show_menu(anchor: &Container, location: NSPoint, entries: &[MenuEntry]) -> Option<usize> {
    let mtm = anchor.mtm();
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let picked = Rc::new(Cell::new(-1));
    let sink = MenuSink::new(mtm, picked.clone());
    let target: &AnyObject = sink.as_ref();
    sync_menu(mtm, &menu, entries, Some((target, sel!(ngMenuPick:))));
    menu.popUpMenuPositioningItem_atLocation_inView(None, location, Some(anchor));
    drop(sink);
    let index = picked.get();
    (index >= 0).then_some(index as usize)
}

/// A real `NSMenu` on a GPUI-drawn region, shown on right-click or control-click. Only a
/// `Container` anchor is pooled, to carry the coordinate space.
pub fn native_context_menu(id: impl Into<ElementId>, child: impl IntoElement) -> NativeContextMenu {
    NativeContextMenu {
        id: id.into(),
        child: child.into_any_element(),
        entries: Vec::new(),
        build: None,
        origin: MenuOrigin::default(),
        on_select: None,
        on_close: None,
    }
}
type SelectHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

type EntryBuilder = Rc<dyn Fn(Point<Pixels>, &mut Window, &mut App) -> Vec<MenuEntry>>;

type CloseHandler = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct NativeContextMenu {
    id: ElementId,
    child: AnyElement,
    entries: Vec<MenuEntry>,
    build: Option<EntryBuilder>,
    origin: MenuOrigin,
    on_select: Option<SelectHandler>,
    on_close: Option<CloseHandler>,
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

    pub fn entries_at(
        mut self,
        build: impl Fn(Point<Pixels>, &mut Window, &mut App) -> Vec<MenuEntry> + 'static,
    ) -> Self {
        self.build = Some(Rc::new(build));
        self
    }

    pub fn origin(mut self, origin: MenuOrigin) -> Self {
        self.origin = origin;
        self
    }

    pub fn on_close(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(handler));
        self
    }

    /// The chosen row's depth-first index into the entries, separators and submenu rows counted.
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
        let anchor = host.place(key, Layer::Above, bounds, mask, |mtm, _| {
            Container::new(mtm, NSRect::ZERO, false)
        });
        host.on_change(key, ValueKind::None, None);

        if let Some(handler) = self.on_select.take() {
            let build = self.build.take();
            let closed = self.on_close.take();
            if !self.entries.is_empty() || build.is_some() {
                let entries = std::mem::take(&mut self.entries);
                let origin = self.origin;
                let async_cx = window.to_async(cx);
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let contextual = event.button == MouseButton::Right
                        || (event.button == MouseButton::Left && event.modifiers.control);
                    if !contextual
                        || SHOWING.get()
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
                    let entries = match &build {
                        Some(build) => build(event.position - bounds.origin, window, cx),
                        None => entries.clone(),
                    };
                    if entries.is_empty() {
                        return;
                    }
                    let anchor = anchor.clone();
                    let handler = handler.clone();
                    let closed = closed.clone();
                    SHOWING.set(true);
                    // Deferred: the tracking loop services GPUI's display link, and entering it
                    // under GPUI's `App` borrow panics on the next frame.
                    async_cx
                        .spawn(async move |cx| {
                            let picked = show_menu(&anchor, location, &entries);
                            SHOWING.set(false);
                            if let Some(closed) = closed {
                                let _ = cx.update(|window, app| closed(window, app));
                            }
                            let Some(index) = picked else {
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
