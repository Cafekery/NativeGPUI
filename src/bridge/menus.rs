use std::cell::{Cell, RefCell};
use std::ffi::CStr;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::system::wording::worded;
use block2::RcBlock;
use gpui::{Action, App, AsyncApp, Keystroke, Menu, MenuItem, SharedString};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{Bool, ProtocolObject, Sel};
use objc2::{ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSEventModifierFlags,
    NSEventTrackingRunLoopMode, NSMenu, NSMenuDelegate, NSMenuItem, NSText,
};
use objc2_foundation::{
    MainThreadMarker, NSBundle, NSNumber, NSObject, NSObjectProtocol, NSProcessInfo, NSRunLoop,
    NSString, NSTimer,
};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum MenuSection {
    App,
    File,
    Edit,
    View,
    Window,
    Help,
}

impl MenuSection {
    fn title(self, app: &str) -> String {
        match self {
            Self::App => app.to_owned(),
            Self::File => "File".into(),
            Self::Edit => "Edit".into(),
            Self::View => "View".into(),
            Self::Window => "Window".into(),
            Self::Help => "Help".into(),
        }
    }
}

/// The chord is the item's key equivalent, not a GPUI keymap binding.
pub struct MenuCommand {
    title: String,
    chord: Option<String>,
    action: Option<Box<dyn Action>>,
    rows: Vec<MenuCommand>,
    alternate: bool,
    option_only: bool,
    checked: bool,
    selector: Option<Sel>,
    fill: Option<Rc<Fill>>,
}

type Fill = dyn Fn() -> Vec<MenuCommand>;

impl MenuCommand {
    pub fn new(title: impl Into<String>, action: impl Action) -> Self {
        Self {
            title: title.into(),
            action: Some(Box::new(action)),
            ..Self::separator()
        }
    }

    /// A row that sends `selector` up the responder chain, as AppKit's own rows do.
    pub fn appkit(title: impl Into<String>, selector: &CStr) -> Self {
        Self {
            title: title.into(),
            selector: Some(Sel::register(selector)),
            ..Self::separator()
        }
    }

    /// Nests `rows` under this row, which its action then only enables.
    pub fn rows(mut self, rows: impl IntoIterator<Item = MenuCommand>) -> Self {
        self.rows = rows.into_iter().collect();
        self
    }

    /// Nests the rows `fill` builds each time the submenu is about to show.
    pub fn filled(mut self, fill: impl Fn() -> Vec<MenuCommand> + 'static) -> Self {
        self.fill = Some(Rc::new(fill));
        self
    }

    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }

    /// Stands in for the row above it while Option is held.
    pub fn option_alternate(mut self) -> Self {
        self.alternate = true;
        self
    }

    pub fn option_only(mut self) -> Self {
        self.option_only = true;
        self
    }

    pub fn submenu(title: impl Into<String>, rows: impl IntoIterator<Item = MenuCommand>) -> Self {
        Self {
            title: title.into(),
            rows: rows.into_iter().collect(),
            ..Self::separator()
        }
    }

    pub fn separator() -> Self {
        Self {
            title: String::new(),
            chord: None,
            action: None,
            rows: Vec::new(),
            alternate: false,
            option_only: false,
            checked: false,
            selector: None,
            fill: None,
        }
    }

    pub fn key(mut self, chord: impl Into<String>) -> Self {
        self.chord = Some(chord.into());
        self
    }
}

fn app_name() -> String {
    let bundle = NSBundle::mainBundle();
    for key in ["CFBundleDisplayName", "CFBundleName"] {
        let name = bundle
            .objectForInfoDictionaryKey(&NSString::from_str(key))
            .and_then(|object| object.downcast::<NSString>().ok())
            .map(|name| name.to_string())
            .filter(|name| !name.is_empty());
        if let Some(name) = name {
            return name;
        }
    }
    NSProcessInfo::processInfo().processName().to_string()
}

const BAR: &str = "NGStandardMenuBar";

