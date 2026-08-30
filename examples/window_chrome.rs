//! Window-level chrome, live: the corner radius driven by GPUI controls, a real unified toolbar
//! with two SF Symbol items, a GPUI region wearing the system pointing hand, and the titlebar band
//! and traffic-light inset read from the system every frame and painted by GPUI.
//!
//! The arms with no on-screen control are driven by appending to a command file:
//!
//! ```text
//! echo corner 24       >> /tmp/wchrome/cmd
//! echo toolbar unified >> /tmp/wchrome/cmd
//! echo overlay int     >> /tmp/wchrome/cmd
//! echo flags dark on   >> /tmp/wchrome/cmd
//! echo activate        >> /tmp/wchrome/cmd
//! echo move <x> <y>    >> /tmp/wchrome/cmd   # global top-left, over the cyan region
//! echo cursor          >> /tmp/wchrome/cmd
//! echo probe           >> /tmp/wchrome/cmd
//! ```
//!
//! Everything it prints is prefixed `WC `. `move` and `click` warp the real pointer; `pointer`
//! puts it back, and so does `quit`.
//!
//! `cursor` reports whether the claim registered, which is key-independent. Seeing the glyph
//! itself swap additionally needs the app to be key, which is what `activate` is for — one-shot
//! activation at launch does not survive focus being stolen.
//!
//! The GPUI-painted probes, in content-logical points: `anchor` and `fract` are the orange
//! coordinate-recipe targets the `overlay` arm lands a child window on, `claimed` is the cyan
//! cursor region, and the blue control beside it is never claimed, so it must keep reacting.

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::ptr::NonNull;

use gpui::{
    App, Application, Bounds, Context, MouseButton, Render, Window, WindowBounds, div, point,
    prelude::*, px, rgb, size,
};
use native_gpui::window_chrome::{
    Cursor, ToolbarItem, ToolbarStyle, WindowLevel, chrome_host, corner_radius,
    corner_radius_readback, current_cursor_description, cursor_region, native_rect,
    native_screen_rect, native_window_rect, restore_window_flags, set_corner_radius, set_toolbar,
    set_window_flags, supports_corner_radius, system_corner_radius, titlebar_band,
    traffic_light_inset, window_flags, window_level,
};
use native_gpui::{native_root, tokens, window_options};
use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSColor, NSWindow, NSWindowCollectionBehavior,
    NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSTimer,
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

fn bounds_of(r: [f32; 4]) -> Bounds<gpui::Pixels> {
    Bounds {
        origin: point(px(r[0]), px(r[1])),
        size: size(px(r[2]), px(r[3])),
    }
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
        source: *mut std::ffi::c_void,
        kind: u32,
        position: NSPoint,
        button: u32,
    ) -> *mut std::ffi::c_void;
    fn CGEventPost(tap: u32, event: *mut std::ffi::c_void);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const std::ffi::c_void);
}

/// Both halves are load-bearing: `CGWarpMouseCursorPosition` moves the pointer but posts nothing,
/// so AppKit never re-evaluates which cursor rect it is in, and a posted move without the warp
/// drives the event path with the pointer somewhere else.
fn synth(kind: u32, x: f64, y: f64) {
    let at = NSPoint::new(x, y);
    DRIVER.with(|h| h.borrow_mut().warped = true);
    unsafe {
        CGWarpMouseCursorPosition(at);
        let event = CGEventCreateMouseEvent(std::ptr::null_mut(), kind, at, 0);
        if !event.is_null() {
            CGEventPost(0, event);
            CFRelease(event.cast_const());
        }
    }
}

const MOUSE_MOVED: u32 = 5;
const LEFT_DOWN: u32 = 1;
const LEFT_UP: u32 = 2;

thread_local! {
    /// Scratch state only. Everything the *crate* owns is read back from the crate, never mirrored
    /// here, so a readout cannot drift from the thing it reports.
    static DRIVER: RefCell<Driver> = RefCell::new(Driver::default());
}

#[derive(Default)]
struct Driver {
    cx: Option<gpui::AsyncWindowContext>,
    overlay: Option<Retained<NSWindow>>,
    /// Where the operator's pointer was at launch, already flipped to the global top-left space
    /// `CGWarpMouseCursorPosition` takes.
    pointer_home: Option<NSPoint>,
    warped: bool,
}

