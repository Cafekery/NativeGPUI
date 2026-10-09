use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::fs;
use std::io::Write;
use std::ptr::NonNull;

use gpui::{
    App, Bounds, Context, MouseButton, Pixels, Render, Size, Window, WindowBounds, div, point,
    prelude::*, px, rgb, size,
};
use native_gpui::sidebar::{NativeSidebar, SidebarOptions, sidebar_item};
use native_gpui::window_chrome::{
    Cursor, ToolbarItem, ToolbarSegment, ToolbarSelection, ToolbarStyle, WindowLevel, chrome_host,
    corner_radius, corner_radius_readback, current_cursor_description, cursor_region, native_rect,
    native_screen_rect, native_window_rect, restore_window_flags, set_corner_radius, set_toolbar,
    set_toolbar_search_active, set_window_flags, supports_corner_radius, system_corner_radius,
    titlebar_band, toolbar_description, toolbar_item_view, traffic_light_inset, window_flags,
    window_level,
};
use native_gpui::{MenuEntry, native_root, tokens, window_options};
use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSColor, NSCursor, NSEvent, NSProgressIndicator, NSScreen,
    NSSegmentedControl, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowOrderingMode,
    NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer,
};

const DIR: &str = "/tmp/wchrome";
const CMD: &str = "/tmp/wchrome/cmd";

const ANCHOR: [f32; 4] = [560., 120., 120., 40.];
const FRACT: [f32; 4] = [560.5, 200.25, 120.5, 40.75];
const CLAIMED: [f32; 4] = [60., 250., 200., 48.];

fn log(s: &str) {
    println!("WC {s}");
    let _ = std::io::stdout().flush();
}

macro_rules! wlog {
    ($($t:tt)*) => { log(&format!($($t)*)) };
}

fn bounds_of(r: [f32; 4]) -> Bounds<Pixels> {
    Bounds::new(point(px(r[0]), px(r[1])), size(px(r[2]), px(r[3])))
}

fn fmt(r: NSRect) -> String {
    format!(
        "{:.4},{:.4} {:.4}x{:.4}",
        r.origin.x, r.origin.y, r.size.width, r.size.height
    )
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWarpMouseCursorPosition(point: NSPoint) -> i32;
    fn CGEventCreateMouseEvent(
        source: *mut c_void,
        kind: u32,
        position: NSPoint,
        button: u32,
    ) -> *mut c_void;
    fn CGEventPost(tap: u32, event: *mut c_void);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
}

const MOUSE_MOVED: u32 = 5;
const LEFT_DOWN: u32 = 1;
const LEFT_UP: u32 = 2;

/// `CGWarpMouseCursorPosition` posts no event of its own, so the move has to be posted too.
fn synth(kind: u32, at: NSPoint) {
    DRIVER.with_borrow_mut(|h| h.warped = true);
    unsafe {
        CGWarpMouseCursorPosition(at);
        let event = CGEventCreateMouseEvent(std::ptr::null_mut(), kind, at, 0);
        if !event.is_null() {
            CGEventPost(0, event);
            CFRelease(event);
        }
    }
}

thread_local! {
    static DRIVER: RefCell<Driver> = RefCell::new(Driver::default());
    static MODE: Cell<usize> = const { Cell::new(1) };
}

#[derive(Default)]
struct Driver {
    cx: Option<gpui::AsyncWindowContext>,
    overlay: Option<Retained<NSWindow>>,
    pointer_home: Option<NSPoint>,
    warped: bool,
}

fn restore_pointer() {
    let Some(home) = DRIVER.with_borrow(|h| h.pointer_home.filter(|_| h.warped)) else {
        return;
    };
    synth(MOUSE_MOVED, home);
    DRIVER.with_borrow_mut(|h| h.warped = false);
    wlog!("POINTER restored to {:.1},{:.1}", home.x, home.y);
}

fn ns_window(window: &Window) -> Retained<NSWindow> {
    chrome_host(window)
        .window()
        .expect("chrome host is in the window")
}

