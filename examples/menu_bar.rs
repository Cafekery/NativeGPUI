//! The standard menu bar, read back out of AppKit. Exits with the number of failed rows.
//!
//! ```text
//! NG_ARM=table   default; compares every row against the table below, then fires ⌘H, ⌘M and ⌘W
//! NG_ARM=extend  adds an app menu and an app row; every standard row must survive
//! NG_ARM=order   which layer answers ⌘M, with and without a gpui binding for it
//! NG_ARM=quit    fires ⌘Q through the menu; the pass is this process being gone
//! NG_ARM=noquit  the control for `quit`: the same fire with no bar installed, must stay alive
//! NG_ARM=hold    installs the bar and waits NG_HOLD seconds (default 12) for an external reader
//! NG_MENUS=0     skips install_menus entirely, in any arm
//! ```

use std::env;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Bounds, Context, KeyBinding, Keystroke, Render, Window,
    WindowBounds, WindowHandle, div, point, prelude::*, px, rgb, size,
};
use native_gpui::{MenuCommand, MenuSection, add_menu, extend_menu, install_menus, window_options};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, Sel};
use objc2::{MainThreadOnly, sel};
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem, NSView, NSWindow,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSProcessInfo, NSString};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Stands in for the app's own name in the table below.
const APP: &str = "{}";

/// title, key equivalent, modifier chord, selector. A separator is a title of `-`; a submenu host
/// reports `(submenu)`.
type Row = (&'static str, &'static str, &'static str, &'static str);

/// `exact` is true only where every row is this crate's: AppKit maintains a tail of its own on
/// the rest, so there only the standard prefix is compared.
struct Expect {
    menu: &'static str,
    exact: bool,
    rows: &'static [Row],
}

const EXPECTED: &[Expect] = &[
    Expect {
        menu: APP,
        exact: true,
        rows: &[
            ("About {}", "", "", "orderFrontStandardAboutPanel:"),
            ("-", "", "", ""),
            ("-", "", "", ""),
            ("Services", "", "", "(submenu) submenuAction:"),
            ("-", "", "", ""),
            ("Hide {}", "h", "cmd", "hide:"),
            ("Hide Others", "h", "cmd-alt", "hideOtherApplications:"),
            ("Show All", "", "", "unhideAllApplications:"),
            ("-", "", "", ""),
            ("Quit {}", "q", "cmd", "terminate:"),
        ],
    },
    Expect {
        menu: "File",
        exact: false,
        rows: &[("Close", "w", "cmd", "performClose:")],
    },
    Expect {
        menu: "Edit",
        exact: false,
        rows: &[
            ("Undo", "z", "cmd", "undo:"),
            ("Redo", "z", "cmd-shift", "redo:"),
            ("-", "", "", ""),
            ("Cut", "x", "cmd", "cut:"),
            ("Copy", "c", "cmd", "copy:"),
            ("Paste", "v", "cmd", "paste:"),
            (
                "Paste and Match Style",
                "v",
                "cmd-alt-shift",
                "pasteAsPlainText:",
            ),
            ("Delete", "", "", "delete:"),
            ("Select All", "a", "cmd", "selectAll:"),
        ],
    },
    Expect {
        menu: "View",
        exact: false,
        rows: &[
            ("Show Toolbar", "t", "cmd-alt", "toggleToolbarShown:"),
            (
                "Customize Toolbar…",
                "",
                "",
                "runToolbarCustomizationPalette:",
            ),
            ("-", "", "", ""),
            ("Show Sidebar", "s", "cmd-ctrl", "toggleSidebar:"),
            ("-", "", "", ""),
            // macOS substitutes Globe-F for the ⌃⌘F this crate asks for, keeping ⌃⌘F on an
            // alternate of its own.
            ("Enter Full Screen", "f", "fn", "toggleFullScreen:"),
        ],
    },
    Expect {
        menu: "Window",
        exact: false,
        rows: &[
            ("Minimize", "m", "cmd", "performMiniaturize:"),
            ("Zoom", "", "", "performZoom:"),
            ("-", "", "", ""),
            ("Bring All to Front", "", "", "arrangeInFront:"),
        ],
    },
    Expect {
        menu: "Help",
        exact: false,
        rows: &[("{} Help", "?", "cmd", "showHelp:")],
    },
];

