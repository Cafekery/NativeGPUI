//! The standard menu bar, read back out of AppKit. Exits with the number of failed rows.

use std::env;
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

const APP: &str = "{}";

/// title, key equivalent, modifiers, selector; a separator is `-`, a submenu host `(submenu)`.
type Row = (&'static str, &'static str, &'static str, &'static str);

/// `exact`: every row is this crate's. Elsewhere AppKit keeps a tail, so only the prefix counts.
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
            // macOS swaps the ⌃⌘F asked for to Globe-F, keeping ⌃⌘F on an alternate.
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

#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = ng_probe)]
struct FromKeymap;

#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = ng_probe)]
struct FromMenu;

fn report(label: &str, name: &str, detail: &str) {
    ROWS.fetch_add(1, Ordering::Relaxed);
    if label != "pass" {
        FAIL.fetch_add(1, Ordering::Relaxed);
    }
    println!("{label:<8} {name:<40} {detail}");
}

fn verdict(name: &str, ok: bool, detail: &str) {
    report(if ok { "pass" } else { "FAIL" }, name, detail);
}

fn blocked(name: &str, why: &str) {
    report("BLOCKED", name, why);
}

fn effect(name: &str, after: bool, fired: bool) {
    verdict(
        name,
        after && fired,
        &format!("pre[false] post[{after}] want[true] performKeyEquivalent[{fired}]"),
    );
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
        return "-|||".into();
    }
    format!(
        "{}|{}|{}|{}",
        item.title(),
        item.keyEquivalent(),
        mods_string(item.keyEquivalentModifierMask()),
        selector_name(item)
    )
}

fn expected_signature(row: &Row, name: &str) -> String {
    format!("{}|{}|{}|{}", row.0.replace(APP, name), row.1, row.2, row.3)
}