fn geometry(window: &Window, tag: &str) {
    let w = ns_window(window);
    let frame = w.frame();
    let naive = frame.size.height - w.contentRectForFrameRect(frame).size.height;
    let lights = traffic_light_inset(window)
        .map(|p| format!("{:.2},{:.2}", f32::from(p.x), f32::from(p.y)))
        .unwrap_or_else(|| "absent".into());
    wlog!(
        "GEOM[{tag}] frame=({}) layoutRect=({}) band={:.2} naive_band={naive:.2} lights=({lights}) styleMask=0x{:x} toolbar={}",
        fmt(frame),
        fmt(w.contentLayoutRect()),
        f32::from(titlebar_band(window)),
        w.styleMask().0,
        w.toolbar().is_some(),
    );
}

fn corner_probe(window: &Window, tag: &str) {
    let radius = |r: Option<Pixels>, none: &str| {
        r.map_or_else(|| none.to_string(), |r| format!("{:.3}", f32::from(r)))
    };
    let readback = radius(corner_radius_readback(window), "unsupported");
    let written = radius(corner_radius(window), "system");
    wlog!(
        "CORNER[{tag}] supported={} system={:.3} written={written} readback={readback}",
        supports_corner_radius(window),
        f32::from(system_corner_radius(window)),
    );
}

fn overlay(window: &Window, which: Option<[f32; 4]>) {
    let mtm = MainThreadMarker::new().unwrap();
    let parent = ns_window(window);
    let host = chrome_host(window);
    if let Some(old) = DRIVER.with_borrow_mut(|h| h.overlay.take()) {
        parent.removeChildWindow(&old);
        old.orderOut(None);
    }
    let Some(probe) = which else {
        log("OVERLAY off");
        return;
    };
    let bounds = bounds_of(probe);
    let on_screen = native_screen_rect(&host, bounds);
    wlog!(
        "COORD gpui=({:.4},{:.4} {:.4}x{:.4}) host=({}) window=({}) screen=({})",
        probe[0],
        probe[1],
        probe[2],
        probe[3],
        fmt(native_rect(bounds)),
        fmt(native_window_rect(&host, bounds)),
        fmt(on_screen),
    );
    let overlay = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            on_screen,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    let magenta = NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.0, 1.0, 1.0);
    overlay.setBackgroundColor(Some(&magenta));
    overlay.setHasShadow(false);
    overlay.setOpaque(true);
    overlay.setIgnoresMouseEvents(true);
    unsafe { parent.addChildWindow_ordered(&overlay, NSWindowOrderingMode::Above) };
    // `initWithContentRect:` rounds the rect; `setFrame:` keeps it.
    overlay.setFrame_display(on_screen, true);
    wlog!("OVERLAY frame=({})", fmt(overlay.frame()));
    DRIVER.with_borrow_mut(|h| h.overlay = Some(overlay));
}

fn flags_arm(window: &Window, name: &str, on: bool) {
    let mut flags = window_flags(window);
    match name {
        "shadow" => flags.has_shadow = on,
        "opaque" => flags.is_opaque = on,
        "alpha" => flags.alpha = if on { 1.0 } else { 0.55 },
        "movablebg" => flags.movable_by_background = on,
        "titlebar_transparent" => flags.titlebar_appears_transparent = on,
        "titled" => flags.titled = on,
        "fullsize" => flags.full_size_content = on,
        "level" => {
            flags.level = window_level(if on {
                WindowLevel::Floating
            } else {
                WindowLevel::Normal
            })
        }
        "collection" => {
            flags.collection = if on {
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
            } else {
                NSWindowCollectionBehavior::empty()
            }
        }
        "dark" => flags.appearance = on.then_some(tokens::Appearance::Dark),
        "light" => flags.appearance = on.then_some(tokens::Appearance::Light),
        other => {
            wlog!("FLAG unknown {other}");
            return;
        }
    }
    set_window_flags(window, flags);
    wlog!("FLAG {name}={on} -> {:?}", window_flags(window));
    geometry(window, &format!("flag-{name}-{on}"));
}