/// Put the operator's pointer back, but only if this process moved it — otherwise `quit` would
/// yank a pointer the operator has since moved themselves.
fn restore_pointer() {
    let home = DRIVER.with(|h| {
        let h = h.borrow();
        h.warped.then_some(h.pointer_home).flatten()
    });
    let Some(home) = home else { return };
    synth(MOUSE_MOVED, home.x, home.y);
    DRIVER.with(|h| h.borrow_mut().warped = false);
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
    let readback = corner_radius_readback(window)
        .map(|r| format!("{:.3}", f32::from(r)))
        .unwrap_or_else(|| "unsupported".into());
    let written = corner_radius(window)
        .map(|r| format!("{:.3}", f32::from(r)))
        .unwrap_or_else(|| "system".into());
    wlog!(
        "CORNER[{tag}] supported={} system={:.3} written={written} readback={readback}",
        supports_corner_radius(window),
        f32::from(system_corner_radius(window)),
    );
}

/// Place a magenta child window at exactly `native_screen_rect(host, bounds)`. If the coordinate
/// recipe is exact, it covers the orange GPUI target with no orange left.
fn overlay(window: &Window, which: Option<[f32; 4]>) {
    let mtm = MainThreadMarker::new().unwrap();
    let parent = ns_window(window);
    let host = chrome_host(window);
    DRIVER.with(|h| {
        let mut h = h.borrow_mut();
        if let Some(old) = h.overlay.take() {
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
        h.overlay = Some(overlay);
    });
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
            let radius = if arg == "default" || arg == "system" {
                None
            } else {
                arg.parse::<f32>().ok().map(px)
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
        "move" => {
            let x: f64 = arg.parse().unwrap_or(0.);
            let y: f64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.);
            synth(MOUSE_MOVED, x, y);
            wlog!("MOVE warp+mouseMoved to {x:.1},{y:.1}");
        }
        "click" => {
            let x: f64 = arg.parse().unwrap_or(0.);
            let y: f64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.);
            synth(MOUSE_MOVED, x, y);
            synth(LEFT_DOWN, x, y);
            synth(LEFT_UP, x, y);
            wlog!("CLICK warp+moved+down+up at {x:.1},{y:.1}");
        }
        "pointer" => restore_pointer(),
        "cursor" => {
            let host = chrome_host(window);
            let want = native_screen_rect(&host, bounds_of(CLAIMED));
            let mouse = objc2_app_kit::NSEvent::mouseLocation();
            let inside = mouse.x >= want.origin.x
                && mouse.x <= want.origin.x + want.size.width
                && mouse.y >= want.origin.y
                && mouse.y <= want.origin.y + want.size.height;
            // `currentCursor` is this process's own cursor stack; `currentSystemCursor` is what
            // the WindowServer is showing. If only the first moves, we set a cursor nobody sees.
            #[allow(deprecated)]
            let system = objc2_app_kit::NSCursor::currentSystemCursor()
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
            // `hitTest:` takes a point in the RECEIVER's superview space, so ask the content view
            // and let it walk down. Anything but NGChrome means the claim never registered, which
            // separates "the claim is wrong" from "the cursor is wrong".
            let in_window = native_window_rect(&host, bounds_of(CLAIMED));
            let centre = NSPoint::new(
                in_window.origin.x + in_window.size.width / 2.,
                in_window.origin.y + in_window.size.height / 2.,
            );
            let hit = ns_window(window)
                .contentView()
                .and_then(|c| c.hitTest(centre))
                .map(|v| v.class().name().to_string_lossy().to_string())
                .unwrap_or_else(|| "nil".into());
            wlog!(
                "CURSOR hitTest@claimed_centre={hit} key={} current={} system={system} mouse={:.1},{:.1} claimed_screen=({}) inside={inside}",
                ns_window(window).isKeyWindow(),
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
        // Cursor rects only apply to a key, active app, so `cursor` cannot observe a glyph swap
        // until this has run. One-shot activation at launch does not survive focus being stolen.
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
    let cx = DRIVER.with(|h| h.borrow().cx.clone());
    let Some(mut cx) = cx else { return };
    let _ = cx.update(|window, app| {
        for line in body.lines() {
            let line = line.trim();
            if !line.is_empty() {
                exec(line, window, app);
            }
        }
    });
}

fn toolbar_items() -> Vec<ToolbarItem> {
    vec![
        ToolbarItem::symbol("ng.sidebar", "sidebar.left", "Sidebar")
            .on_click(|_, _| log("TOOLBAR clicked ng.sidebar")),
        ToolbarItem::flexible_space(),
        ToolbarItem::symbol("ng.share", "square.and.arrow.up", "Share")
            .on_click(|_, _| log("TOOLBAR clicked ng.share")),
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
            // `mouseLocation` is Cocoa bottom-left; `CGWarpMouseCursorPosition` takes global
            // top-left. Both spaces are anchored to the PRIMARY screen, so the flip constant is
            // `screens()[0]`'s height and not this window's screen — a pointer parked on a
            // display above the primary correctly flips to a negative y.
            let mouse = objc2_app_kit::NSEvent::mouseLocation();
            let height = objc2_app_kit::NSScreen::screens(MainThreadMarker::new().unwrap())
                .firstObject()
                .map_or(0., |s| s.frame().size.height);
            DRIVER.with(|h| {
                let mut h = h.borrow_mut();
                h.cx = Some(window.to_async(cx));
                h.pointer_home = Some(NSPoint::new(mouse.x, height - mouse.y));
            });

            let block = block2::RcBlock::new(move |_: NonNull<NSTimer>| pump());
            let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(0.1, true, &block) };
            unsafe { NSRunLoop::currentRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
            std::mem::forget(timer);

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

        native_root(
            div()
                .size_full()
                .relative()
                .font_family(tokens::system_font_family())
                // Starts BELOW the live band, not at the frame top: on a FullSizeContentView
                // window GPUI's content view spans the whole frame, so a full-bleed background
                // paints over the toolbar and the traffic lights and they vanish. It still reaches
                // the bottom corners opaquely, which is what the drop shadow needs.
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
                            match inset {
                                Some(p) =>
                                    format!("inset {:.2},{:.2}", f32::from(p.x), f32::from(p.y)),
                                None => "absent".into(),
                            }
                        )))
                        .child(label(format!(
                            "corner: written {}   readback {}   system default {:.2}pt",
                            written
                                .map(|r| format!("{:.2}pt", f32::from(r)))
                                .unwrap_or_else(|| "system".into()),
                            readback
                                .map(|r| format!("{:.2}pt", f32::from(r)))
                                .unwrap_or_else(|| "n/a".into()),
                            f32::from(system_corner_radius(window)),
                        )))
                        .child(label(format!(
                            "cursor region clicks {}   control clicks {}",
                            self.clicks, self.control_clicks
                        ))),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(60.))
                        .top(band + px(78.))
                        .flex()
                        .gap_2()
                        .child(button("r8", "8").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, window, cx| {
                                set_corner_radius(window, Some(px(8.)));
                                cx.notify();
                            }),
                        ))
                        .child(button("r16", "16").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, window, cx| {
                                set_corner_radius(window, Some(px(16.)));
                                cx.notify();
                            }),
                        ))
                        .child(button("r24", "24").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, window, cx| {
                                set_corner_radius(window, Some(px(24.)));
                                cx.notify();
                            }),
                        ))
                        .child(button("r40", "40").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, window, cx| {
                                set_corner_radius(window, Some(px(40.)));
                                cx.notify();
                            }),
                        ))
                        .child(button("rsys", "system").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, window, cx| {
                                set_corner_radius(window, None);
                                cx.notify();
                            }),
                        )),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(60.))
                        .top(band + px(122.))
                        .flex()
                        .gap_2()
                        .child(button("tb-unified", "unified toolbar").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, window, cx| {
                                set_toolbar(
                                    window,
                                    cx,
                                    Some(ToolbarStyle::Unified),
                                    &toolbar_items(),
                                );
                                cx.notify();
                            }),
                        ))
                        .child(button("tb-compact", "compact").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, window, cx| {
                                set_toolbar(
                                    window,
                                    cx,
                                    Some(ToolbarStyle::UnifiedCompact),
                                    &toolbar_items(),
                                );
                                cx.notify();
                            }),
                        ))
                        .child(button("tb-off", "no toolbar").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, window, cx| {
                                set_toolbar(window, cx, None, &[]);
                                cx.notify();
                            }),
                        )),
                )
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
                // Never claimed: the discriminator for "claiming costs GPUI nothing".
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
                .child(
                    div()
                        .absolute()
                        .left(px(ANCHOR[0]))
                        .top(px(ANCHOR[1]))
                        .w(px(ANCHOR[2]))
                        .h(px(ANCHOR[3]))
                        .bg(rgb(0xff8000)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(FRACT[0]))
                        .top(px(FRACT[1]))
                        .w(px(FRACT[2]))
                        .h(px(FRACT[3]))
                        .bg(rgb(0xff8000)),
                ),
        )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        tokens::init(cx);
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(1150.), px(200.)),
                    size: size(px(820.), px(560.)),
                })),
                // Deliberately NOT setting TitlebarOptions::traffic_light_position: it is mutually
                // exclusive with a toolbar. See `set_toolbar`.
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