const NONE: NSEventModifierFlags = NSEventModifierFlags::empty();
const CMD: NSEventModifierFlags = NSEventModifierFlags::Command;
const CTRL: NSEventModifierFlags = NSEventModifierFlags::Control;
const CMD_SHIFT: NSEventModifierFlags = CMD.union(NSEventModifierFlags::Shift);
const CMD_ALT: NSEventModifierFlags = CMD.union(NSEventModifierFlags::Option);
const CMD_CTRL: NSEventModifierFlags = CMD.union(CTRL);
const CTRL_SHIFT: NSEventModifierFlags = CTRL.union(NSEventModifierFlags::Shift);

fn menu(mtm: MainThreadMarker, title: &str) -> Retained<NSMenu> {
    NSMenu::initWithTitle(NSMenu::alloc(mtm), &worded(title))
}

fn submenu_row(mtm: MainThreadMarker, title: &str, submenu: &NSMenu) -> Retained<NSMenuItem> {
    let row = NSMenuItem::new(mtm);
    row.setTitle(&worded(title));
    row.setSubmenu(Some(submenu));
    row
}

fn section(mtm: MainThreadMarker, bar: &NSMenu, title: &str) -> Retained<NSMenu> {
    let submenu = menu(mtm, title);
    bar.addItem(&submenu_row(mtm, title, &submenu));
    submenu
}

trait Rows {
    fn add_row(&self, title: &str, action: Sel, key: &str, mask: NSEventModifierFlags);
    fn add_separator(&self);
}

impl Rows for NSMenu {
    fn add_row(&self, title: &str, action: Sel, key: &str, mask: NSEventModifierFlags) {
        let row = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(MainThreadMarker::from(self)),
                &worded(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        row.setKeyEquivalentModifierMask(mask);
        self.addItem(&row);
    }

    fn add_separator(&self) {
        self.addItem(&NSMenuItem::separatorItem(MainThreadMarker::from(self)));
    }
}

#[derive(Clone, PartialEq, Action)]
#[action(namespace = native_gpui, no_register)]
struct Unhandled;

/// gpui answers edit rows through a tag table only `App::set_menus` fills, and deadlocks on a miss.
fn reserve_gpui_menu_slot(cx: &mut App) {
    cx.set_menus(vec![Menu {
        name: "".into(),
        items: vec![MenuItem::action("", Unhandled)],
        disabled: false,
    }]);
}

/// Never call `App::set_menus` after this: it replaces the bar.
pub fn install_menus(cx: &mut App) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let name = app_name();
    // A bundled app already has a main menu, so only the bar's own title marks it as ours.
    if app
        .mainMenu()
        .is_some_and(|bar| bar.title().to_string() == BAR)
    {
        return;
    }

    CommandSink::install(mtm, cx);
    reserve_gpui_menu_slot(cx);

    let bar = menu(mtm, BAR);

    let app_menu = section(mtm, &bar, &name);
    app_menu.add_row(
        &format!("About {name}"),
        sel!(orderFrontStandardAboutPanel:),
        "",
        NONE,
    );
    app_menu.add_separator();
    // Rows for `MenuSection::App` go between these two; adjacent separators collapse on screen.
    app_menu.add_separator();
    let services = menu(mtm, "Services");
    let services_row = submenu_row(mtm, "Services", &services);
    services_row.setKeyEquivalentModifierMask(NONE);
    app_menu.addItem(&services_row);
    app.setServicesMenu(Some(&services));
    app_menu.add_separator();
    app_menu.add_row(&format!("Hide {name}"), sel!(hide:), "h", CMD);
    app_menu.add_row("Hide Others", sel!(hideOtherApplications:), "h", CMD_ALT);
    app_menu.add_row("Show All", sel!(unhideAllApplications:), "", NONE);
    app_menu.add_separator();
    app_menu.add_row(&format!("Quit {name}"), sel!(terminate:), "q", CMD);

    let file = section(mtm, &bar, "File");
    crate::chrome::tabs::teach_new_tab(mtm);
    file.add_row("New Tab", sel!(newWindowForTab:), "t", CMD);
    file.add_row("Close", sel!(performClose:), "w", CMD);

