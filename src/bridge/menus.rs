//! Every row carries one of AppKit's own selectors and no target, so macOS supplies the key
//! equivalent, localisation, enabling and the Show/Hide title flips.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use gpui::{Action, App, AsyncApp, Keystroke, Menu, MenuItem};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{Bool, Sel};
use objc2::{ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSEventModifierFlags,
    NSEventTrackingRunLoopMode, NSMenu, NSMenuItem, NSText,
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
}

impl MenuCommand {
    pub fn new(title: impl Into<String>, action: impl Action) -> Self {
        Self {
            title: title.into(),
            chord: None,
            action: Some(Box::new(action)),
            rows: Vec::new(),
            alternate: false,
            option_only: false,
            checked: false,
        }
    }

    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }

    /// The row the one above it becomes while Option is held, as Finder swaps Duplicate.
    pub fn option_alternate(mut self) -> Self {
        self.alternate = true;
        self
    }

    /// A row that is there only while Option is held, as Finder's Go ▸ Library. AppKit hides an
    /// alternate only when it stands in for the row above, so this one is hidden and shown by hand.
    pub fn option_only(mut self) -> Self {
        self.option_only = true;
        self
    }

    pub fn submenu(title: impl Into<String>, rows: impl IntoIterator<Item = MenuCommand>) -> Self {
        Self {
            title: title.into(),
            chord: None,
            action: None,
            rows: rows.into_iter().collect(),
            alternate: false,
            option_only: false,
            checked: false,
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

/// The bar's own title, never displayed, marks it as this crate's.
const BAR: &str = "NGStandardMenuBar";

const CMD: NSEventModifierFlags = NSEventModifierFlags::Command;
const NONE: NSEventModifierFlags = NSEventModifierFlags::empty();

fn shift() -> NSEventModifierFlags {
    CMD | NSEventModifierFlags::Shift
}

fn alt() -> NSEventModifierFlags {
    CMD | NSEventModifierFlags::Option
}

fn ctrl() -> NSEventModifierFlags {
    CMD | NSEventModifierFlags::Control
}

fn ctrl_shift() -> NSEventModifierFlags {
    NSEventModifierFlags::Control | NSEventModifierFlags::Shift
}

fn menu(mtm: MainThreadMarker, title: &str) -> Retained<NSMenu> {
    NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title))
}

fn item(
    mtm: MainThreadMarker,
    into: &NSMenu,
    title: &str,
    selector: Sel,
    key: &str,
    mask: NSEventModifierFlags,
) -> Retained<NSMenuItem> {
    let row = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(selector),
            &NSString::from_str(key),
        )
    };
    row.setKeyEquivalentModifierMask(mask);
    into.addItem(&row);
    row
}

fn separator(mtm: MainThreadMarker, into: &NSMenu) {
    into.addItem(&NSMenuItem::separatorItem(mtm));
}

fn section(mtm: MainThreadMarker, bar: &NSMenu, title: &str) -> Retained<NSMenu> {
    let host = NSMenuItem::new(mtm);
    host.setTitle(&NSString::from_str(title));
    let submenu = menu(mtm, title);
    host.setSubmenu(Some(&submenu));
    bar.addItem(&host);
    submenu
}

#[derive(Clone, PartialEq, Action)]
#[action(namespace = native_gpui, no_register)]
struct Unhandled;

/// gpui's app delegate answers Cut/Copy/Paste/Undo/Redo/Select All by looking the sender's tag
/// up in a table only `App::set_menus` fills, and on a miss re-locks a mutex it holds, freezing
/// the process.
fn reserve_gpui_menu_slot(cx: &mut App) {
    cx.set_menus(vec![Menu {
        name: "".into(),
        items: vec![MenuItem::action("", Unhandled)],
        disabled: false,
    }]);
}