fn exec(line: &str, window: &mut Window, cx: &mut App) {
    let mut parts = line.split_whitespace();
    let Some(head) = parts.next() else { return };
    let arg = parts.next().unwrap_or("");
    match head {
        "corner" => {
            let radius = match arg {
                "default" | "system" => None,
                _ => arg.parse().ok().map(px),
            };
            set_corner_radius(window, radius);
            corner_probe(window, arg);
        }
        "toolbar" => {
            let style = match arg {
                "unified" => Some(ToolbarStyle::Unified),
                "compact" => Some(ToolbarStyle::UnifiedCompact),
                _ => None,
            };
            geometry(window, &format!("before-toolbar-{arg}"));
            set_toolbar(window, cx, style, &toolbar_items());
            geometry(window, &format!("after-toolbar-{arg}"));
        }
        "items" => {
            for line in toolbar_description(window).lines() {
                wlog!("ITEM {line}");
            }
        }
        "mode" => {
            if let Ok(index) = arg.parse::<usize>() {
                MODE.set(index);
                set_toolbar(window, cx, Some(ToolbarStyle::Unified), &toolbar_items());
                wlog!("MODE set {index}");
            }
        }
        "level" => {
            if let Ok(value) = arg.parse::<f64>() {
                match toolbar_item_view(window, "ng.pinned")
                    .and_then(|view| view.downcast::<NSProgressIndicator>().ok())
                {
                    Some(bar) => {
                        bar.setIndeterminate(false);
                        bar.setDoubleValue(value);
                        wlog!(
                            "LEVEL ng.pinned <- {value} (readback {})",
                            bar.doubleValue()
                        );
                    }
                    None => log("LEVEL ng.pinned has no view"),
                }
            }
        }
        "search" => {
            let active = arg != "off";
            set_toolbar_search_active(window, "ng.search", active);
            wlog!("SEARCH active={active}");
        }
        "resize" => {
            if let Ok(width) = arg.parse::<f64>() {
                if let Some(ns_window) = chrome_host(window).window() {
                    let mut frame = ns_window.frame();
                    frame.size.width = width;
                    ns_window.setFrame_display(frame, true);
                    wlog!("RESIZE width={width}");
                }
            }
        }
        "sidebar" => {
            let sidebar = NativeSidebar::install(window, cx, SidebarOptions::default());
            sidebar.update(&[
                sidebar_item("here", "Here").symbol("folder"),
                sidebar_item("there", "There").symbol("clock"),
            ]);
            sidebar.set_selected(Some("here"));
            let mut items = toolbar_items();
            items.insert(1, ToolbarItem::separator("ng.divider", 0));
            set_toolbar(window, cx, Some(ToolbarStyle::Unified), &items);
            wlog!("SIDEBAR installed");
        }
        "overlay" => overlay(
            window,
            match arg {
                "int" => Some(ANCHOR),
                "frac" => Some(FRACT),
                _ => None,
            },
        ),
        "flags" => {
            if arg == "restore" {
                restore_window_flags(window);
                geometry(window, "flags-restored");
                wlog!("FLAG restored -> {:?}", window_flags(window));
            } else {
                flags_arm(window, arg, parts.next() == Some("on"));
            }
        }
        "move" | "click" => {
            let x: f64 = arg.parse().unwrap_or(0.);
            let y: f64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.);
            let at = NSPoint::new(x, y);
            synth(MOUSE_MOVED, at);
            if head == "move" {
                wlog!("MOVE warp+mouseMoved to {x:.1},{y:.1}");
            } else {
                synth(LEFT_DOWN, at);
                synth(LEFT_UP, at);
                wlog!("CLICK warp+moved+down+up at {x:.1},{y:.1}");
            }
        }
        "pointer" => restore_pointer(),
        "cursor" => {
            let host = chrome_host(window);
            let want = native_screen_rect(&host, bounds_of(CLAIMED));
            let mouse = NSEvent::mouseLocation();
            let inside = mouse.x >= want.origin.x
                && mouse.x <= want.origin.x + want.size.width
                && mouse.y >= want.origin.y
                && mouse.y <= want.origin.y + want.size.height;
            // `currentCursor` is this process's own stack; `currentSystemCursor` is what shows.
            #[allow(deprecated)]
            let system = NSCursor::currentSystemCursor()
                .map(|c| {
                    format!(
                        "{} hotSpot={:.1},{:.1} imageSize={:.1}x{:.1}",
                        c.class().name().to_string_lossy(),
                        c.hotSpot().x,
                        c.hotSpot().y,
                        c.image().size().width,
                        c.image().size().height
                    )
                })
                .unwrap_or_else(|| "none".into());
            // `hitTest:` takes a point in the receiver's superview space, so ask the content view.
            let in_window = native_window_rect(&host, bounds_of(CLAIMED));
            let centre = NSPoint::new(
                in_window.origin.x + in_window.size.width / 2.,
                in_window.origin.y + in_window.size.height / 2.,
            );
            let w = ns_window(window);
            let hit = w
                .contentView()
                .and_then(|c| c.hitTest(centre))
                .map(|v| v.class().name().to_string_lossy().into_owned())
                .unwrap_or_else(|| "nil".into());
            wlog!(
                "CURSOR hitTest@claimed_centre={hit} key={} current={} system={system} mouse={:.1},{:.1} claimed_screen=({}) inside={inside}",
                w.isKeyWindow(),
                current_cursor_description(),
                mouse.x,
                mouse.y,
                fmt(want),
            );
        }
        "probe" => {
            geometry(window, "probe");
            corner_probe(window, "probe");
            let host = chrome_host(window);
            let gpui = unsafe { host.superview() }.unwrap();
            wlog!(
                "PROBE host.isFlipped={} gpuiView.isFlipped={} scale={}",
                host.isFlipped(),
                gpui.isFlipped(),
                window.scale_factor(),
            );
            wlog!("PROBE flags={:?}", window_flags(window));
        }
        "activate" => {
            let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            let w = ns_window(window);
            w.makeKeyAndOrderFront(None);
            wlog!("ACTIVATE key={} main={}", w.isKeyWindow(), w.isMainWindow());
        }
        "quit" => {
            restore_pointer();
            log("QUIT");
            cx.quit();
        }
        other => wlog!("UNKNOWN {other}"),
    }
    window.refresh();
}