    let edit = section(mtm, &bar, "Edit");
    edit.add_row("Undo", sel!(undo:), "z", CMD);
    edit.add_row("Redo", sel!(redo:), "z", CMD_SHIFT);
    edit.add_separator();
    edit.add_row("Cut", sel!(cut:), "x", CMD);
    edit.add_row("Copy", sel!(copy:), "c", CMD);
    edit.add_row("Paste", sel!(paste:), "v", CMD);
    edit.add_row(
        "Paste and Match Style",
        sel!(pasteAsPlainText:),
        "v",
        CMD_ALT | NSEventModifierFlags::Shift,
    );
    edit.add_row("Delete", sel!(delete:), "", NONE);
    edit.add_row("Select All", sel!(selectAll:), "a", CMD);

    // AppKit flips these titles itself; naming the current state would freeze them.
    let view = section(mtm, &bar, "View");
    view.add_row("Show Toolbar", sel!(toggleToolbarShown:), "t", CMD_ALT);
    view.add_row(
        "Customize Toolbar…",
        sel!(runToolbarCustomizationPalette:),
        "",
        NONE,
    );
    view.add_separator();
    view.add_row("Show Tab Bar", sel!(toggleTabBar:), "t", CMD_SHIFT);
    view.add_row("Show All Tabs", sel!(toggleTabOverview:), "\\", CMD_SHIFT);
    view.add_separator();
    view.add_row("Show Sidebar", sel!(toggleSidebar:), "s", CMD_CTRL);
    view.add_separator();
    view.add_row("Enter Full Screen", sel!(toggleFullScreen:), "f", CMD_CTRL);

    let window = section(mtm, &bar, "Window");
    window.add_row("Minimize", sel!(performMiniaturize:), "m", CMD);
    window.add_row("Zoom", sel!(performZoom:), "", NONE);
    window.add_separator();
    window.add_row(
        "Show Previous Tab",
        sel!(selectPreviousTab:),
        "\u{19}",
        CTRL_SHIFT,
    );
    window.add_row("Show Next Tab", sel!(selectNextTab:), "\t", CTRL);
    window.add_row(
        "Move Tab to New Window",
        sel!(moveTabToNewWindow:),
        "",
        NONE,
    );
    window.add_row("Merge All Windows", sel!(mergeAllWindows:), "", NONE);
    window.add_separator();
    window.add_row("Bring All to Front", sel!(arrangeInFront:), "", NONE);

    let help = section(mtm, &bar, "Help");
    help.add_row(&format!("{name} Help"), sel!(showHelp:), "?", CMD);

    app.setMainMenu(Some(&bar));
    app.setWindowsMenu(Some(&window));
    app.setHelpMenu(Some(&help));
}

pub fn add_menu(title: &str, commands: impl IntoIterator<Item = MenuCommand>) {
    let Some((mtm, bar)) = bar() else {
        return;
    };
    let submenu = menu(mtm, title);
    for command in commands {
        add_command(mtm, &submenu, submenu.numberOfItems(), command);
    }
    let before = index_of(&bar, &MenuSection::Window.title(&app_name()))
        .unwrap_or_else(|| bar.numberOfItems());
    bar.insertItem_atIndex(&submenu_row(mtm, title, &submenu), before);
}

pub fn set_menu(title: &str, commands: impl IntoIterator<Item = MenuCommand>) {
    let Some((mtm, bar)) = bar() else {
        return;
    };
    let Some(submenu) = submenu_of(&bar, title) else {
        return;
    };
    submenu.removeAllItems();
    for command in commands {
        add_command(mtm, &submenu, submenu.numberOfItems(), command);
    }
}

pub fn prepend_menu(section: MenuSection, commands: impl IntoIterator<Item = MenuCommand>) {
    rows_into(section, commands, true);
}

pub fn extend_menu(section: MenuSection, commands: impl IntoIterator<Item = MenuCommand>) {
    rows_into(section, commands, false);
}

fn rows_into(section: MenuSection, commands: impl IntoIterator<Item = MenuCommand>, first: bool) {
    let Some((mtm, bar)) = bar() else {
        return;
    };
    let Some(menu) = submenu_of(&bar, &section.title(&app_name())) else {
        return;
    };
    let start = match (first, section) {
        (_, MenuSection::App) => 2,
        (true, _) => 0,
        // AppKit appends Writing Tools, AutoFill, Dictation and Emoji to Edit; an app's rows go above.
        (false, MenuSection::Edit) => {
            match menu.indexOfItemWithTitle(&NSString::from_str("Select All")) {
                -1 => menu.numberOfItems(),
                at => at + 1,
            }
        }
        (false, _) => menu.numberOfItems(),
    };
    for (offset, command) in commands.into_iter().enumerate() {
        add_command(mtm, &menu, start + offset as isize, command);
    }
}

