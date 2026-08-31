//! Measurement apparatus for `NSMenu` submenus and custom-view rows. `menu.rs` is the demo.
//!
//! Every reading is differential — the same menu with and without the row under test — because
//! nothing public reports an `NSMenuItem`'s own height.
//!
//! ```text
//! NG_ARM=metrics   default; headless, opens no window. Asserts the facts `menu.rs` records,
//!                  and that every SF Symbol name below resolves non-nil.
//! NG_ARM=open      opens a window, posts a right-click through this process's own NSApplication
//!                  and holds the resulting menu open so it can be captured.
//! NG_HOLD=n        seconds to hold the open menu (default 20)
//! ```

use std::env;
use std::ptr::NonNull;

use block2::RcBlock;
use gpui::{
    App, Application, Bounds, Context, Hsla, Modifiers, Render, Window, WindowBounds, div, point,
    prelude::*, px, rgb, size,
};
use native_gpui::{
    MenuEntry, MenuOrigin, MenuView, native_context_menu, native_popup, native_root, window_options,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadOnly, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSControlStateValueOff, NSControlStateValueOn,
    NSEvent, NSEventModifierFlags, NSEventType, NSImage, NSMenu, NSMenuItem, NSPopUpButton, NSView,
};
use objc2_foundation::{
    MainThreadMarker, NSObject, NSPoint, NSProcessInfo, NSRect, NSRunLoop, NSRunLoopCommonModes,
    NSSize, NSString, NSTimer,
};

/// Every SF Symbol name this probe puts in a menu. An unknown name resolves to no image rather
/// than a placeholder, so a typo degrades to a blank row that looks deliberate.
const SYMBOLS: &[&str] = &[
    "arrow.up.arrow.down",
    "slider.horizontal.3",
    "eye",
    "rectangle.3.group",
    "folder.badge.gearshape",
    "iphone",
];

/// Finder's seven tag colours, in Finder's order.
const TAGS: &[u32] = &[
    0xfc5b57, 0xf5a623, 0xf7ce46, 0x7bc862, 0x55a8f2, 0xc87fe0, 0xb8b8b8,
];

/// The shape of Finder's `…` menu: two submenu hosts, a strip of tag swatches on one row, a
/// checked row, a row that draws a chord, and a trailing row with no symbol at all.
fn more_entries() -> Vec<MenuEntry> {
    let colors: Vec<Hsla> = TAGS.iter().map(|hex| Hsla::from(rgb(*hex))).collect();
    vec![
        MenuEntry::item("Sort By")
            .symbol("arrow.up.arrow.down")
            .submenu([
                MenuEntry::item("Name"),
                MenuEntry::item("Kind"),
                MenuEntry::item("Date Last Opened"),
                MenuEntry::separator(),
                MenuEntry::item("Snap to Grid"),
            ]),
        MenuEntry::item("Show View Options")
            .symbol("slider.horizontal.3")
            .key("j", Modifiers::command()),
        MenuEntry::item("Show Preview Options").symbol("eye"),
        MenuEntry::separator(),
        MenuEntry::item("Use Groups")
            .symbol("rectangle.3.group")
            .checked(true),
        MenuEntry::separator(),
        MenuEntry::view(MenuView::swatches(colors)),
        MenuEntry::separator(),
        MenuEntry::item("Customize Folder…").symbol("folder.badge.gearshape"),
        MenuEntry::item("Import from iPhone")
            .symbol("iphone")
            .submenu([
                MenuEntry::item("Take Photo"),
                MenuEntry::item("Scan Documents"),
            ]),
        MenuEntry::separator(),
        MenuEntry::item("Upload to Limit"),
    ]
}

fn plain(mtm: MainThreadMarker, title: &str) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    item
}

fn three_rows(mtm: MainThreadMarker, extra: Option<&NSMenuItem>) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    for title in ["Sort By", "Use Groups", "Show View Options"] {
        menu.addItem(&plain(mtm, title));
    }
    if let Some(extra) = extra {
        menu.addItem(extra);
    }
    menu
}

fn view_item(mtm: MainThreadMarker, view: &NSView) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setView(Some(view));
    item
}

fn sized_view(mtm: MainThreadMarker, width: f64, height: f64) -> Retained<NSView> {
    NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::ZERO, NSSize::new(width, height)),
    )
}

