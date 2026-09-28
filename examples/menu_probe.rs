use std::env;
use std::ptr::NonNull;
use std::time::Duration;

use block2::RcBlock;
use gpui::{
    App, Bounds, Context, Modifiers, Render, Window, WindowBounds, div, point, prelude::*, px, rgb,
    size,
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
    NSWindow,
};
use objc2_foundation::{
    MainThreadMarker, NSObject, NSPoint, NSProcessInfo, NSRect, NSRunLoop, NSRunLoopCommonModes,
    NSSize, NSString, NSTimer,
};

const SYMBOLS: &[&str] = &[
    "arrow.up.arrow.down",
    "slider.horizontal.3",
    "eye",
    "rectangle.3.group",
    "folder.badge.gearshape",
    "iphone",
];

const TAGS: &[u32] = &[
    0xfc5b57, 0xf5a623, 0xf7ce46, 0x7bc862, 0x55a8f2, 0xc87fe0, 0xb8b8b8,
];

fn more_entries() -> Vec<MenuEntry> {
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
        MenuEntry::view(MenuView::swatches(TAGS.iter().map(|&hex| rgb(hex).into()))),
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

fn states(menu: &NSMenu) -> Vec<isize> {
    menu.itemArray().iter().map(|item| item.state()).collect()
}

fn new_popup(
    mtm: MainThreadMarker,
    pulls_down: bool,
    titles: [&str; 3],
) -> (Retained<NSPopUpButton>, Retained<NSMenu>) {
    let popup =
        NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, pulls_down);
    popup.setAutoenablesItems(false);
    let menu = popup.menu().expect("a fresh NSPopUpButton owns a menu");
    for title in titles {
        menu.addItem(&plain(mtm, title));
    }
    (popup, menu)
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
    let (popup, popup_menu) = new_popup(mtm, false, ["Automatic", "Graphite", "Vivid Blue"]);
    popup.selectItemAtIndex(2);
    println!("  selected 2 -> states {:?}", states(&popup_menu));
    ok &= check(
        "selecting a row sets that item's state, which is the drawn tick",
        states(&popup_menu) == [0, 0, 1],
    );

    for item in popup_menu.itemArray().iter() {
        if item.state() != NSControlStateValueOff {
            item.setState(NSControlStateValueOff);
        }
    }
    println!(
        "  after an Off pass -> states {:?}, indexOfSelectedItem {}",
        states(&popup_menu),
        popup.indexOfSelectedItem()
    );
    ok &= check(
        "the cell keeps its index, so an index-guarded restore never fires",
        states(&popup_menu) == [0, 0, 0] && popup.indexOfSelectedItem() == 2,
    );
    popup.selectItemAtIndex(2);
    ok &= check(
        "re-selecting the index the cell already holds does NOT bring the tick back",
        states(&popup_menu) == [0, 0, 0],
    );
    popup_menu
        .itemAtIndex(2)
        .expect("in range")
        .setState(NSControlStateValueOn);
    ok &= check(
        "only writing the state back restores it",
        states(&popup_menu) == [0, 0, 1],
    );

    let (pull, pull_menu) = new_popup(mtm, true, ["Document", "New Sketch", "Duplicate"]);
    pull.selectItemAtIndex(1);
    let pull_states = states(&pull_menu);
    println!(
        "  pull-down, selected 1 -> states {pull_states:?}, indexOfSelectedItem {}",
        pull.indexOfSelectedItem()
    );
    ok &= check(
        "a pull-down draws no tick: AppKit sets no state for one",
        pull_states == [0, 0, 0] && pull.indexOfSelectedItem() == 1,
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

const PAINTS: usize = 40;

struct PopupProbe {
    frames: usize,
}

impl Render for PopupProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frames += 1;
        if self.frames < PAINTS {
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

fn first_window(mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
    NSApplication::sharedApplication(mtm)
        .windows()
        .firstObject()
}

fn find_popup(view: &NSView) -> Option<Retained<NSPopUpButton>> {
    view.subviews()
        .iter()
        .find_map(|subview| match subview.downcast::<NSPopUpButton>() {
            Ok(popup) => Some(popup),
            Err(subview) => find_popup(&subview),
        })
}

fn finish(ok: bool) -> ! {
    println!("\nVERDICT: {}", if ok { "PASS" } else { "FAIL" });
    std::process::exit(if ok { 0 } else { 1 });
}

fn report_popup_tick(mtm: MainThreadMarker) -> ! {
    let placed = first_window(mtm)
        .and_then(|window| window.contentView())
        .and_then(|content| find_popup(&content));
    let Some(popup) = placed else {
        println!("no NSPopUpButton was placed\nVERDICT: FAIL");
        std::process::exit(1);
    };
    let states = states(&popup.menu().expect("a placed NSPopUpButton owns a menu"));
    println!(
        "after {PAINTS} paints: indexOfSelectedItem {}  states {states:?}  title {:?}",
        popup.indexOfSelectedItem(),
        popup.titleOfSelectedItem()
    );
    finish(check(
        "the pop-up still ticks its selected row after the diff has run over it",
        states == [0, 0, 1] && popup.indexOfSelectedItem() == 2,
    ))
}

/// Control-left, not right: gpui reads `buttonNumber`, which is 0 on any synthesised event.
fn click_own_window(mtm: MainThreadMarker, at: NSPoint) {
    let Some(window) = first_window(mtm) else {
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
            NSApplication::sharedApplication(mtm).postEvent_atStart(&event, true);
        }
        None => println!("could not synthesise the event"),
    }
}

fn post_key(mtm: MainThreadMarker, window_number: isize, key: &str, key_code: u16) {
    let text = NSString::from_str(key);
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
        NSApplication::sharedApplication(mtm).postEvent_atStart(&event, false);
    }
}

/// Common modes: menu tracking runs the loop in `NSEventTrackingRunLoopMode`.
fn schedule_submenu_reveal(window_number: isize) {
    let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        println!("revealing the submenu: down, then right");
        post_key(mtm, window_number, "\u{F701}", 125);
        post_key(mtm, window_number, "\u{F703}", 124);
    });
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(2.5, false, &block) };
    unsafe {
        NSRunLoop::currentRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
    }
}

fn main() {
    let mtm = MainThreadMarker::new().expect("main thread");
    let arm = env::var("NG_ARM").unwrap_or_else(|_| "metrics".into());
    if arm == "metrics" {
        NSApplication::sharedApplication(mtm)
            .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        finish(metrics(mtm));
    }

    let hold: u64 = env::var("NG_HOLD")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20);

    if arm == "popup" {
        gpui_platform::application().run(move |cx: &mut App| {
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
                    .timer(Duration::from_millis(1500))
                    .await;
                cx.update(|_| report_popup_tick(mtm));
            })
            .detach();

            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(hold));
                println!("timed out before the tick could be read\nVERDICT: FAIL");
                std::process::exit(1);
            });
        });
        return;
    }
    gpui_platform::application().run(move |cx: &mut App| {
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

        // Menu tracking blocks the main thread, so the deadline cannot be a main-queue timer.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(hold + 2));
            println!("hold expired");
            std::process::exit(0);
        });

        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            cx.update(|_| {
                let number = first_window(mtm).map_or(0, |window| window.windowNumber());
                if env::var("NG_SUBMENU").is_ok() {
                    schedule_submenu_reveal(number);
                }
                // Window coordinates: y-up from the bottom-left of a 520x420 content rect.
                click_own_window(mtm, NSPoint::new(150., 260.));
            });
        })
        .detach();
    });
}