fn bar() -> Option<(MainThreadMarker, Retained<NSMenu>)> {
    let mtm = MainThreadMarker::new()?;
    let bar = NSApplication::sharedApplication(mtm).mainMenu()?;
    Some((mtm, bar))
}

fn index_of(bar: &NSMenu, title: &str) -> Option<isize> {
    bar.itemArray()
        .iter()
        .position(|item| item.title().to_string() == title)
        .map(|index| index as isize)
}

fn submenu_of(bar: &NSMenu, title: &str) -> Option<Retained<NSMenu>> {
    bar.itemAtIndex(index_of(bar, title)?)?.submenu()
}

fn add_command(mtm: MainThreadMarker, into: &NSMenu, at: isize, command: MenuCommand) {
    let (key, mut mask) = command
        .chord
        .as_deref()
        .and_then(equivalent)
        .unwrap_or_else(|| (String::new(), NONE));
    // A key-less alternate pairs with the row above it by its modifier mask alone.
    if command.alternate && key.is_empty() {
        mask = NSEventModifierFlags::Option;
    }
    let nested = !command.rows.is_empty() || command.fill.is_some();
    let row = match command.action {
        None if nested => nested_row(mtm, &command.title, command.rows, command.fill),
        None => match command.selector {
            Some(selector) => {
                let row = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &worded(&command.title),
                        Some(selector),
                        &NSString::from_str(&key),
                    )
                };
                row.setKeyEquivalentModifierMask(mask);
                row
            }
            None => NSMenuItem::separatorItem(mtm),
        },
        Some(action) => {
            let row = match nested {
                true => {
                    let row = nested_row(mtm, &command.title, command.rows, command.fill);
                    // A submenu's row sends nothing; the action only lets validation enable it.
                    unsafe { row.setAction(Some(sel!(ngMenuCommand:))) };
                    row
                }
                false => unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &worded(&command.title),
                        Some(sel!(ngMenuCommand:)),
                        &NSString::from_str(&key),
                    )
                },
            };
            row.setKeyEquivalentModifierMask(mask);
            // The index lives in `representedObject`, never the tag: see `reserve_gpui_menu_slot`.
            let index = NSNumber::new_isize(CommandSink::register(action));
            unsafe { row.setRepresentedObject(Some(index.as_ref())) };
            let Some(sink) = SHARED.with_borrow(|shared| shared.clone()) else {
                return;
            };
            unsafe { row.setTarget(Some(sink.as_ref())) };
            row
        }
    };
    if command.alternate {
        row.setAlternate(true);
    }
    if command.option_only {
        show_with_option(&row);
    }
    set_state(&row, command.checked);
    into.insertItem_atIndex(&row, at.min(into.numberOfItems()));
}

thread_local! {
    static FILLS: RefCell<Vec<(Weak<NSMenu>, Rc<Fill>)>> = const { RefCell::new(Vec::new()) };
}

fn nested_row(
    mtm: MainThreadMarker,
    title: &str,
    rows: Vec<MenuCommand>,
    fill: Option<Rc<Fill>>,
) -> Retained<NSMenuItem> {
    let inner = menu(mtm, title);
    for row in rows {
        add_command(mtm, &inner, inner.numberOfItems(), row);
    }
    if let Some(fill) = fill {
        if let Some(sink) = SHARED.with_borrow(|shared| shared.clone()) {
            inner.setDelegate(Some(ProtocolObject::from_ref(&*sink)));
        }
        FILLS.with_borrow_mut(|fills| {
            fills.retain(|(menu, _)| menu.load().is_some());
            fills.push((Weak::new(&*inner), fill));
        });
    }
    submenu_row(mtm, title, &inner)
}

