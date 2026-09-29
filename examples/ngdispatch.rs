//! Gate: a hosted AppKit control still reaches its GPUI handler, and a cleared handler stays
//! silent. Exits 0 pass, 1 fail, 90 void; `NGD_SABOTAGE=nohandler|noclear` are the failing arms.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use gpui::{App, Bounds, Context, Render, Window, WindowBounds, div, point, prelude::*, px, size};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{ClassType, msg_send};
use objc2_app_kit::{NSSwitch, NSView};
use objc2_foundation::NSObjectProtocol;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use native_gpui::{native_root, native_switch, tokens, window_options};

const FRAMES: u32 = 75;
// Handlers are dispatched on a later turn, never inline, so a read cannot be adjacent to its click.
const CLICK_ONE: u32 = 30;
const READ_ONE: u32 = 45;
const CLEAR: u32 = 46;
const CLICK_TWO: u32 = 60;
const STALL: Duration = Duration::from_secs(15);

#[derive(Default)]
struct Counters {
    frame: AtomicU32,
    fires: AtomicU32,
    at_click_one: AtomicU32,
    reported: AtomicBool,
}

struct Probe {
    armed: bool,
    clears: bool,
    counters: Arc<Counters>,
    started: Instant,
}

fn gpui_ns_view(window: &Window) -> Retained<NSView> {
    let handle = HasWindowHandle::window_handle(window).expect("gpui window has a native handle");
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        unreachable!("macOS-only probe saw a non-AppKit handle");
    };
    unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }.expect("live NSView")
}

fn find_switch(view: &NSView) -> Option<Retained<NSSwitch>> {
    if view.isKindOfClass(NSSwitch::class()) {
        let raw = (view as *const NSView).cast::<NSSwitch>().cast_mut();
        return unsafe { Retained::retain(raw) };
    }
    let subviews = view.subviews();
    (0..subviews.count()).find_map(|index| find_switch(&subviews.objectAtIndex(index)))
}

fn report(counters: &Counters, elapsed: f64) -> ! {
    use std::io::Write;
    let frame = counters.frame.load(Ordering::Relaxed);
    let armed = counters.at_click_one.load(Ordering::Relaxed);
    let cleared = counters.fires.load(Ordering::Relaxed).saturating_sub(armed);
    let (verdict, code) = if frame < FRAMES {
        (
            format!("VOID (window painted {frame} frames; both arms need {FRAMES})"),
            90,
        )
    } else if armed == 0 {
        (
            "VOID (armed arm never fired — nothing was listening)".to_owned(),
            90,
        )
    } else if cleared == 0 {
        ("PASS".to_owned(), 0)
    } else {
        ("FAIL".to_owned(), 1)
    };
    println!(
        "NG-DISPATCH frames={frame} t={elapsed:.2}s armed_fires={armed} \
         cleared_fires={cleared} verdict={verdict}"
    );
    std::io::stdout().flush().ok();
    std::process::exit(code)
}

fn watchdog(counters: Arc<Counters>, started: Instant) {
    thread::spawn(move || {
        thread::sleep(STALL);
        if counters.reported.swap(true, Ordering::SeqCst) {
            return;
        }
        report(&counters, started.elapsed().as_secs_f64())
    });
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();
        let frame = self.counters.frame.fetch_add(1, Ordering::Relaxed) + 1;

        if frame == CLEAR && self.clears {
            self.armed = false;
        }

        if frame == CLICK_ONE || frame == CLICK_TWO {
            let root = gpui_ns_view(window);
            let content = unsafe { root.superview() }.expect("gpui's view sits in a content view");
            match find_switch(&content) {
                Some(switch) => {
                    let _: bool =
                        unsafe { msg_send![&*switch, performClick: std::ptr::null::<AnyObject>()] };
                }
                None => println!("NG-DISPATCH ABORT: no hosted NSSwitch in the window"),
            }
        }

        if frame == READ_ONE {
            let fires = self.counters.fires.load(Ordering::Relaxed);
            self.counters.at_click_one.store(fires, Ordering::Relaxed);
        }
        if frame >= FRAMES && !self.counters.reported.swap(true, Ordering::SeqCst) {
            report(&self.counters, self.started.elapsed().as_secs_f64())
        }

        let mut switch = native_switch("probe", true);
        if self.armed {
            switch = switch.on_change(cx.listener(|this: &mut Probe, _: &bool, _, _| {
                this.counters.fires.fetch_add(1, Ordering::Relaxed);
            }));
        }

        native_root(
            div()
                .size_full()
                .bg(tokens::window_background())
                .p_4()
                .child(switch),
        )
    }
}

fn main() {
    let counters = Arc::new(Counters::default());
    let started = Instant::now();
    watchdog(counters.clone(), started);
    let sabotage = std::env::var("NGD_SABOTAGE").unwrap_or_default();
    gpui_platform::application().run(move |cx: &mut App| {
        tokens::init(cx);
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(60.), px(60.)),
                    size: size(px(320.), px(160.)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|_| Probe {
                    armed: sabotage != "nohandler",
                    clears: sabotage != "noclear",
                    counters,
                    started,
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