const CMD: NSEventModifierFlags = NSEventModifierFlags::Command;
/// Physical US-layout key codes: gpui names a key by asking the layout what the code produces.
const KEY_M: u16 = 46;
const KEY_H: u16 = 4;
const KEY_W: u16 = 13;
const KEY_Q: u16 = 12;

static FAIL: AtomicU32 = AtomicU32::new(0);
static ROWS: AtomicU32 = AtomicU32::new(0);
static GPUI_CLAIMED: AtomicU32 = AtomicU32::new(0);
static MENU_CLAIMED: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = ng_probe)]
struct FromKeymap;

#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = ng_probe)]
struct FromMenu;

fn verdict(name: &str, ok: bool, detail: &str) {
    ROWS.fetch_add(1, Ordering::Relaxed);
    if !ok {
        FAIL.fetch_add(1, Ordering::Relaxed);
    }
    println!(
        "{:<8} {:<40} {}",
        if ok { "pass" } else { "FAIL" },
        name,
        detail
    );
}

fn blocked(name: &str, why: &str) {
    ROWS.fetch_add(1, Ordering::Relaxed);
    FAIL.fetch_add(1, Ordering::Relaxed);
    println!("{:<8} {:<40} {}", "BLOCKED", name, why);
}

fn effect(name: &str, before: bool, after: bool, fired: bool) {
    ROWS.fetch_add(1, Ordering::Relaxed);
    let detail = format!("pre[{before}] post[{after}] want[true] performKeyEquivalent[{fired}]");
    let label = if before {
        FAIL.fetch_add(1, Ordering::Relaxed);
        "VACUOUS"
    } else if after && fired {
        "pass"
    } else {
        FAIL.fetch_add(1, Ordering::Relaxed);
        "FAIL"
    };
    println!("{label:<8} {name:<40} {detail}");
}

fn mods_string(mask: NSEventModifierFlags) -> String {
    let mut parts = Vec::new();
    for (flag, name) in [
        (NSEventModifierFlags::Command, "cmd"),
        (NSEventModifierFlags::Option, "alt"),
        (NSEventModifierFlags::Control, "ctrl"),
        (NSEventModifierFlags::Shift, "shift"),
        (NSEventModifierFlags::Function, "fn"),
    ] {
        if mask.contains(flag) {
            parts.push(name);
        }
    }
    parts.join("-")
}

/// SAFETY: a `Sel` from `NSMenuItem::action` or `sel!` is always a registered selector.
fn selector_string(selector: Sel) -> String {
    unsafe { NSString::from_selector(selector) }.to_string()
}

fn selector_name(item: &NSMenuItem) -> String {
    match item.action() {
        Some(selector) if item.submenu().is_some() => {
            format!("(submenu) {}", selector_string(selector))
        }
        Some(selector) => selector_string(selector),
        None if item.submenu().is_some() => "(submenu)".into(),
        None => "(nil)".into(),
    }
}

fn class_name(object: &AnyObject) -> String {
    object.class().name().to_string_lossy().into_owned()
}

fn signature(item: &NSMenuItem) -> String {
    // `separatorItem` reports a Command mask whether or not anything asked for one.
    if item.isSeparatorItem() {
        return "-|||".to_string();
    }
    let title = item.title().to_string();
    format!(
        "{title}|{}|{}|{}",
        item.keyEquivalent(),
        mods_string(item.keyEquivalentModifierMask()),
        selector_name(item)
    )
}

fn expected_signature(row: &Row, name: &str) -> String {
    format!("{}|{}|{}|{}", row.0.replace(APP, name), row.1, row.2, row.3)
}