fn check(label: &str, held: bool) -> bool {
    println!("  {}  {label}", if held { "ok  " } else { "FAIL" });
    held
}

fn metrics(mtm: MainThreadMarker) -> bool {
    let mut ok = true;

    let one = {
        let menu = NSMenu::new(mtm);
        menu.addItem(&plain(mtm, "Sort By"));
        menu.size()
    };
    let base = three_rows(mtm, None).size();

    let bare = view_item(mtm, &NSView::new(mtm));
    let zero = three_rows(mtm, Some(&bare)).size();

    let big = sized_view(mtm, 220., 26.);
    let sized = view_item(mtm, &big);
    let explicit = three_rows(mtm, Some(&sized)).size();

    let titled = view_item(mtm, &sized_view(mtm, 60., 26.));
    titled.setTitle(&NSString::from_str(
        "a title far wider than the custom view is",
    ));
    let titled_size = three_rows(mtm, Some(&titled)).size();

    println!("== custom-view row, differential on NSMenu.size");
    println!("  1 plain row  {one:?}");
    println!(
        "  3 plain rows {base:?}   a plain row is {:.1}pt",
        (base.height - one.height) / 2.
    );
    println!(
        "  + view frame ZERO       {zero:?}  dh {:+.1}",
        zero.height - base.height
    );
    println!(
        "  + view frame 220x26     {explicit:?}  dh {:+.1} dw {:+.1}",
        explicit.height - base.height,
        explicit.width - base.width
    );
    println!(
        "  + view 60x26 wide title {titled_size:?}  dw {:+.1}",
        titled_size.width - base.width
    );
    println!("  the 220x26 view afterwards {:?}", big.frame());
    ok &= check(
        "a view with a zero frame contributes no height at all",
        zero.height == base.height,
    );
    ok &= check(
        "a view with an explicit frame contributes exactly its height",
        explicit.height - base.height == 26.,
    );
    ok &= check(
        "a view wider than the widest text row widens the menu",
        explicit.width == 220.,
    );
    ok &= check(
        "an item that has a view ignores its own title",
        titled_size.width == base.width,
    );
    ok &= check(
        "the sizing pass leaves the view's own frame alone",
        big.frame().size.width == 220.,
    );

    println!("\n== submenu");
    let parent = NSMenu::new(mtm);
    parent.setAutoenablesItems(false);
    let host = plain(mtm, "Sort By");
    parent.addItem(&host);
    let sub = NSMenu::new(mtm);
    let child = plain(mtm, "Name");
    sub.addItem(&child);
    host.setSubmenu(Some(&sub));
    println!(
        "  autoenablesItems: parent {}  submenu {}",
        parent.autoenablesItems(),
        sub.autoenablesItems()
    );
    println!("  host action after setSubmenu: {:?}", host.action());
    ok &= check(
        "a nested NSMenu does not inherit autoenablesItems",
        !parent.autoenablesItems() && sub.autoenablesItems(),
    );
    ok &= check(
        "setSubmenu: installs AppKit's own submenuAction: over a nil action",
        host.action() == Some(sel!(submenuAction:)),
    );

    // The ordering `sync_submenu` depends on. `setSubmenu:` is not a reset: it fills a nil action
    // and leaves any other one alone, so a row that gains a submenu after a frame as an ordinary
    // row keeps firing the caller's action unless it is cleared first.
    let sink: Retained<NSObject> = NSObject::new();
    let target: &AnyObject = sink.as_ref();
    let kept = plain(mtm, "Sort By");
    unsafe {
        kept.setTarget(Some(target));
        kept.setAction(Some(sel!(ngMenuPick:)));
    }
    let late = NSMenu::new(mtm);
    late.addItem(&plain(mtm, "Name"));
    kept.setSubmenu(Some(&late));
    println!("  wired row, then setSubmenu: -> {:?}", kept.action());
    ok &= check(
        "setSubmenu: does NOT displace an action that is already there",
        kept.action() == Some(sel!(ngMenuPick:)),
    );

    let cleared = plain(mtm, "Sort By");
    unsafe {
        cleared.setTarget(Some(target));
        cleared.setAction(Some(sel!(ngMenuPick:)));
        cleared.setTarget(None);
        cleared.setAction(None);
    }
    let after = NSMenu::new(mtm);
    after.addItem(&plain(mtm, "Name"));
    cleared.setSubmenu(Some(&after));
    println!(
        "  cleared first, then setSubmenu: -> {:?}",
        cleared.action()
    );
    ok &= check(
        "clearing the action first is what hands the row to AppKit",
        cleared.action() == Some(sel!(submenuAction:)),
    );

    parent.update();
    sub.update();
    println!(
        "  after update(): host enabled {}  child enabled {}",
        host.isEnabled(),
        child.isEnabled()
    );
    ok &= check(
        "automatic validation greys out an action-less submenu row",
        !child.isEnabled(),
    );

    let guarded = NSMenu::new(mtm);
    guarded.setAutoenablesItems(false);
    let guarded_child = plain(mtm, "Name");
    guarded.addItem(&guarded_child);
    guarded.update();
    ok &= check(
        "setAutoenablesItems(false) on the submenu itself keeps its rows enabled",
        guarded_child.isEnabled(),
    );

    println!("\n== a pop-up's tick is the selected item's state");
    let popup =
        NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
    popup.setAutoenablesItems(false);
    let popup_menu = popup.menu().expect("a fresh NSPopUpButton owns a menu");
    for title in ["Automatic", "Graphite", "Vivid Blue"] {
        popup_menu.addItem(&plain(mtm, title));
    }
    let states = || -> Vec<isize> {
        (0..popup_menu.numberOfItems())
            .map(|i| popup_menu.itemAtIndex(i).expect("in range").state())
            .collect()
    };
    popup.selectItemAtIndex(2);
    println!("  selected 2 -> states {:?}", states());
    ok &= check(
        "selecting a row sets that item's state, which is the drawn tick",
        states() == vec![0, 0, 1],
    );

    // What a `sync_menu` pass over entries that are all `checked: false` does.
    for i in 0..popup_menu.numberOfItems() {
        let item = popup_menu.itemAtIndex(i).expect("in range");
        if item.state() != NSControlStateValueOff {
            item.setState(NSControlStateValueOff);
        }
    }
    println!(
        "  after an Off pass -> states {:?}, indexOfSelectedItem {}",
        states(),
        popup.indexOfSelectedItem()
    );
    ok &= check(
        "the cell keeps its index, so an index-guarded restore never fires",
        states() == vec![0, 0, 0] && popup.indexOfSelectedItem() == 2,
    );
    popup.selectItemAtIndex(2);
    ok &= check(
        "re-selecting the index the cell already holds does NOT bring the tick back",
        states() == vec![0, 0, 0],
    );
    popup_menu
        .itemAtIndex(2)
        .expect("in range")
        .setState(NSControlStateValueOn);
    ok &= check(
        "only writing the state back restores it",
        states() == vec![0, 0, 1],
    );

    let pull =
        NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, true);
    pull.setAutoenablesItems(false);
    let pull_menu = pull.menu().expect("a fresh NSPopUpButton owns a menu");
    for title in ["Document", "New Sketch", "Duplicate"] {
        pull_menu.addItem(&plain(mtm, title));
    }
    pull.selectItemAtIndex(1);
    let pull_states: Vec<isize> = (0..pull_menu.numberOfItems())
        .map(|i| pull_menu.itemAtIndex(i).expect("in range").state())
        .collect();
    println!(
        "  pull-down, selected 1 -> states {pull_states:?}, indexOfSelectedItem {}",
        pull.indexOfSelectedItem()
    );
    ok &= check(
        "a pull-down draws no tick: AppKit sets no state for one",
        pull_states == vec![0, 0, 0] && pull.indexOfSelectedItem() == 1,
    );

    println!("\n== NSMenuItem properties");
    let key = plain(mtm, "Show View Options");
    key.setKeyEquivalent(&NSString::from_str("j"));
    key.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
    println!(
        "  keyEquivalent {:?}  mask {:?}",
        key.keyEquivalent(),
        key.keyEquivalentModifierMask()
    );
    ok &= check(
        "NSMenuItem.keyEquivalent round-trips, unlike NSButton's",
        key.keyEquivalent().to_string() == "j",
    );
    let fresh = NSMenuItem::new(mtm);
    println!(
        "  a fresh item's mask {:?}",
        fresh.keyEquivalentModifierMask()
    );
    ok &= check(
        "a fresh item already carries the Command mask, so it is no discriminator",
        fresh.keyEquivalentModifierMask() == NSEventModifierFlags::Command,
    );
    let state = plain(mtm, "Use Groups");
    let before = state.state();
    state.setState(NSControlStateValueOn);
    println!("  state {before:?} -> {:?}", state.state());
    ok &= check("state round-trips", state.state() == NSControlStateValueOn);

    println!("\n== SF Symbols");
    for name in SYMBOLS {
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(name),
            None,
        );
        match &image {
            Some(image) => println!("  ok    {name:<24} {:?}", image.size()),
            None => println!("  NIL   {name}"),
        }
        ok &= image.is_some();
    }

    ok
}

