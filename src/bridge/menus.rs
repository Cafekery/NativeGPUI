//! The standard macOS menu bar, which is where every ⌘-key equivalent actually lives.
//!
//! [`install_menus`] installs the App / File / Edit / View / Window / Help set once at startup.
//! Every row carries one of AppKit's own selectors and **no target**, so macOS supplies the key
//! equivalent, its localisation, the enable/disable logic, the ellipsis conventions and the
//! Show/Hide title flips. This crate supplies the titles and the app's name, and nothing else.
//!
//! A GPUI app has no menu bar until something asks for one, so without this call ⌘W, ⌘Q, ⌘M, ⌘H
//! and the whole Edit menu reach nothing: the chord is offered to the view hierarchy, declined,
//! offered to a menu bar that is not there, and dropped.
//!
//! # Which layer answers a chord
//!
//! AppKit offers a ⌘-chord to the key window's view hierarchy first and to the menu bar **only
//! after the window declines**, so GPUI's keymap wins every chord it binds and the bar never sees
//! that one. The standard set therefore binds nothing into GPUI's keymap: these chords are
//! AppKit's, and an app that binds one of them in GPUI takes it away from macOS.
//!
//! The order, for one chord, in a window running this crate:
//!
//! 1. hosted AppKit subviews, via the plain `NSView` walk this crate restores — a hosted default
//!    button's Return, a hosted control's own equivalent;
//! 2. GPUI's keymap, but only while GPUI's view is first responder;
//! 3. this menu bar.
//!
//! Measured, both arms, on the bundled app through `NSApp.sendEvent:` so AppKit did the
//! sequencing rather than a probe calling the layers in an order it chose: ⌘M with no GPUI
//! binding reached the menu and miniaturised the window, with zero GPUI actions; the same chord
//! bound in GPUI's keymap raised one GPUI action and left the window alone. Exclusive in both
//! directions, never both layers.
//!
//! The rule an app author needs, and it is not guessable: **binding a chord in GPUI takes it away
//! from macOS.** Bind ⌘W in your keymap and Close stops closing. If you want a command to be
//! context-sensitive, bind it and handle it yourself; if you want macOS's behaviour, leave it
//! alone.
//!
//! # Adding an app's own menus
//!
//! [`install_menus`] first, then [`add_menu`] for a whole menu of your own and [`extend_menu`] to
//! put rows inside a standard one. Both keep the standard set, and a second `install_menus` is a
//! no-op rather than a rebuild, so nothing an app added is silently wiped by one.
//!
//! Never call `App::set_menus` afterwards. It replaces the whole bar, and it builds every row
//! with gpui's own handler selector rather than AppKit's — which is how ⌘W ends up present in the
//! menu, correctly labelled, and doing nothing.
//!
//! An app row dispatches a gpui [`Action`], on the next run-loop turn rather than in place: it
//! fires from inside AppKit's menu tracking loop, and this crate defers every AppKit callback for
//! that reason.
//!
//! # Every row this file installs keeps tag 0. Changing that freezes the app.
//!
//! Not a style rule. gpui's app delegate implements `cut:`, `copy:`, `paste:`, `selectAll:`,
//! `undo:` and `redo:`, and it is the last responder AppKit reaches — so it is what gets asked to
//! validate, and to run, every Edit row nothing earlier in the chain wants, which is every one of
//! them whenever no text field has focus. Both of its handlers look the sender's tag up in a table
//! only `App::set_menus` fills, and **on a miss each re-locks a mutex it is still holding**. The
//! process stops dead — no panic, no log, no timeout, the whole app frozen the first time a user
//! opens the Edit menu. It was found by sampling a hung probe, whose main thread sat in
//! `RawMutex::lock_slow` under `-[NSMenu _enableItem:]`.
//!
//! [`reserve_gpui_menu_slot`] hands gpui one throwaway action so index 0 exists, and every row
//! built here keeps the default tag of 0 so every lookup hits it. An app row's own index lives in
//! `representedObject` for exactly this reason.
//!
//! How far that closes the trap, since "it works now" is not the question:
//!
//! * gpui's table only ever grows — nothing in it is cleared or reassigned — so once index 0
//!   exists it stays valid for the life of the process. That part is robust, not merely sufficient.
//! * An app calling `App::set_menus` later appends more actions, so index 0 survives; what it
//!   destroys is the bar, which is the documented reason not to.
//! * The live edge is a *non-zero* tag on a row whose action can reach gpui's delegate. Nothing
//!   here sets one, and app rows carry an explicit target so AppKit asks that target instead — but
//!   a future row that both takes a tag and keeps a nil target would reintroduce the freeze.

use std::cell::RefCell;