/// Walked rather than asked of `targetForAction:to:from:`, which searches from the key window and
/// so reports every window action as unreachable on a run without one.
fn responder_for(window: &NSWindow, selector: Sel) -> Option<String> {
    let mut next = window.firstResponder();
    while let Some(responder) = next {
        if responder.respondsToSelector(selector) {
            let object: &AnyObject = &responder;
            return Some(class_name(object));
        }
        next = unsafe { responder.nextResponder() };
    }
    if let Some(controller) = window.windowController() {
        if controller.respondsToSelector(selector) {
            let object: &AnyObject = &controller;
            return Some(class_name(object));
        }
    }
    let app = NSApplication::sharedApplication(window.mtm());
    if app.respondsToSelector(selector) {
        return Some("NSApplication".into());
    }
    if let Some(delegate) = app.delegate() {
        if delegate.respondsToSelector(selector) {
            return Some(class_name(delegate.as_ref()));
        }
    }
    None
}

fn chord(
    window: &NSWindow,
    key: &str,
    key_code: u16,
    mask: NSEventModifierFlags,
) -> Option<Retained<NSEvent>> {
    let text = NSString::from_str(key);
    NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        NSEventType::KeyDown,
        NSPoint::ZERO,
        mask,
        NSProcessInfo::processInfo().systemUptime(),
        window.windowNumber(),
        None,
        &text,
        &text,
        false,
        key_code,
    )
}

fn gpui_ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    let view = unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }?;
    view.window()
}

fn ns_window(cx: &mut App, handle: &WindowHandle<Probe>) -> Option<Retained<NSWindow>> {
    handle
        .update(cx, |_, window, _| gpui_ns_window(window))
        .ok()
        .flatten()
}

fn submenu(bar: &NSMenu, title: &str) -> Option<Retained<NSMenu>> {
    bar.itemArray()
        .iter()
        .find(|item| item.title().to_string() == title)
        .and_then(|item| item.submenu())
}

fn menu_titles(bar: &NSMenu) -> Vec<String> {
    bar.itemArray()
        .iter()
        .map(|item| item.title().to_string())
        .collect()
}

fn rows_of(menu: &NSMenu) -> Vec<Retained<NSMenuItem>> {
    // Runs the validation that decides `isEnabled`; without it every row reports the YES an
    // `NSMenuItem` is born with.
    menu.update();
    menu.itemArray().iter().collect()
}

fn print_header() {
    println!();
    println!(
        "{:<10} {:<24} {:<4} {:<14} {:<32} {:<18} ENABLED",
        "MENU", "TITLE", "KEY", "MODIFIERS", "SELECTOR", "TARGET"
    );
}

fn print_row(menu: &str, item: &NSMenuItem) {
    let title = if item.isSeparatorItem() {
        "-".to_string()
    } else {
        item.title().to_string()
    };
    println!(
        "{:<10} {:<24} {:<4} {:<14} {:<32} {:<18} {}",
        menu,
        title,
        item.keyEquivalent(),
        mods_string(item.keyEquivalentModifierMask()),
        selector_name(item),
        match item.target() {
            Some(target) => class_name(&target),
            None => "(nil)".into(),
        },
        item.isEnabled()
    );
}

