//! The standard macOS menu bar. Every row carries one of AppKit's own selectors and no target, so
//! macOS supplies the key equivalent, localisation, enabling and the Show/Hide title flips. A GPUI
//! app has no menu bar until [`install_menus`], so until then ⌘W, ⌘Q, ⌘M, ⌘H and the Edit menu
//! reach nothing; and a chord bound in GPUI's keymap never reaches the bar.

use std::cell::RefCell;

use gpui::{Action, App, AsyncApp, Keystroke, Menu, MenuItem};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSBundle, NSNumber, NSObject, NSProcessInfo, NSString};

/// One of the standard menus, as a place for an app's own rows.
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

/// An app-provided row: a title, an optional `KeyBinding`-style chord (`"cmd-shift-n"`), and the
/// [`Action`] it dispatches. The chord is the item's key equivalent, not a GPUI keymap binding.
pub struct MenuCommand {
    title: String,
    chord: Option<String>,
    action: Option<Box<dyn Action>>,
}

impl MenuCommand {
    pub fn new(title: impl Into<String>, action: impl Action) -> Self {
        Self {
            title: title.into(),
            chord: None,
            action: Some(Box::new(action)),
        }
    }

    pub fn separator() -> Self {
        Self {
            title: String::new(),
            chord: None,
            action: None,
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
/// the process. One throwaway action makes index 0 exist; every row here keeps tag 0.
fn reserve_gpui_menu_slot(cx: &mut App) {
    cx.set_menus(vec![Menu {
        name: "".into(),
        items: vec![MenuItem::action("", Unhandled)],
    }]);
}

/// Install the standard bar once, before opening a window. A second call is a no-op. Never call
/// `App::set_menus` afterwards: it replaces the bar with rows wired to gpui's own handler.
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

/// Append a menu of the app's own, between View and Window.
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

/// Put rows inside one of the standard menus.
pub fn extend_menu(section: MenuSection, commands: impl IntoIterator<Item = MenuCommand>) {
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
    let mut at = if section == MenuSection::App {
        2
    } else {
        menu.numberOfItems()
    };
    for command in commands {
        add_command(mtm, &menu, at, command);
        at += 1;
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
    into.insertItem_atIndex(&row, at.min(into.numberOfItems()));
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
    Some((keystroke.key.clone(), mask))
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
            let index = unsafe { sender.as_ref() }
                .and_then(|item| item.representedObject())
                .and_then(|object| object.downcast::<NSNumber>().ok())
                .map(|number| number.integerValue());
            let Some(action) = index.and_then(|index| {
                COMMANDS.with_borrow(|commands| {
                    commands
                        .get(index as usize)
                        .map(|action| action.boxed_clone())
                })
            }) else {
                return;
            };
            // Deferred: this fires inside AppKit's menu tracking loop.
            self.ivars()
                .cx
                .spawn(async move |cx: &mut AsyncApp| {
                    let _ = cx.update(|cx| cx.dispatch_action(&*action));
                })
                .detach();
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate(&self, _: *mut AnyObject) -> bool {
            true
        }
    }
);

thread_local! {
    static SHARED: RefCell<Option<Retained<CommandSink>>> = const { RefCell::new(None) };
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