use gpui::{Action, App, AsyncApp, Keystroke, Menu, MenuItem};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSBundle, NSNumber, NSObject, NSProcessInfo, NSString};

/// One of the menus [`install_menus`] installs, as a place to put an app's own rows.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum MenuSection {
    /// The leftmost menu, named after the app. Rows land in the block Apple keeps Settings… in:
    /// below About, above Services. Appending would put them under Quit.
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

/// One app-provided menu row: a title, an optional chord, and the gpui [`Action`] it dispatches.
///
/// The chord is spelled the way `KeyBinding` spells one — `"cmd-shift-n"`, `"cmd-alt-t"` — and
/// becomes the item's own key equivalent. It is *not* bound in GPUI's keymap: bind it yourself if
/// the command has to be context-sensitive, and GPUI will then answer it first.
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

    /// A divider. Occupies a row of its own, as it does in `NSMenu`.
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

/// The name macOS puts in About X, Hide X, Quit X and X Help.
///
/// `CFBundleDisplayName` then `CFBundleName` then the process name, which is what an unbundled
/// `cargo run` has: the binary's own name, which is the best answer available and reads correctly.
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

/// The main menu's own title. Never displayed — AppKit draws the app's name over the first item
/// regardless — so it is free to use as the mark that says this bar is this crate's.
const BAR: &str = "NGStandardMenuBar";

const CMD: NSEventModifierFlags = NSEventModifierFlags::Command;
/// A row with no key equivalent. Written explicitly rather than left alone, because an
/// `NSMenuItem`'s mask defaults to Command whether it has an equivalent or not, and a table read
/// back out of AppKit should show the difference.
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

/// A row with one of AppKit's selectors and no target. The nil target is the whole point: it is
/// what asks AppKit to find a responder, so `performClose:` lands on the key window and `undo:`
/// on whatever owns the undo manager at the time, without this crate naming either.
fn item(
    mtm: MainThreadMarker,
    into: &NSMenu,
    title: &str,
    selector: Sel,
    key: &str,
    mask: NSEventModifierFlags,
) -> Retained<NSMenuItem> {
    // SAFETY: every selector below is a documented AppKit action — one object argument, void
    // return — and is looked up by name at build time by `sel!`.
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

/// A top-level menu: a titled item on the bar whose submenu holds the rows.
fn section(mtm: MainThreadMarker, bar: &NSMenu, title: &str) -> Retained<NSMenu> {
    let host = NSMenuItem::new(mtm);
    host.setTitle(&NSString::from_str(title));
    let submenu = menu(mtm, title);
    host.setSubmenu(Some(&submenu));
    bar.addItem(&host);
    submenu
}

/// One action, never listened for, registered only to make gpui's menu-action table non-empty.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = native_gpui, no_register)]
struct Unhandled;

/// Hands gpui one menu action so index 0 of its table exists, then throws away the bar that came
/// with it.
///
/// Cut, Copy, Paste, Select All, Undo and Redo are implemented by gpui's own app delegate, which
/// is the last responder AppKit reaches — so the delegate is what gets asked to validate, and to
/// run, every one of those rows that nothing earlier in the chain wants, which is every one of
/// them whenever no text field has focus. Both of gpui's handlers look the sender's tag up in a
/// table that only `App::set_menus` fills, and **on a miss each re-locks a mutex it is still
/// holding**: the process stops dead, with no panic, no log and no timeout. Sampled from a hung
/// probe, the main thread sat in `RawMutex::lock_slow` under `-[NSMenu _enableItem:]`, forever.
///
/// So one throwaway action goes through `set_menus` first, purely so a lookup can succeed. Every
/// standard row keeps the default tag of 0 and therefore finds it, and what comes back is the
/// availability of an action nothing listens to — false, which is the enable state a row nothing
/// in the chain wants should have had anyway. The bar `set_menus` builds on the way past is
/// replaced a few lines later and is never on screen.
fn reserve_gpui_menu_slot(cx: &mut App) {
    cx.set_menus(vec![Menu {
        name: "".into(),
        items: vec![MenuItem::action("", Unhandled)],
    }]);
}

/// Install the standard menu bar. Call once from inside `Application::run`, before opening a
/// window. Calling it again does nothing, so an app's own menus survive a stray second call.
///
/// AppKit then adds rows of its own to what it has been handed, and nothing here may assume these
/// menus contain only what it put in them: a Close All alternate beside Close, Writing Tools,
/// AutoFill, Start Dictation… and Emoji & Symbols in Edit, and for `toggleFullScreen:` a pair of
/// Globe-key alternates — the ⌃⌘F asked for below reads back as Globe-F, macOS 26 having moved
/// that chord. Inheriting all of it is the point of using the real selectors.
pub fn install_menus(cx: &mut App) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let name = app_name();
    // Ours is recognised by the *bar's* own title, which nothing displays and nothing else sets.
    //
    // Not by its first menu's title being the app's name, which is what this used to do: a
    // **bundled** app already has a main menu when `Application::run` starts, and AppKit titles
    // its first item with the app's name. So that test matched the system's bar on the very first
    // call, `install_menus` returned having installed nothing, and the app came up with one empty
    // menu and no ⌘W, ⌘Q, ⌘M or ⌘H at all. Unbundled it passed, because there `mainMenu` is nil —
    // the failure existed only in the configuration every shipped app is in.
    if app
        .mainMenu()
        .is_some_and(|bar| bar.title().to_string() == BAR)
    {
        return;
    }

    // Registers the dispatch sink now so `add_menu` and `extend_menu` need no context of their
    // own, and so the `AsyncApp` they defer through is the one from startup.
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
    // Where `extend_menu(MenuSection::App, ..)` inserts, and where Settings… goes. Kept even when
    // empty: two adjacent separators collapse to one on screen, so the reserved slot costs nothing.
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

    // `undo:` and `redo:` are implemented by neither a hosted field's field editor, nor
    // `NSResponder`, nor `NSUndoManager` — they are `NSWindow`'s, which forwards to the undo
    // manager it gets from its first responder. So a nil target walks the chain past the field
    // editor to the window and lands on the focused field's own undo stack. gpui builds these two
    // items with its own handler selector instead, on the assumption that there is no `NSTextView`
    // to enable them on; a hosted field has one.
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

    // Every title here is the one AppKit rewrites: it flips Show/Hide Toolbar, Show/Hide Sidebar
    // and Enter/Exit Full Screen itself while validating the row, so naming the current state
    // would freeze it at whatever this line said.
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
    // Both are what makes AppKit maintain these menus rather than merely display them: the window
    // list appends itself under Bring All to Front, and Help gains the system's search field.
    app.setWindowsMenu(Some(&window));
    app.setHelpMenu(Some(&help));
}