fn refill(menu: &NSMenu) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let fill = FILLS.with_borrow(|fills| {
        fills
            .iter()
            .find(|(weak, _)| weak.load().is_some_and(|known| std::ptr::eq(&*known, menu)))
            .map(|(_, fill)| fill.clone())
    });
    let Some(fill) = fill else {
        return;
    };
    menu.removeAllItems();
    for command in fill() {
        add_command(mtm, menu, menu.numberOfItems(), command);
    }
}

thread_local! {
    static OPTION_ONLY: RefCell<Vec<Weak<NSMenuItem>>> = const { RefCell::new(Vec::new()) };
    static OPTION_POLL: Cell<bool> = const { Cell::new(false) };
}

/// Menu tracking takes flagsChanged before any monitor sees it, so a tracking-mode timer polls.
fn show_with_option(row: &NSMenuItem) {
    row.setHidden(true);
    OPTION_ONLY.with_borrow_mut(|rows| rows.push(Weak::new(row)));
    if OPTION_POLL.replace(true) {
        return;
    }
    let block = RcBlock::new(|_: NonNull<NSTimer>| {
        let held = NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Option);
        OPTION_ONLY.with_borrow_mut(|rows| {
            rows.retain(|row| row.load().inspect(|row| row.setHidden(!held)).is_some())
        });
    });
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(0.05, true, &block) };
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSEventTrackingRunLoopMode) };
}

fn editing_text() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    NSApplication::sharedApplication(mtm)
        .keyWindow()
        .and_then(|window| window.firstResponder())
        .is_some_and(|responder| responder.isKindOfClass(NSText::class()))
}

/// The field editor's own ⌘ and ⌥ delete and arrow chords, which a row would take off the text.
fn edits_text(item: &NSMenuItem) -> bool {
    const DELETE: &str = "\u{8}";
    const ARROWS: [&str; 4] = ["\u{f700}", "\u{f701}", "\u{f702}", "\u{f703}"];
    let key = item.keyEquivalent().to_string();
    let held = item.keyEquivalentModifierMask() - NSEventModifierFlags::Shift;
    let alone = held == NSEventModifierFlags::Command || held == NSEventModifierFlags::Option;
    alone && (key == DELETE || ARROWS.contains(&key.as_str()))
}

fn equivalent_key(key: &str) -> Option<String> {
    let named = match key {
        "backspace" => '\u{8}',
        "delete" => '\u{f728}',
        "enter" => '\r',
        "tab" => '\t',
        "escape" => '\u{1b}',
        "space" => ' ',
        "up" => '\u{f700}',
        "down" => '\u{f701}',
        "left" => '\u{f702}',
        "right" => '\u{f703}',
        "home" => '\u{f729}',
        "end" => '\u{f72b}',
        "pageup" => '\u{f72c}',
        "pagedown" => '\u{f72d}',
        _ => {
            let mut chars = key.chars();
            let single = chars.next().filter(|_| chars.next().is_none())?;
            single.to_ascii_lowercase()
        }
    };
    Some(named.to_string())
}

fn equivalent(chord: &str) -> Option<(String, NSEventModifierFlags)> {
    let keystroke = Keystroke::parse(chord).ok()?;
    let mut mask = NSEventModifierFlags::empty();
    for (held, flag) in [
        (keystroke.modifiers.platform, NSEventModifierFlags::Command),
        (keystroke.modifiers.control, NSEventModifierFlags::Control),
        (keystroke.modifiers.alt, NSEventModifierFlags::Option),
        (keystroke.modifiers.shift, NSEventModifierFlags::Shift),
        (keystroke.modifiers.function, NSEventModifierFlags::Function),
    ] {
        if held {
            mask |= flag;
        }
    }
    Some((equivalent_key(&keystroke.key)?, mask))
}

fn set_state(row: &NSMenuItem, on: bool) {
    row.setState(match on {
        true => NSControlStateValueOn,
        false => NSControlStateValueOff,
    });
}

type MenuState = dyn Fn(&dyn Action) -> Option<bool>;

/// Each `Some` is that row's state. Runs while gpui holds its `App`: read state of your own.
pub fn on_menu_state(check: impl Fn(&dyn Action) -> Option<bool> + 'static) {
    STATE.with_borrow_mut(|slot| *slot = Some(Rc::new(check)));
}