fn read_table(bar: &NSMenu, name: &str) {
    let want: Vec<String> = EXPECTED
        .iter()
        .map(|section| section.menu.replace(APP, name))
        .collect();
    let read = menu_titles(bar);
    verdict(
        "bar.menus",
        read == want,
        &format!("read{read:?} want{want:?}"),
    );

    print_header();
    for section in EXPECTED {
        let menu_title = section.menu.replace(APP, name);
        let Some(menu) = submenu(bar, &menu_title) else {
            verdict(&format!("{menu_title}.present"), false, "no such menu");
            continue;
        };
        let items = rows_of(&menu);
        for item in &items {
            print_row(&menu_title, item);
        }
        for (index, row) in section.rows.iter().enumerate() {
            let want = expected_signature(row, name);
            let read = items
                .get(index)
                .map(|item| signature(item))
                .unwrap_or_else(|| "(missing)".into());
            if read != want {
                verdict(
                    &format!("{menu_title}[{index}]"),
                    false,
                    &format!("read[{read}] want[{want}]"),
                );
            } else {
                ROWS.fetch_add(1, Ordering::Relaxed);
            }
        }
        let extra = items.len().saturating_sub(section.rows.len());
        if section.exact {
            verdict(
                &format!("{menu_title}.no_extra_rows"),
                extra == 0,
                &format!("{} rows, want {}", items.len(), section.rows.len()),
            );
        } else {
            let shadow: Vec<String> = items[section.rows.len().min(items.len())..]
                .iter()
                .filter(|item| {
                    item.keyEquivalentModifierMask() == CMD
                        && matches!(
                            item.keyEquivalent().to_string().as_str(),
                            "w" | "q" | "m" | "h"
                        )
                })
                .map(|item| signature(item))
                .collect();
            verdict(
                &format!("{menu_title}.tail_shadows_nothing"),
                shadow.is_empty(),
                &format!("{extra} rows AppKit added of its own, shadowing{shadow:?}"),
            );
        }
    }
    println!();
}

/// By object identity rather than by looking for the tail AppKit maintains: that tail only appears
/// when the app has something to put in one, so its absence says nothing.
fn registered_menus(bar: &NSMenu, name: &str) {
    let mtm = bar.mtm();
    let app = NSApplication::sharedApplication(mtm);
    for (label, ours, registered) in [
        ("windowsMenu", submenu(bar, "Window"), app.windowsMenu()),
        ("helpMenu", submenu(bar, "Help"), app.helpMenu()),
        (
            "servicesMenu",
            submenu(bar, name).and_then(|app_menu| {
                app_menu
                    .itemArray()
                    .iter()
                    .find(|item| item.title().to_string() == "Services")
                    .and_then(|item| item.submenu())
            }),
            app.servicesMenu(),
        ),
    ] {
        let same = match (ours.as_deref(), registered.as_deref()) {
            (Some(ours), Some(registered)) => std::ptr::eq(ours, registered),
            _ => false,
        };
        verdict(
            &format!("registered.{label}"),
            same,
            &format!(
                "ours[{}] registered[{}]",
                ours.is_some(),
                registered.is_some()
            ),
        );
    }
}

/// Whether the ENABLED column can report a false at all.
fn controls(mtm: MainThreadMarker, bar: &NSMenu) {
    let Some(file) = submenu(bar, "File") else {
        verdict("control.file_menu", false, "no File menu to plant in");
        return;
    };
    let nil_action = NSMenuItem::new(mtm);
    nil_action.setTitle(&NSString::from_str("NG control: nil action"));
    let unimplemented = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("NG control: unimplemented selector"),
            Some(sel!(ngNoSuchActionAnywhere:)),
            &NSString::from_str(""),
        )
    };
    file.addItem(&nil_action);
    file.addItem(&unimplemented);
    file.update();
    verdict(
        "control.nil_action_disabled",
        !nil_action.isEnabled(),
        &format!("isEnabled[{}], must be false", nil_action.isEnabled()),
    );
    verdict(
        "control.unimplemented_disabled",
        !unimplemented.isEnabled(),
        &format!("isEnabled[{}], must be false", unimplemented.isEnabled()),
    );
    file.removeItem(&nil_action);
    file.removeItem(&unimplemented);
}

/// `performKeyEquivalent:` declines a disabled row, so a fire with no effect reads against this.
fn enabled_for(menu: &NSMenu, key: &str, mask: NSEventModifierFlags) -> Option<bool> {
    menu.update();
    menu.itemArray()
        .iter()
        .find(|item| {
            item.keyEquivalent().to_string() == key && item.keyEquivalentModifierMask() == mask
        })
        .map(|item| item.isEnabled())
}