fn pump() {
    let Ok(body) = fs::read_to_string(CMD) else {
        return;
    };
    if body.trim().is_empty() {
        return;
    }
    let _ = fs::write(CMD, "");
    let Some(mut cx) = DRIVER.with_borrow(|h| h.cx.clone()) else {
        return;
    };
    let _ = cx.update(|window, app| {
        for line in body.lines() {
            exec(line, window, app);
        }
    });
}

fn toolbar_items() -> Vec<ToolbarItem> {
    vec![
        ToolbarItem::symbol("ng.sidebar", "sidebar.left", "Sidebar")
            .on_click(|_, _| log("TOOLBAR clicked ng.sidebar")),
        ToolbarItem::flexible_space(),
        ToolbarItem::group(
            "ng.view",
            "View",
            [
                ToolbarSegment::new("square.grid.2x2", "Icons"),
                ToolbarSegment::new("list.bullet", "List"),
                ToolbarSegment::new("rectangle.split.3x1", "Columns"),
                ToolbarSegment::new("squares.below.rectangle", "Gallery"),
            ],
            MODE.get(),
        )
        .on_select(|index, _, _| {
            MODE.set(index);
            wlog!("TOOLBAR group ng.view -> {index}");
        }),
        ToolbarItem::menu(
            "ng.sort",
            "square.grid.3x3",
            "Group",
            [
                MenuEntry::item("Name").symbol("textformat"),
                MenuEntry::item("Kind").symbol("doc"),
                MenuEntry::separator(),
                MenuEntry::item("Date Modified").symbol("calendar"),
            ],
        )
        .on_select(|index, _, _| wlog!("TOOLBAR menu ng.sort -> {index}")),
        ToolbarItem::group(
            "ng.nav",
            "Navigate",
            [
                ToolbarSegment::new("chevron.left", "Back"),
                ToolbarSegment::new("chevron.right", "Forward"),
            ],
            0,
        )
        .selection(ToolbarSelection::Momentary)
        .on_select(|index, _, _| wlog!("TOOLBAR group ng.nav -> {index}")),
        ToolbarItem::share("ng.share", "Share", ["A shared string from the toolbar"]),
        ToolbarItem::symbol("ng.tag", "tag", "Tags").on_click(|_, _| log("TOOLBAR clicked ng.tag")),
        ToolbarItem::view("ng.seg", "Segmented", Size::default(), |mtm| {
            let control = NSSegmentedControl::new(mtm);
            control.setSegmentCount(2);
            control.setLabel_forSegment(&NSString::from_str("A"), 0);
            control.setLabel_forSegment(&NSString::from_str("B"), 1);
            Retained::into_super(Retained::into_super(control))
        }),
        ToolbarItem::view("ng.bare", "Bare", Size::default(), NSView::new),
        ToolbarItem::view("ng.pinned", "Pinned", size(px(90.), px(22.)), |mtm| {
            let bar = NSProgressIndicator::new(mtm);
            bar.setIndeterminate(false);
            // `NSProgressIndicator`'s range is 0..100, not 0..1.
            bar.setDoubleValue(35.);
            Retained::into_super(bar)
        }),
        ToolbarItem::search("ng.search", "Search")
            .placeholder("Search")
            .width(px(240.))
            .on_change(|text, _, _| wlog!("TOOLBAR search ng.search -> {text:?}")),
    ]
}