struct Probe;

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        native_root(
            div().size_full().bg(rgb(0x2c2c30)).child(
                native_context_menu("more", div().size_full())
                    .entries(more_entries())
                    .origin(MenuOrigin::Pointer)
                    .on_select(|index, _, _| println!("on_select reported {index}")),
            ),
        )
    }
}

/// A pooled [`native_popup`] that repaints continuously, which is the only way the regression this
/// guards against shows up: the tick survives the first paint and is lost on the second.
struct PopupProbe {
    frames: usize,
}

impl Render for PopupProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frames += 1;
        if self.frames < 40 {
            cx.notify();
        }
        native_root(
            div()
                .size_full()
                .bg(rgb(0x2c2c30))
                .p(px(24.))
                .child(native_popup("accent", 2).items(["Automatic", "Graphite", "Vivid Blue"])),
        )
    }
}

/// Finds the `NSPopUpButton` the crate actually placed, by walking the window it went into. An
/// example cannot reach the view pool, and reaching it would prove less: this is the same object
/// the user clicks.
fn find_popup(view: &NSView) -> Option<Retained<NSPopUpButton>> {
    for subview in view.subviews().iter() {
        // `downcast` hands the value back on the miss, so the walk keeps its `Retained`.
        match subview.downcast::<NSPopUpButton>() {
            Ok(popup) => return Some(popup),
            Err(subview) => {
                if let Some(found) = find_popup(&subview) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// The tick a pop-up draws on its selected row is that item's `state`, which `sync_menu` also
/// writes from `MenuEntry::checked`. Read after many paints, because the first paint sets the
/// selection and every later one re-runs the diff over it.
fn report_popup_tick(mtm: MainThreadMarker, frames: usize) -> ! {
    let placed = NSApplication::sharedApplication(mtm)
        .windows()
        .iter()
        .next()
        .and_then(|window| window.contentView())
        .and_then(|content| find_popup(&content));
    let Some(popup) = placed else {
        println!("no NSPopUpButton was placed\nVERDICT: FAIL");
        std::process::exit(1);
    };
    let menu = popup.menu().expect("a placed NSPopUpButton owns a menu");
    let states: Vec<isize> = (0..menu.numberOfItems())
        .map(|i| menu.itemAtIndex(i).expect("in range").state())
        .collect();
    println!(
        "after {frames} paints: indexOfSelectedItem {}  states {states:?}  title {:?}",
        popup.indexOfSelectedItem(),
        popup.titleOfSelectedItem()
    );
    let ok = check(
        "the pop-up still ticks its selected row after the diff has run over it",
        states == vec![0, 0, 1] && popup.indexOfSelectedItem() == 2,
    );
    println!("\nVERDICT: {}", if ok { "PASS" } else { "FAIL" });
    std::process::exit(if ok { 0 } else { 1 });
}

/// Posts one control-left-mouse-down through this process's own `NSApplication`, routed by window
/// number, so it cannot reach another application and nothing goes near the global HID tap.
///
/// Control-left rather than right: gpui derives the button from `NSEvent.buttonNumber`, and a
/// synthesised event reports 0 whatever type it carries, so a right-mouse-down arrives as a left
/// click and a context menu correctly ignores it. Control-left is the same gesture by the other
/// route macOS offers, and it survives the zero.
fn click_own_window(mtm: MainThreadMarker, at: NSPoint) {
    let app = NSApplication::sharedApplication(mtm);
    let Some(window) = app.windows().iter().next() else {
        println!("no window to click");
        return;
    };
    let event =
        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::LeftMouseDown,
            at,
            NSEventModifierFlags::Control,
            NSProcessInfo::processInfo().systemUptime(),
            window.windowNumber(),
            None,
            0,
            1,
            1.,
        );
    match event {
        Some(event) => {
            println!(
                "posting control-left at {at:?} into window {}, buttonNumber {}",
                window.windowNumber(),
                event.buttonNumber()
            );
            app.postEvent_atStart(&event, true);
        }
        None => println!("could not synthesise the event"),
    }
}

/// Posts one key-down into this process's own queue. Menu tracking reads the same queue, so this
/// is how the open menu is navigated without moving the pointer or touching the global HID tap.
fn post_key(mtm: MainThreadMarker, window_number: isize, character: char, key_code: u16) {
    let app = NSApplication::sharedApplication(mtm);
    let text = NSString::from_str(&character.to_string());
    let event =
        NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown,
            NSPoint::ZERO,
            NSEventModifierFlags::Function | NSEventModifierFlags::NumericPad,
            NSProcessInfo::processInfo().systemUptime(),
            window_number,
            None,
            &text,
            &text,
            false,
            key_code,
        );
    if let Some(event) = event {
        app.postEvent_atStart(&event, false);
    }
}

/// Highlights the first row and opens its submenu, from inside the tracking loop.
///
/// It has to be an `NSTimer` added for the *common* run-loop modes: menu tracking runs the loop in
/// `NSEventTrackingRunLoopMode`, so a timer left in the default mode never fires while the menu is
/// up, and a background thread cannot do it instead — building an `NSEvent` or reaching `NSApp`
/// off the main thread would need a main-thread marker that would be a lie.
fn schedule_submenu_reveal(mtm: MainThreadMarker, window_number: isize, after: f64) {
    let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        println!("revealing the submenu: down, then right");
        post_key(mtm, window_number, '\u{F701}', 125);
        post_key(mtm, window_number, '\u{F703}', 124);
    });
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(after, false, &block) };
    unsafe {
        NSRunLoop::currentRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
    }
    let _ = mtm;
}