/// Every row this crate installed carries tag 0: gpui's app delegate looks a sender's tag up in a
/// one-entry table and, on a miss, re-locks a mutex it still holds and freezes the process.
fn tags(bar: &NSMenu, name: &str) {
    let mut offenders = Vec::new();
    let mut checked = 0;
    for section in EXPECTED {
        let menu_title = section.menu.replace(APP, name);
        let Some(menu) = submenu(bar, &menu_title) else {
            continue;
        };
        for item in menu.itemArray().iter().take(section.rows.len()) {
            checked += 1;
            if item.tag() != 0 {
                offenders.push(format!("{menu_title}/{} tag {}", item.title(), item.tag()));
            }
        }
    }
    verdict(
        "tags.all_zero",
        offenders.is_empty() && checked > 0,
        &format!("{checked} rows checked, non-zero{offenders:?}"),
    );
}

/// Each of ⌘H, ⌘M, ⌘W and ⌘Q: the responder its selector reaches, and for the first three the
/// state the chord is supposed to move. `hide:`, `performMiniaturize:` and `deminiaturize` all
/// complete on a later run-loop turn, so every row settles before reading.
async fn fire(cx: &mut AsyncApp, bar: &NSMenu, window: &NSWindow, name: &str, resolvable: bool) {
    let mtm = window.mtm();
    let app = NSApplication::sharedApplication(mtm);

    for (label, selector) in [
        ("cmd_h", sel!(hide:)),
        ("cmd_q", sel!(terminate:)),
        ("cmd_w", sel!(performClose:)),
        ("cmd_m", sel!(performMiniaturize:)),
    ] {
        let found = responder_for(window, selector);
        verdict(
            &format!("responder.{label}"),
            found.is_some(),
            &format!(
                "{} answered by {}",
                selector_string(selector),
                found.as_deref().unwrap_or("NOTHING IN THE CHAIN")
            ),
        );
    }

    if !resolvable {
        blocked(
            "fire",
            "skipped: no key or main window, so a fired row could not have worked and a red here \
             would say nothing about the menu",
        );
        return;
    }

    let (Some(app_menu), Some(file), Some(window_menu)) = (
        submenu(bar, name),
        submenu(bar, "File"),
        submenu(bar, "Window"),
    ) else {
        verdict("fire.menus", false, "a menu the fire rows need is missing");
        return;
    };

    // ⌘M first, then ⌘W, then ⌘H: ⌘W destroys the window ⌘M needs, and `hide:` takes the key
    // window with it.
    if let Some(event) = chord(window, "m", KEY_M, CMD) {
        let enabled = enabled_for(&window_menu, "m", CMD);
        if window.isMiniaturized() {
            blocked("fire.cmd_m_miniaturizes", "window was already miniaturised");
        } else {
            let fired = window_menu.performKeyEquivalent(&event);
            settle(cx).await;
            let after = window.isMiniaturized();
            println!("         (Minimize row enabled at fire time: {enabled:?})");
            effect("fire.cmd_m_miniaturizes", false, after, fired);
        }
        window.deminiaturize(None);
        // The deminiaturise has to land before ⌘W, or ⌘W reads an invisible window.
        settle(cx).await;
        settle(cx).await;
    }

    if let Some(event) = chord(window, "w", KEY_W, CMD) {
        let enabled = enabled_for(&file, "w", CMD);
        println!("         (Close row enabled at fire time: {enabled:?})");
        if !window.isVisible() || window.isMiniaturized() {
            blocked(
                "fire.cmd_w_closes_window",
                "window was not on screen before the chord, so closing it proves nothing",
            );
        } else {
            let fired = file.performKeyEquivalent(&event);
            settle(cx).await;
            effect(
                "fire.cmd_w_closes_window",
                false,
                !window.isVisible(),
                fired,
            );
        }
    }

    if let Some(event) = chord(window, "h", KEY_H, CMD) {
        let enabled = enabled_for(&app_menu, "h", CMD);
        println!("         (Hide row enabled at fire time: {enabled:?})");
        if app.isHidden() {
            blocked("fire.cmd_h_hides_app", "app was already hidden");
        } else {
            let fired = app_menu.performKeyEquivalent(&event);
            settle(cx).await;
            effect("fire.cmd_h_hides_app", false, app.isHidden(), fired);
        }
        app.unhide(None);
        settle(cx).await;
    }
}