/// Never call `App::set_menus` after this: it replaces the bar with rows wired to gpui's own
/// handler.
pub fn install_menus(cx: &mut App) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let name = app_name();
    // A bundled app already has a main menu titled with the app's name, so the bar's own title
    // is the only reliable mark.
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
    item(
        mtm,
        &app_menu,
        &format!("About {name}"),
        sel!(orderFrontStandardAboutPanel:),
        "",
        NONE,
    );
    separator(mtm, &app_menu);
    // The reserved block `extend_menu(MenuSection::App, ..)` inserts into; adjacent separators
    // collapse on screen.
    separator(mtm, &app_menu);
    let services = menu(mtm, "Services");
    let services_host = NSMenuItem::new(mtm);
    services_host.setTitle(&NSString::from_str("Services"));
    services_host.setSubmenu(Some(&services));
    services_host.setKeyEquivalentModifierMask(NONE);
    app_menu.addItem(&services_host);
    app.setServicesMenu(Some(&services));
    separator(mtm, &app_menu);
    item(
        mtm,
        &app_menu,
        &format!("Hide {name}"),
        sel!(hide:),
        "h",
        CMD,
    );
    item(
        mtm,
        &app_menu,
        "Hide Others",
        sel!(hideOtherApplications:),
        "h",
        alt(),
    );
    item(
        mtm,
        &app_menu,
        "Show All",
        sel!(unhideAllApplications:),
        "",
        NONE,
    );
    separator(mtm, &app_menu);
    item(
        mtm,
        &app_menu,
        &format!("Quit {name}"),
        sel!(terminate:),
        "q",
        CMD,
    );

    let file = section(mtm, &bar, "File");
    crate::chrome::tabs::teach_new_tab(mtm);
    // Live only for windows that opted into tabs (`window_tabs`); AppKit validates it.
    item(mtm, &file, "New Tab", sel!(newWindowForTab:), "t", CMD);
    item(mtm, &file, "Close", sel!(performClose:), "w", CMD);

    let edit = section(mtm, &bar, "Edit");
    item(mtm, &edit, "Undo", sel!(undo:), "z", CMD);
    item(mtm, &edit, "Redo", sel!(redo:), "z", shift());
    separator(mtm, &edit);
    item(mtm, &edit, "Cut", sel!(cut:), "x", CMD);
    item(mtm, &edit, "Copy", sel!(copy:), "c", CMD);
    item(mtm, &edit, "Paste", sel!(paste:), "v", CMD);
    item(
        mtm,
        &edit,
        "Paste and Match Style",
        sel!(pasteAsPlainText:),
        "v",
        CMD | NSEventModifierFlags::Option | NSEventModifierFlags::Shift,
    );
    item(mtm, &edit, "Delete", sel!(delete:), "", NONE);
    item(mtm, &edit, "Select All", sel!(selectAll:), "a", CMD);

    // AppKit flips these titles itself; naming the current state would freeze them.
    let view = section(mtm, &bar, "View");
    item(
        mtm,
        &view,
        "Show Toolbar",
        sel!(toggleToolbarShown:),
        "t",
        alt(),
    );
    item(
        mtm,
        &view,
        "Customize Toolbar…",
        sel!(runToolbarCustomizationPalette:),
        "",
        NONE,
    );
    separator(mtm, &view);
    item(
        mtm,
        &view,
        "Show Tab Bar",
        sel!(toggleTabBar:),
        "t",
        shift(),
    );
    item(
        mtm,
        &view,
        "Show All Tabs",
        sel!(toggleTabOverview:),
        "\\",
        shift(),
    );
    separator(mtm, &view);
    item(
        mtm,
        &view,
        "Show Sidebar",
        sel!(toggleSidebar:),
        "s",
        ctrl(),
    );
    separator(mtm, &view);
    item(
        mtm,
        &view,
        "Enter Full Screen",
        sel!(toggleFullScreen:),
        "f",
        ctrl(),
    );

    let window = section(mtm, &bar, "Window");
    item(
        mtm,
        &window,
        "Minimize",
        sel!(performMiniaturize:),
        "m",
        CMD,
    );
    item(mtm, &window, "Zoom", sel!(performZoom:), "", NONE);
    separator(mtm, &window);
    item(
        mtm,
        &window,
        "Show Previous Tab",
        sel!(selectPreviousTab:),
        "\u{19}",
        ctrl_shift(),
    );
    item(
        mtm,
        &window,
        "Show Next Tab",
        sel!(selectNextTab:),
        "\t",
        NSEventModifierFlags::Control,
    );
    item(
        mtm,
        &window,
        "Move Tab to New Window",
        sel!(moveTabToNewWindow:),
        "",
        NONE,
    );
    item(
        mtm,
        &window,
        "Merge All Windows",
        sel!(mergeAllWindows:),
        "",
        NONE,
    );
    separator(mtm, &window);
    item(
        mtm,
        &window,
        "Bring All to Front",
        sel!(arrangeInFront:),
        "",
        NONE,
    );

    let help = section(mtm, &bar, "Help");
    item(
        mtm,
        &help,
        &format!("{name} Help"),
        sel!(showHelp:),
        "?",
        CMD,
    );

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
    let host = NSMenuItem::new(mtm);
    host.setTitle(&NSString::from_str(title));
    host.setSubmenu(Some(&submenu));
    let before = index_of(&bar, &MenuSection::Window.title(&app_name()))
        .unwrap_or_else(|| bar.numberOfItems());
    bar.insertItem_atIndex(&host, before);
}