/// Rows it answers `false` for are disabled; the rest stay enabled.
pub fn on_menu_enabled(check: impl Fn(&dyn Action) -> bool + 'static) {
    ENABLED.with_borrow_mut(|slot| *slot = Some(Rc::new(check)));
}

/// Rows it names take that title each time they are validated.
pub fn on_menu_title(title: impl Fn(&dyn Action) -> Option<SharedString> + 'static) {
    TITLE.with_borrow_mut(|slot| *slot = Some(Rc::new(title)));
}

fn action_of(item: &NSMenuItem) -> Option<Box<dyn Action>> {
    let index = item
        .representedObject()
        .and_then(|object| object.downcast::<NSNumber>().ok())
        .map(|number| number.integerValue())?;
    COMMANDS.with_borrow(|commands| {
        commands
            .get(index as usize)
            .map(|action| action.boxed_clone())
    })
}

struct SinkIvars {
    cx: AsyncApp,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGMenuBarSink"]
    #[ivars = SinkIvars]
    struct CommandSink;

    impl CommandSink {
        #[unsafe(method(ngMenuCommand:))]
        fn ng_menu_command(&self, sender: *mut NSMenuItem) {
            let Some(action) = unsafe { sender.as_ref() }.and_then(action_of) else {
                return;
            };
            self.ivars()
                .cx
                .spawn(async move |cx: &mut AsyncApp| {
                    cx.update(|cx| cx.dispatch_action(&*action));
                })
                .detach();
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate(&self, sender: *mut NSMenuItem) -> Bool {
            let Some(item) = (unsafe { sender.as_ref() }) else {
                return Bool::YES;
            };
            if editing_text() && edits_text(item) {
                return Bool::NO;
            }
            let Some(action) = action_of(item) else {
                return Bool::YES;
            };
            if let Some(on) = STATE.with_borrow(|slot| slot.clone()).and_then(|check| check(&*action))
            {
                set_state(item, on);
            }
            if let Some(title) = TITLE.with_borrow(|slot| slot.clone()).and_then(|t| t(&*action)) {
                item.setTitle(&worded(&title));
            }
            let enabled = ENABLED.with_borrow(|slot| slot.clone());
            Bool::new(enabled.is_none_or(|enabled| enabled(&*action)))
        }
    }

    unsafe impl NSObjectProtocol for CommandSink {}

    unsafe impl NSMenuDelegate for CommandSink {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            refill(menu);
        }
    }
);

thread_local! {
    static SHARED: RefCell<Option<Retained<CommandSink>>> = const { RefCell::new(None) };
    static STATE: RefCell<Option<Rc<MenuState>>> = const { RefCell::new(None) };
    static ENABLED: RefCell<Option<Rc<dyn Fn(&dyn Action) -> bool>>> = const { RefCell::new(None) };
    static TITLE: RefCell<Option<Rc<dyn Fn(&dyn Action) -> Option<SharedString>>>> =
        const { RefCell::new(None) };
    static COMMANDS: RefCell<Vec<Box<dyn Action>>> = const { RefCell::new(Vec::new()) };
}

impl CommandSink {
    fn install(mtm: MainThreadMarker, cx: &mut App) {
        let this = Self::alloc(mtm).set_ivars(SinkIvars { cx: cx.to_async() });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        SHARED.with_borrow_mut(|shared| *shared = Some(this));
    }

    fn register(action: Box<dyn Action>) -> isize {
        COMMANDS.with_borrow_mut(|commands| {
            commands.push(action);
            commands.len() as isize - 1
        })
    }
}

#[cfg(test)]
mod tests {
    use super::equivalent_key;

    #[test]
    fn a_named_key_is_the_character_it_types() {
        assert_eq!(equivalent_key("backspace").as_deref(), Some("\u{8}"));
        assert_eq!(equivalent_key("up").as_deref(), Some("\u{f700}"));
        assert_eq!(equivalent_key("N").as_deref(), Some("n"));
        assert_eq!(equivalent_key("/").as_deref(), Some("/"));
        assert_eq!(equivalent_key("f13"), None);
    }
}