/// An app menu and an app row alongside the standard set. A subsequence match, so an inserted row
/// is fine and a dropped or reordered one is not.
fn extended(bar: &NSMenu, name: &str) {
    let read = menu_titles(bar);
    let want = vec![
        name.to_string(),
        "File".into(),
        "Edit".into(),
        "View".into(),
        "Probe".into(),
        "Window".into(),
        "Help".into(),
    ];
    verdict(
        "extend.bar_menus",
        read == want,
        &format!("read{read:?} want{want:?}"),
    );

    print_header();
    for section in EXPECTED {
        let menu_title = section.menu.replace(APP, name);
        let Some(menu) = submenu(bar, &menu_title) else {
            verdict(
                &format!("extend.{menu_title}.present"),
                false,
                "no such menu",
            );
            continue;
        };
        let items = rows_of(&menu);
        for item in &items {
            print_row(&menu_title, item);
        }
        let read: Vec<String> = items.iter().map(|item| signature(item)).collect();
        let want: Vec<String> = section
            .rows
            .iter()
            .map(|row| expected_signature(row, name))
            .collect();
        let mut cursor = read.iter();
        let missing: Vec<&String> = want
            .iter()
            .filter(|row| !cursor.any(|read| read == *row))
            .collect();
        verdict(
            &format!("extend.{menu_title}.standard_rows_intact"),
            missing.is_empty(),
            &format!("{} standard rows, missing{missing:?}", want.len()),
        );
    }

    let Some(app_menu) = submenu(bar, name) else {
        return;
    };
    let settings = rows_of(&app_menu).get(2).map(|item| signature(item));
    verdict(
        "extend.settings_row",
        settings.as_deref() == Some("Settings…|,|cmd|ngMenuCommand:"),
        &format!("app menu row 2 is {settings:?}"),
    );
    let Some(probe) = submenu(bar, "Probe") else {
        verdict("extend.probe_menu", false, "no Probe menu");
        return;
    };
    let rows: Vec<String> = rows_of(&probe).iter().map(|item| signature(item)).collect();
    for item in rows_of(&probe) {
        print_row("Probe", &item);
    }
    verdict(
        "extend.probe_rows",
        rows == vec![
            "Neutral|j|cmd-alt-ctrl-shift|ngMenuCommand:".to_string(),
            "-|||".to_string(),
            "Also Neutral|||ngMenuCommand:".to_string(),
        ],
        &format!("{rows:?}"),
    );
    let enabled = rows_of(&probe).first().map(|item| item.isEnabled());
    verdict(
        "extend.probe_row_enabled",
        enabled == Some(true),
        &format!("isEnabled{enabled:?}"),
    );
}

struct Probe;

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(rgb(0x1c1c1e))
    }
}

fn app_menus() {
    add_menu(
        "Probe",
        [
            MenuCommand::new("Neutral", FromMenu).key("cmd-alt-ctrl-shift-j"),
            MenuCommand::separator(),
            MenuCommand::new("Also Neutral", FromMenu),
        ],
    );
    extend_menu(
        MenuSection::App,
        [MenuCommand::new("Settings…", FromMenu).key("cmd-,")],
    );
}