/// Append a menu of the app's own, between View and Window — where Finder keeps Go.
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

/// Put rows inside one of the standard menus, keeping the rest of it standard.
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
    // The App menu's rows belong in the reserved block below About; everywhere else, after the
    // standard rows.
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
            // SAFETY: `ngMenuCommand:` is defined below with exactly this signature.
            let row = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(&command.title),
                    Some(sel!(ngMenuCommand:)),
                    &NSString::from_str(&key),
                )
            };
            row.setKeyEquivalentModifierMask(mask);
            // The index goes in `representedObject`, not the tag, and every row this crate
            // installs keeps tag 0 — see [`reserve_gpui_menu_slot`] for what a tag gpui cannot
            // resolve does. Nothing else asks an `NSMenuItem` which action it carries, so this is
            // the same place a symbol name and a view key are parked elsewhere in the crate.
            let index = NSNumber::new_isize(CommandSink::register(action));
            unsafe { row.setRepresentedObject(Some(index.as_ref())) };
            // An app row is the one kind that needs a target: nothing in the responder chain
            // implements `ngMenuCommand:`, so a nil target would leave it permanently greyed out.
            let Some(sink) = CommandSink::shared() else {
                return;
            };
            unsafe { row.setTarget(Some(sink.as_ref())) };
            row
        }
    };
    into.insertItem_atIndex(&row, at.min(into.numberOfItems()));
}

/// A `KeyBinding` chord as an `NSMenuItem` key equivalent. `None` for anything gpui itself would
/// reject, so a typo is a row with no equivalent rather than a row answering the wrong chord.
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
    /// The target every app-provided row points at. `NSMenuItem::setTarget:` is weak, so this is
    /// held for the life of the process in [`SHARED`].
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGMenuBarSink"]
    #[ivars = SinkIvars]
    struct CommandSink;

    impl CommandSink {
        #[unsafe(method(ngMenuCommand:))]
        fn ng_menu_command(&self, sender: *mut NSMenuItem) {
            // SAFETY: AppKit only ever sends an action the item it fired for.
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
            self.ivars()
                .cx
                .spawn(async move |cx: &mut AsyncApp| {
                    let _ = cx.update(|cx| cx.dispatch_action(&*action));
                })
                .detach();
        }

        /// Without this the rows validate against a responder chain that does not implement
        /// `ngMenuCommand:` at all, and AppKit greys every one of them out.
        #[unsafe(method(validateMenuItem:))]
        fn validate(&self, _: *mut AnyObject) -> bool {
            true
        }
    }
);

thread_local! {
    static SHARED: RefCell<Option<Retained<CommandSink>>> = const { RefCell::new(None) };
    /// The `representedObject` number on an app row is an index here. Grows only; a bar is built
    /// once, and an index that has been handed out must stay valid.
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