struct Demo {
    installed: bool,
    clicks: usize,
    control_clicks: usize,
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.installed {
            self.installed = true;
            let _ = fs::create_dir_all(DIR);
            let _ = fs::write(CMD, "");
            // Both spaces are anchored to the PRIMARY screen, so the flip uses screens()[0].
            let mouse = NSEvent::mouseLocation();
            let height = NSScreen::screens(MainThreadMarker::new().unwrap())
                .firstObject()
                .map_or(0., |s| s.frame().size.height);
            DRIVER.with_borrow_mut(|h| {
                h.cx = Some(window.to_async(cx));
                h.pointer_home = Some(NSPoint::new(mouse.x, height - mouse.y));
            });

            let block = block2::RcBlock::new(|_: NonNull<NSTimer>| pump());
            unsafe {
                let timer = NSTimer::timerWithTimeInterval_repeats_block(0.1, true, &block);
                NSRunLoop::currentRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
            }

            wlog!("READY pid={} cmd={CMD}", std::process::id());
            geometry(window, "install");
            corner_probe(window, "install");
        }

        let band = titlebar_band(window);
        let inset = traffic_light_inset(window);
        let written = corner_radius(window);
        let readback = corner_radius_readback(window);

        let label = |text: String| {
            div()
                .text_color(rgb(0xe8eaed))
                .text_size(px(13.))
                .child(text)
        };
        let button = |id: &'static str, text: &'static str| {
            div()
                .id(id)
                .px_3()
                .py_2()
                .rounded_md()
                .bg(rgb(0x2c3136))
                .text_color(rgb(0xe8eaed))
                .text_size(px(12.))
                .hover(|s| s.bg(rgb(0x3d444b)))
                .child(text)
        };
        let pt = |r: Option<Pixels>, none: &str| {
            r.map_or_else(|| none.to_string(), |r| format!("{:.2}pt", f32::from(r)))
        };
        let row = |dy: f32| {
            div()
                .absolute()
                .left(px(60.))
                .top(band + px(dy))
                .flex()
                .gap_2()
        };
        let radii = [
            ("r8", "8", Some(8.)),
            ("r16", "16", Some(16.)),
            ("r24", "24", Some(24.)),
            ("r40", "40", Some(40.)),
            ("rsys", "system", None),
        ]
        .map(|(id, text, radius)| {
            button(id, text).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _, window, cx| {
                    set_corner_radius(window, radius.map(px));
                    cx.notify();
                }),
            )
        });
        let toolbars = [
            ("tb-unified", "unified toolbar", Some(ToolbarStyle::Unified)),
            ("tb-compact", "compact", Some(ToolbarStyle::UnifiedCompact)),
            ("tb-off", "no toolbar", None),
        ]
        .map(|(id, text, style)| {
            button(id, text).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _, window, cx| {
                    let items = style.map_or_else(Vec::new, |_| toolbar_items());
                    set_toolbar(window, cx, style, &items);
                    cx.notify();
                }),
            )
        });

        native_root(
            div()
                .size_full()
                .relative()
                .font_family(tokens::system_font_family())
                // Below the band: on a full-size content view a full-bleed fill hides the toolbar.
                .child(
                    div()
                        .absolute()
                        .left(px(0.))
                        .right(px(0.))
                        .top(band)
                        .bottom(px(0.))
                        .bg(rgb(0x141719)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(60.))
                        .top(band + px(10.))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(label(format!(
                            "titlebar band {:.2}pt   traffic lights {}",
                            f32::from(band),
                            inset.map_or_else(
                                || "absent".into(),
                                |p| format!("inset {:.2},{:.2}", f32::from(p.x), f32::from(p.y))
                            )
                        )))
                        .child(label(format!(
                            "corner: written {}   readback {}   system default {:.2}pt",
                            pt(written, "system"),
                            pt(readback, "n/a"),
                            f32::from(system_corner_radius(window)),
                        )))
                        .child(label(format!(
                            "cursor region clicks {}   control clicks {}",
                            self.clicks, self.control_clicks
                        ))),
                )
                .child(row(78.).children(radii))
                .child(row(122.).children(toolbars))
                .child(
                    div()
                        .absolute()
                        .left(px(CLAIMED[0]))
                        .top(px(CLAIMED[1]))
                        .child(cursor_region(
                            "claimed",
                            Cursor::PointingHand,
                            div()
                                .id("claimed-inner")
                                .w(px(CLAIMED[2]))
                                .h(px(CLAIMED[3]))
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(rgb(0x00d0ff))
                                .text_color(rgb(0x0b0d0f))
                                .text_size(px(12.))
                                .hover(|s| s.bg(rgb(0xff2020)))
                                .on_hover(|hovered, _, _| wlog!("GPUIHOVER claimed={hovered}"))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this: &mut Demo, _, _, cx| {
                                        this.clicks += 1;
                                        log("GPUICLICK claimed reached GPUI");
                                        cx.notify();
                                    }),
                                )
                                .child("pointing hand"),
                        )),
                )
                .child(
                    div()
                        .id("control")
                        .absolute()
                        .left(px(300.))
                        .top(px(250.))
                        .w(px(200.))
                        .h(px(48.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgb(0xa0a0ff))
                        .text_color(rgb(0x0b0d0f))
                        .text_size(px(12.))
                        .hover(|s| s.bg(rgb(0xff2020)))
                        .on_hover(|hovered, _, _| wlog!("GPUIHOVER control={hovered}"))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this: &mut Demo, _, _, cx| {
                                this.control_clicks += 1;
                                log("GPUICLICK control reached GPUI");
                                cx.notify();
                            }),
                        )
                        .child("unclaimed control"),
                )
                .children([ANCHOR, FRACT].map(|r| {
                    div()
                        .absolute()
                        .left(px(r[0]))
                        .top(px(r[1]))
                        .w(px(r[2]))
                        .h(px(r[3]))
                        .bg(rgb(0xff8000))
                })),
        )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        tokens::init(cx);
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(1150.), px(200.)),
                    size: size(px(820.), px(560.)),
                })),
                // No TitlebarOptions::traffic_light_position: it excludes a toolbar.
                ..window_options()
            },
            |_, cx| {
                cx.new(|_| Demo {
                    installed: false,
                    clicks: 0,
                    control_clicks: 0,
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