fn main() -> ExitCode {
    let arm = env::var("NG_ARM").unwrap_or_else(|_| "table".into());
    gpui_platform::application().run(move |cx: &mut App| {
        if arm != "noquit" && env::var("NG_MENUS").as_deref() != Ok("0") {
            install_menus(cx);
        }
        cx.on_action(|_: &FromMenu, _| {
            MENU_CLAIMED.fetch_add(1, Ordering::Relaxed);
        });
        // Without the stop, gpui declines the chord and the menu answers it anyway, so both arms
        // of the ordering measurement would read the same.
        cx.on_action(|_: &FromKeymap, cx| {
            GPUI_CLAIMED.fetch_add(1, Ordering::Relaxed);
            cx.stop_propagation();
        });
        if arm == "extend" {
            app_menus();
            // The second call must be a no-op rather than a rebuild that wipes the app's own rows.
            install_menus(cx);
        }

        let handle = cx
            .open_window(
                gpui::WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(1400.), px(40.)),
                        size: size(px(200.), px(120.)),
                    })),
                    // `run` makes the window key inside this app without activating it, which is
                    // what a window action resolves against.
                    focus: false,
                    ..window_options()
                },
                |_, cx| cx.new(|_| Probe),
            )
            .unwrap();

        cx.spawn(async move |cx: &mut AsyncApp| {
            settle(cx).await;
            run(&arm, cx, &handle).await;
            println!();
            println!(
                "{} rows, {} failed",
                ROWS.load(Ordering::Relaxed),
                FAIL.load(Ordering::Relaxed)
            );
            std::process::exit(FAIL.load(Ordering::Relaxed).min(120) as i32);
        })
        .detach();
    });
    ExitCode::SUCCESS
}

async fn settle(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(140))
        .await;
}