pub fn set_menu(title: &str, commands: impl IntoIterator<Item = MenuCommand>) {
    let Some((mtm, bar)) = bar() else {
        return;
    };
    let Some(host) = index_of(&bar, title).map(|at| bar.itemAtIndex(at)) else {
        return;
    };
    let Some(submenu) = host.and_then(|item| item.submenu()) else {
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
    let name = app_name();
    let Some(menu) = index_of(&bar, &section.title(&name))
        .and_then(|index| bar.itemAtIndex(index))
        .and_then(|host| host.submenu())
    else {
        return;
    };
    let start = match (first, section) {
        (_, MenuSection::App) => 2,
        (true, _) => 0,
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

fn add_command(mtm: MainThreadMarker, into: &NSMenu, at: isize, command: MenuCommand) {
    let row = match command.action {
        None if !command.rows.is_empty() => {
            let host = NSMenuItem::new(mtm);
            host.setTitle(&NSString::from_str(&command.title));
            let inner = menu(mtm, &command.title);
            for row in command.rows {
                add_command(mtm, &inner, inner.numberOfItems(), row);
            }
            host.setSubmenu(Some(&inner));
            host
        }
        None => NSMenuItem::separatorItem(mtm),
        Some(action) => {
            let (key, mask) = command
                .chord
                .as_deref()
                .and_then(equivalent)
                .unwrap_or_else(|| (String::new(), NONE));
            let row = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(&command.title),
                    Some(sel!(ngMenuCommand:)),
                    &NSString::from_str(&key),
                )
            };
            row.setKeyEquivalentModifierMask(mask);
            // The index lives in `representedObject`, never the tag: see `reserve_gpui_menu_slot`.
            let index = NSNumber::new_isize(CommandSink::register(action));
            unsafe { row.setRepresentedObject(Some(index.as_ref())) };
            let Some(sink) = CommandSink::shared() else {
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
    static OPTION_ONLY: RefCell<Vec<Weak<NSMenuItem>>> = const { RefCell::new(Vec::new()) };
    static OPTION_POLL: Cell<bool> = const { Cell::new(false) };
}

/// Menu tracking takes flagsChanged before any monitor sees it, so while a menu is open a timer in
/// the tracking mode reads the held modifiers instead. Rows a rebuilt menu dropped fall out of the
/// list as they go.
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

/// A field editor is what AppKit puts in front of an `NSTextField` while it is being typed into.
fn editing_text() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    NSApplication::sharedApplication(mtm)
        .keyWindow()
        .and_then(|window| window.firstResponder())
        .is_some_and(|responder| responder.isKindOfClass(NSText::class()))
}

/// Cocoa's standard bindings give the field editor ⌘ and ⌥ with delete and the arrows, and a menu
/// that answered those first would take the keystroke off the text being typed.
fn edits_text(item: &NSMenuItem) -> bool {
    const DELETE: &str = "\u{8}";
    const ARROWS: [&str; 4] = ["\u{f700}", "\u{f701}", "\u{f702}", "\u{f703}"];
    let key = item.keyEquivalent().to_string();
    let held = item.keyEquivalentModifierMask() - NSEventModifierFlags::Shift;
    let alone = held == NSEventModifierFlags::Command || held == NSEventModifierFlags::Option;
    alone && (key == DELETE || ARROWS.contains(&key.as_str()))
}

/// AppKit matches a key equivalent against the characters an event carries, so a named key is
/// the character it types: `"backspace"` as a title never matches ⌫.
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

/// AppKit validates every row as its menu opens, and each `Some` answer is that row's state. The
/// closure runs inside AppKit's menu tracking, where gpui holds its `App`, so it must read state
/// of its own.
pub fn on_menu_state(check: impl Fn(&dyn Action) -> Option<bool> + 'static) {
    STATE.with_borrow_mut(|slot| *slot = Some(Rc::new(check)));
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
    /// The target every app-provided row points at, held for the life of the process.
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
            // Deferred: this fires inside AppKit's menu tracking loop.
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
            let check = STATE.with_borrow(|slot| slot.clone());
            if let Some(on) = check.zip(action_of(item)).and_then(|(check, action)| check(&*action))
            {
                set_state(item, on);
            }
            Bool::YES
        }
    }
);

thread_local! {
    static SHARED: RefCell<Option<Retained<CommandSink>>> = const { RefCell::new(None) };
    static STATE: RefCell<Option<Rc<MenuState>>> = const { RefCell::new(None) };
    static COMMANDS: RefCell<Vec<Box<dyn Action>>> = const { RefCell::new(Vec::new()) };
}

impl CommandSink {
    fn install(mtm: MainThreadMarker, cx: &mut App) {
        let this = Self::alloc(mtm).set_ivars(SinkIvars { cx: cx.to_async() });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        SHARED.with_borrow_mut(|shared| *shared = Some(this));
    }

    fn shared() -> Option<Retained<Self>> {
        SHARED.with_borrow(|shared| shared.clone())
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