fn main() {
    let mtm = MainThreadMarker::new().expect("main thread");
    if env::var("NG_ARM").as_deref().unwrap_or("metrics") == "metrics" {
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        let ok = metrics(mtm);
        println!("\nVERDICT: {}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            std::process::exit(1);
        }
        return;
    }

    let hold: u64 = env::var("NG_HOLD")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20);

    if env::var("NG_ARM").as_deref() == Ok("popup") {
        // No `activate`: the tick is read out of AppKit, so this arm never takes the screen.
        Application::new().run(move |cx: &mut App| {
            cx.open_window(
                gpui::WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(180.), px(180.)),
                        size: size(px(320.), px(140.)),
                    })),
                    ..window_options()
                },
                |_, cx| cx.new(|_| PopupProbe { frames: 0 }),
            )
            .expect("window");

            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1500))
                    .await;
                let _ = cx.update(|_| {
                    let mtm = MainThreadMarker::new().expect("main thread");
                    report_popup_tick(mtm, 40);
                });
            })
            .detach();

            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(hold));
                println!("timed out before the tick could be read\nVERDICT: FAIL");
                std::process::exit(1);
            });
        });
        return;
    }
    Application::new().run(move |cx: &mut App| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(140.), px(140.)),
                    size: size(px(520.), px(420.)),
                })),
                ..window_options()
            },
            |_, cx| cx.new(|_| Probe),
        )
        .expect("window");
        cx.activate(true);

        // The tracking loop the menu enters blocks the main thread, so the deadline that ends the
        // run is a plain sleeping thread rather than anything scheduled on the main queue.
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(hold + 2));
            println!("hold expired");
            std::process::exit(0);
        });

        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1200))
                .await;
            let _ = cx.update(|_| {
                let mtm = MainThreadMarker::new().expect("main thread");
                let number = NSApplication::sharedApplication(mtm)
                    .windows()
                    .iter()
                    .next()
                    .map_or(0, |window| window.windowNumber());
                if env::var("NG_SUBMENU").is_ok() {
                    schedule_submenu_reveal(mtm, number, 2.5);
                }
                // Window coordinates: y-up from the bottom-left of a 520x420 content rect.
                click_own_window(mtm, NSPoint::new(150., 260.));
            });
        })
        .detach();
    });
}