async fn run(arm: &str, cx: &mut AsyncApp, handle: &WindowHandle<Probe>) {
    let Some(mtm) = MainThreadMarker::new() else {
        verdict("main_thread", false, "not on the main thread");
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let process = NSProcessInfo::processInfo().processName().to_string();
    // The bundled run sets `NG_EXPECT_NAME` to a name the executable does not have.
    let expected = env::var("NG_EXPECT_NAME").ok();
    let name = expected.clone().unwrap_or_else(|| process.clone());

    let window = cx.update(|cx| ns_window(cx, handle));
    let Some(window) = window else {
        verdict("window", false, "no NSWindow behind the gpui window");
        return;
    };
    // Key inside this app, without `NSApp.activate`: a window action resolves against the key
    // window.
    window.makeKeyAndOrderFront(None);
    settle(cx).await;

    println!(
        "app[{name}] active[{}] hidden[{}] windows[{}] keyWindow[{}] mainWindow[{}] \
         isKey[{}] isMain[{}]",
        app.isActive(),
        app.isHidden(),
        app.windows().len(),
        app.keyWindow().is_some(),
        app.mainWindow().is_some(),
        window.isKeyWindow(),
        window.isMainWindow(),
    );

    if arm == "noquit" {
        // A bundled app has a system main menu whether or not this crate installed one, so what
        // has to be absent is the standard set.
        let bar = app.mainMenu();
        let ours: Vec<&str> = ["File", "Edit", "View", "Window", "Help"]
            .into_iter()
            .filter(|title| bar.as_deref().and_then(|bar| submenu(bar, title)).is_some())
            .collect();
        verdict(
            "noquit.standard_set_absent",
            ours.is_empty(),
            &format!("mainMenu present[{}] ours{ours:?}", bar.is_some()),
        );
        if let Some(event) = chord(&window, "q", KEY_Q, CMD) {
            app.sendEvent(&event);
        }
        settle(cx).await;
        println!("noquit: fired ⌘Q with no menu bar; reaching this line is the pass");
        return;
    }

    let Some(bar) = app.mainMenu() else {
        if arm == "hold" {
            println!("hold: no menu bar installed — this is the arm an external reader must fail");
            for _ in 0..60 {
                settle(cx).await;
            }
            return;
        }
        verdict("bar", false, "install_menus left NSApp.mainMenu nil");
        return;
    };

    if let Some(expected) = expected.as_deref() {
        // The menu count is part of the condition: AppKit titles the first item of a bundled
        // app's system menu with the app's name whether or not `install_menus` ran.
        let titles = menu_titles(&bar);
        let first = titles.first().cloned().unwrap_or_default();
        verdict(
            "name.from_bundle_not_process",
            first == expected && expected != process && titles.len() >= EXPECTED.len(),
            &format!(
                "app menu titled[{first}] want[{expected}] processName[{process}] menus[{}]",
                titles.len()
            ),
        );
    }

    let resolvable = app.keyWindow().is_some() || app.mainWindow().is_some();
    if !resolvable {
        blocked(
            "key_window",
            "no key or main window, so performClose:/performMiniaturize: cannot resolve — \
             bundle the example and launch it with `open -n` before believing any window row",
        );
    }

    match arm {
        "quit" => {
            let Some(app_menu) = submenu(&bar, &name) else {
                verdict("quit.app_menu", false, "no app menu to fire ⌘Q from");
                return;
            };
            let Some(event) = chord(&window, "q", KEY_Q, CMD) else {
                return;
            };
            println!("quit: firing ⌘Q — the pass is this process printing nothing more");
            let fired = app_menu.performKeyEquivalent(&event);
            settle(cx).await;
            verdict(
                "quit.terminated",
                false,
                &format!("STILL ALIVE, performKeyEquivalent[{fired}]"),
            );
        }
        "order" => {
            order(cx, &window, false).await;
            let bound = cx.update(|cx| {
                cx.bind_keys([KeyBinding::new("cmd-m", FromKeymap, None)]);
                cx.all_bindings_for_input(&[Keystroke::parse("cmd-m").unwrap()])
                    .len()
            });
            verdict(
                "order.binding_registered",
                bound == 1,
                &format!("gpui bindings for cmd-m: {bound}"),
            );
            order(cx, &window, true).await;
        }
        "extend" => extended(&bar, &name),
        "hold" => {
            let seconds: u64 = env::var("NG_HOLD")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(12);
            println!("hold: bar installed, waiting {seconds}s for an external reader");
            for _ in 0..seconds * 5 {
                settle(cx).await;
            }
        }
        _ => {
            read_table(&bar, &name);
            registered_menus(&bar, &name);
            tags(&bar, &name);
            controls(mtm, &bar);
            fire(cx, &bar, &window, &name, resolvable).await;
        }
    }
}

/// Which layer claims ⌘M. The chord goes into `NSApp.sendEvent:` so AppKit sequences the layers
/// rather than this probe calling them in an order it chose.
async fn order(cx: &mut AsyncApp, window: &NSWindow, bound: bool) {
    let mtm = window.mtm();
    let app = NSApplication::sharedApplication(mtm);
    let label = if bound { "bound" } else { "unbound" };
    let Some(event) = chord(window, "m", KEY_M, CMD) else {
        verdict(&format!("order.{label}.event"), false, "no event");
        return;
    };

    // `deminiaturize` lands on a later turn, and fired at a miniaturised window the chord routes
    // nowhere and both arms read zero.
    window.deminiaturize(None);
    settle(cx).await;
    settle(cx).await;
    if window.isMiniaturized() {
        blocked(
            &format!("order.{label}"),
            "window still miniaturised, so the chord could not reach either layer",
        );
        return;
    }

    GPUI_CLAIMED.store(0, Ordering::Relaxed);
    let before = window.isMiniaturized();
    app.sendEvent(&event);
    settle(cx).await;
    let miniaturized = window.isMiniaturized();
    let gpui = GPUI_CLAIMED.load(Ordering::Relaxed);
    window.deminiaturize(None);

    verdict(
        &format!("order.{label}.claimed_by"),
        if bound {
            gpui == 1 && !miniaturized
        } else {
            gpui == 0 && miniaturized && !before
        },
        &format!(
            "pre[{before}] miniaturized[{miniaturized}] gpui_actions[{gpui}] — {}",
            if bound {
                "a chord gpui binds must reach gpui and leave the menu item unfired"
            } else {
                "an unbound chord must fall through to the menu"
            }
        ),
    );
    verdict(
        &format!("order.{label}.exclusive"),
        !(gpui > 0 && miniaturized && !before),
        &format!("gpui_actions[{gpui}] miniaturized[{miniaturized}] — never both layers"),
    );
}