/// Walked, not `targetForAction:to:from:`, which starts at the key window a run may lack.
fn responder_for(window: &NSWindow, selector: Sel) -> Option<String> {
    let mut next = window.firstResponder();
    while let Some(responder) = next {
        if responder.respondsToSelector(selector) {
            return Some(class_name(&responder));
        }
        next = unsafe { responder.nextResponder() };
    }
    if let Some(controller) = window.windowController() {
        if controller.respondsToSelector(selector) {
            return Some(class_name(&controller));
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

fn cmd_chord(window: &NSWindow, key: &str, key_code: u16) -> Option<Retained<NSEvent>> {
    let text = NSString::from_str(key);
    NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        NSEventType::KeyDown,
        NSPoint::ZERO,
        CMD,
        NSProcessInfo::processInfo().systemUptime(),
        window.windowNumber(),
        None,
        &text,
        &text,
        false,
        key_code,
    )
}

fn ns_window(cx: &mut App, handle: &WindowHandle<Probe>) -> Option<Retained<NSWindow>> {
    handle
        .update(cx, |_, window, _| {
            let handle = HasWindowHandle::window_handle(window).ok()?;
            let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
                return None;
            };
            let view = unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }?;
            view.window()
        })
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
    // Validates `isEnabled`; unvalidated, every row reports the YES it was born with.
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

fn printed_menu(bar: &NSMenu, title: &str, prefix: &str) -> Option<Vec<Retained<NSMenuItem>>> {
    let Some(menu) = submenu(bar, title) else {
        verdict(&format!("{prefix}{title}.present"), false, "no such menu");
        return None;
    };
    let items = rows_of(&menu);
    for item in &items {
        print_row(title, item);
    }
    Some(items)
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
        let Some(items) = printed_menu(bar, &menu_title, "") else {
            continue;
        };
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
            let shadow: Vec<String> = items
                .iter()
                .skip(section.rows.len())
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

/// By identity: AppKit's tail appears only once there is something to put in it.
fn registered_menus(bar: &NSMenu, name: &str) {
    let app = NSApplication::sharedApplication(bar.mtm());
    for (label, ours, registered) in [
        ("windowsMenu", submenu(bar, "Window"), app.windowsMenu()),
        ("helpMenu", submenu(bar, "Help"), app.helpMenu()),
        (
            "servicesMenu",
            submenu(bar, name).and_then(|app_menu| submenu(&app_menu, "Services")),
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

fn controls(bar: &NSMenu) {
    let Some(file) = submenu(bar, "File") else {
        verdict("control.file_menu", false, "no File menu to plant in");
        return;
    };
    let mtm = bar.mtm();
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
    for (name, item) in [
        ("control.nil_action_disabled", &nil_action),
        ("control.unimplemented_disabled", &unimplemented),
    ] {
        let enabled = item.isEnabled();
        verdict(
            name,
            !enabled,
            &format!("isEnabled[{enabled}], must be false"),
        );
    }
    file.removeItem(&nil_action);
    file.removeItem(&unimplemented);
}

/// `performKeyEquivalent:` declines a disabled row, so a fire with no effect reads against this.
fn enabled_for(menu: &NSMenu, key: &str) -> Option<bool> {
    menu.update();
    menu.itemArray()
        .iter()
        .find(|item| {
            item.keyEquivalent().to_string() == key && item.keyEquivalentModifierMask() == CMD
        })
        .map(|item| item.isEnabled())
}

/// Tag 0 on every row: gpui's delegate re-locks a mutex it holds on any other tag and freezes.
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

/// `hide:`, `performMiniaturize:` and `deminiaturize` land a turn later, so each row settles.
async fn fire(cx: &mut AsyncApp, bar: &NSMenu, window: &NSWindow, name: &str, resolvable: bool) {
    let app = NSApplication::sharedApplication(window.mtm());

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

    // ⌘M, ⌘W, ⌘H in that order: ⌘W destroys the window ⌘M needs, and `hide:` takes the key window.
    if let Some(event) = cmd_chord(window, "m", KEY_M) {
        let enabled = enabled_for(&window_menu, "m");
        if window.isMiniaturized() {
            blocked("fire.cmd_m_miniaturizes", "window was already miniaturised");
        } else {
            let fired = window_menu.performKeyEquivalent(&event);
            settle(cx).await;
            let after = window.isMiniaturized();
            println!("         (Minimize row enabled at fire time: {enabled:?})");
            effect("fire.cmd_m_miniaturizes", after, fired);
        }
        window.deminiaturize(None);
        // The deminiaturise has to land before ⌘W, or ⌘W reads an invisible window.
        settle(cx).await;
        settle(cx).await;
    }

    if let Some(event) = cmd_chord(window, "w", KEY_W) {
        let enabled = enabled_for(&file, "w");
        println!("         (Close row enabled at fire time: {enabled:?})");
        if !window.isVisible() || window.isMiniaturized() {
            blocked(
                "fire.cmd_w_closes_window",
                "window was not on screen before the chord, so closing it proves nothing",
            );
        } else {
            let fired = file.performKeyEquivalent(&event);
            settle(cx).await;
            effect("fire.cmd_w_closes_window", !window.isVisible(), fired);
        }
    }

    if let Some(event) = cmd_chord(window, "h", KEY_H) {
        let enabled = enabled_for(&app_menu, "h");
        println!("         (Hide row enabled at fire time: {enabled:?})");
        if app.isHidden() {
            blocked("fire.cmd_h_hides_app", "app was already hidden");
        } else {
            let fired = app_menu.performKeyEquivalent(&event);
            settle(cx).await;
            effect("fire.cmd_h_hides_app", app.isHidden(), fired);
        }
        app.unhide(None);
        settle(cx).await;
    }
}

/// A subsequence match: an inserted row is fine, a dropped or reordered one is not.
fn extended(bar: &NSMenu, name: &str) {
    let read = menu_titles(bar);
    let want = [name, "File", "Edit", "View", "Probe", "Window", "Help"];
    verdict(
        "extend.bar_menus",
        read == want,
        &format!("read{read:?} want{want:?}"),
    );

    print_header();
    for section in EXPECTED {
        let menu_title = section.menu.replace(APP, name);
        let Some(items) = printed_menu(bar, &menu_title, "extend.") else {
            continue;
        };
        let mut read = items.iter().map(|item| signature(item));
        let missing: Vec<String> = section
            .rows
            .iter()
            .map(|row| expected_signature(row, name))
            .filter(|want| !read.any(|row| row == *want))
            .collect();
        verdict(
            &format!("extend.{menu_title}.standard_rows_intact"),
            missing.is_empty(),
            &format!("{} standard rows, missing{missing:?}", section.rows.len()),
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
    let items = rows_of(&probe);
    for item in &items {
        print_row("Probe", item);
    }
    let rows: Vec<String> = items.iter().map(|item| signature(item)).collect();
    verdict(
        "extend.probe_rows",
        rows == [
            "Neutral|j|cmd-alt-ctrl-shift|ngMenuCommand:",
            "-|||",
            "Also Neutral|||ngMenuCommand:",
        ],
        &format!("{rows:?}"),
    );
    let enabled = items.first().map(|item| item.isEnabled());
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

fn main() {
    let arm = env::var("NG_ARM").unwrap_or_else(|_| "table".into());
    gpui_platform::application().run(move |cx: &mut App| {
        if arm != "noquit" && env::var("NG_MENUS").as_deref() != Ok("0") {
            install_menus(cx);
        }
        // Without the stop the menu answers the chord too, and both order arms read the same.
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
                    // Keyed within the app, not activated: what a window action resolves against.
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
    let name = expected.as_deref().unwrap_or(&process);

    let Some(window) = cx.update(|cx| ns_window(cx, handle)) else {
        verdict("window", false, "no NSWindow behind the gpui window");
        return;
    };
    // Key within the app, without `NSApp.activate`: window actions resolve against the key window.
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
        // A bundled app always has a main menu, so what must be absent is the standard set.
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
        if let Some(event) = cmd_chord(&window, "q", KEY_Q) {
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
        // Count menus too: a bundle's system menu bears the app's name even without install_menus.
        let titles = menu_titles(&bar);
        let first = titles.first().map_or("", String::as_str);
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
            let Some(app_menu) = submenu(&bar, name) else {
                verdict("quit.app_menu", false, "no app menu to fire ⌘Q from");
                return;
            };
            let Some(event) = cmd_chord(&window, "q", KEY_Q) else {
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
        "extend" => extended(&bar, name),
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
            read_table(&bar, name);
            registered_menus(&bar, name);
            tags(&bar, name);
            controls(&bar);
            fire(cx, &bar, &window, name, resolvable).await;
        }
    }
}

/// Through `NSApp.sendEvent:`, so AppKit, not this probe, orders the layers.
async fn order(cx: &mut AsyncApp, window: &NSWindow, bound: bool) {
    let app = NSApplication::sharedApplication(window.mtm());
    let label = if bound { "bound" } else { "unbound" };
    let Some(event) = cmd_chord(window, "m", KEY_M) else {
        verdict(&format!("order.{label}.event"), false, "no event");
        return;
    };

    // `deminiaturize` lands a turn later; at a miniaturised window the chord routes nowhere.
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
            gpui == 0 && miniaturized
        },
        &format!(
            "pre[false] miniaturized[{miniaturized}] gpui_actions[{gpui}] — {}",
            if bound {
                "a chord gpui binds must reach gpui and leave the menu item unfired"
            } else {
                "an unbound chord must fall through to the menu"
            }
        ),
    );
    verdict(
        &format!("order.{label}.exclusive"),
        !(gpui > 0 && miniaturized),
        &format!("gpui_actions[{gpui}] miniaturized[{miniaturized}] — never both layers"),
    );
}
