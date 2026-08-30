//! The menu bar, which is where macOS keeps every ⌘-key equivalent.

use std::cell::Cell;

use gpui::{App, KeyBinding, Menu, MenuItem, OsAction};
use objc2::{ClassType, sel};
use objc2_app_kit::{NSApplication, NSWindow};
use objc2_foundation::{MainThreadMarker, NSProcessInfo};

/// One unit struct per menu item, bound in [`bind_keys`] purely so [`App::set_menus`] has a
/// keystroke to derive that item's key equivalent from.
///
/// `no_register` keeps them out of the consuming app's action registry: nothing ever looks them up
/// by name, and a registered name that collides with another crate's panics at `App` creation.
macro_rules! menu_actions {
    ($($name:ident),* $(,)?) => {$(
        #[derive(Clone, PartialEq, gpui::Action)]
        #[action(namespace = native_gpui, no_register)]
        struct $name;
    )*};
}

menu_actions!(Undo, Redo, Cut, Copy, Paste, SelectAll, Hide, Quit);

const EDIT: &str = "Edit";
const UNDO: &str = "Undo";
const REDO: &str = "Redo";

/// The standard macOS Edit menu, needed before any ⌘-key reaches a hosted control.
///
/// Every item's key equivalent is read out of the keymap when the menu is *installed*, not from
/// anything named here — see [`install_menus`], which is what makes these work.
pub fn edit_menu() -> Menu {
    Menu {
        name: EDIT.into(),
        items: vec![
            MenuItem::os_action(UNDO, Undo, OsAction::Undo),
            MenuItem::os_action(REDO, Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::separator(),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
        ],
    }
}

/// Installs [`edit_menu`] plus the keymap bindings its key equivalents are derived from.
///
/// Call it once from inside `Application::run`. A GPUI app has no menu bar until something calls
/// this, and no menu bar means no Edit menu: ⌘C on a focused hosted field is offered to the view
/// hierarchy, declined, offered to a menu bar that is not there, and dropped. GPUI is not
/// swallowing it — there is nowhere for it to land.
///
/// It replaces the whole bar. An app with its own should call this first, for the bindings and the
/// app-menu keys, then set its own bar including [`edit_menu`] — Undo and Redo then come back as
/// gpui builds them, which is to say inert.
pub fn install_menus(cx: &mut App) {
    bind_keys(cx);
    let name = NSProcessInfo::processInfo().processName().to_string();
    cx.set_menus(vec![app_menu(&name), edit_menu()]);
    retarget_undo_redo();
}

/// What replacing the system's own menu bar would otherwise take away: an unbundled binary gets a
/// synthesised app menu with Hide and Quit in it, and `setMainMenu:` is all-or-nothing.
fn app_menu(name: &str) -> Menu {
    Menu {
        name: name.to_owned().into(),
        items: vec![
            MenuItem::action(format!("Hide {name}"), Hide),
            MenuItem::separator(),
            MenuItem::action(format!("Quit {name}"), Quit),
        ],
    }
}

/// Register before [`App::set_menus`], never after: `set_menus` walks the keymap once, and an
/// action with no binding yet produces an item with no key equivalent at all — a menu that looks
/// right, reads right in the UI, and answers no keystroke.
///
/// Cut, Copy, Paste, Select All, Undo and Redo are deliberately left with no listener. A listener
/// runs in the bubble phase, which stops propagation, which makes GPUI's `performKeyEquivalent:`
/// report the chord handled and starves the menu that was the whole point. Hide and Quit are the
/// opposite: their items carry gpui's own handler selector, so they do nothing until an action
/// listener exists.
fn bind_keys(cx: &mut App) {
    thread_local! {
        static BOUND: Cell<bool> = const { Cell::new(false) };
    }
    if BOUND.replace(true) {
        return;
    }

    cx.bind_keys([
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("cmd-shift-z", Redo, None),
        KeyBinding::new("cmd-x", Cut, None),
        KeyBinding::new("cmd-c", Copy, None),
        KeyBinding::new("cmd-v", Paste, None),
        KeyBinding::new("cmd-a", SelectAll, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
    cx.on_action(|_: &Hide, _| {
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).hide(None);
        }
    });
    cx.on_action(|_: &Quit, cx| cx.quit());
}

/// Point Undo and Redo at the real selectors.
///
/// gpui builds these two items with its own handler selector rather than `undo:`/`redo:`, on the
/// stated assumption that there is no `NSTextView` to enable them on. A hosted field has one: its
/// first responder while focused is the window's field editor, which is an `NSTextView`.
///
/// `undo:` is implemented by neither that field editor, nor `NSResponder`, nor `NSUndoManager` —
/// it is `NSWindow` that implements it and forwards to the undo manager it gets from its first
/// responder. So a nil target plus the real selector walks the chain past the field editor to the
/// window and lands on the focused field's own undo stack, leaving gpui's app delegate — which
/// implements `undo:` too — behind the window as the last resort it already was.
fn retarget_undo_redo() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // The one part of the chain above that is not in a header. Without it these items would point
    // at a selector nothing implements, which is worse than gpui's inert handler.
    let window = NSWindow::class();
    if window.instance_method(sel!(undo:)).is_none()
        || window.instance_method(sel!(redo:)).is_none()
    {
        return;
    }

    let app = NSApplication::sharedApplication(mtm);
    let Some(bar) = app.mainMenu() else {
        return;
    };
    let Some(edit) = bar
        .itemArray()
        .iter()
        .find(|item| item.title().to_string() == EDIT)
        .and_then(|item| item.submenu())
    else {
        return;
    };

    for item in edit.itemArray().iter() {
        let selector = match item.title().to_string().as_str() {
            UNDO => sel!(undo:),
            REDO => sel!(redo:),
            _ => continue,
        };
        // SAFETY: both selectors take a sender and return void, and a nil target is what asks
        // AppKit to find one in the responder chain.
        unsafe {
            item.setTarget(None);
            item.setAction(Some(selector));
        }
    }
}
