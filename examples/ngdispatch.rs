//! Does a hosted AppKit control still reach its GPUI handler? Self-reporting, no screen needed.
//!
//! `element_key` -> `place` -> `wire` -> `on_change` is the crate's extension point, and until
//! this example the only thing standing behind it was that the shipped components happen to work
//! when a human looks at a window. This drives a real `NSSwitch` with `performClick:` in process,
//! so it needs no synthetic input, no capture and no key window.
//!
//! Two arms, one control, at two times:
//!
//! * **ARMED** — a handler is registered, the switch is clicked, the handler must run.
//! * **CLEARED** — the same switch is repainted with no handler, clicked again, and nothing must
//!   run. That is [`NativeHost::on_change`] with `None`, which components use to drop a stale
//!   handler off a reused pool slot.
//!
//! `armed_fires == 0` reports **VOID**, never PASS: a control that fired nothing and a control
//! nobody was listening to produce the same silence on the cleared arm, so the positive arm has to
//! carry the run.
//!
//! It does need a window that **paints**, `FRAMES` times, which is a different requirement from
//! needing a screen and is the one that starves. See [`watchdog`].
//!
//! Exit codes, so a gate can use it: **0** pass, **1** fail, **90** void — the run proved nothing.
//! `NGD_SABOTAGE` proves each arm can fail rather than asserting it:
//!
//! ```text
//! cargo run --release --example ngdispatch                      # 0, verdict=PASS
//! NGD_SABOTAGE=nohandler cargo run --release --example ngdispatch  # 90, verdict=VOID
//! NGD_SABOTAGE=noclear   cargo run --release --example ngdispatch  # 1,  verdict=FAIL
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Application, Bounds, Context, Render, Window, WindowBounds, div, point,
    prelude::*, px, size,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{ClassType, msg_send};
use objc2_app_kit::{NSSwitch, NSView};
use objc2_foundation::NSObjectProtocol;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use native_gpui::{native_root, native_switch, tokens, window_options};

/// Frames both arms need: click, read, clear, click, read.
const FRAMES: u32 = 75;
/// Frame the first click lands on, and the frame its fires are read at — handlers are dispatched on
/// a later turn, never inline, so the read cannot be adjacent to the click.
const CLICK_ONE: u32 = 30;
const READ_ONE: u32 = 45;
/// Frame the switch is first painted with no handler, which is the clearing path.
const CLEAR: u32 = 46;
const CLICK_TWO: u32 = 60;
/// How long [`watchdog`] gives the window to paint `FRAMES` times. At a display's own rate that
/// takes under a second, so this is slack rather than a threshold.
const STALL: Duration = Duration::from_secs(15);

/// Everything the verdict is computed from, shared because [`watchdog`] reads it off a thread that
/// `render` does not run on. One source of truth: `render` writes only through here.
#[derive(Default)]
struct Counters {
    frame: AtomicU32,
    fires: AtomicU32,
    at_click_one: AtomicU32,
    /// Claimed by whichever of the two paths reports first, so they cannot both print.
    reported: AtomicBool,
}

struct Probe {
    armed: bool,
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

/// The first hosted `NSSwitch` in the window, found by class rather than by pool key: the pool has
/// no public accessor and this proves the click lands on the same object the crate placed.
fn find_switch(view: &NSView) -> Option<Retained<NSSwitch>> {
    if view.isKindOfClass(NSSwitch::class()) {
        let raw = (view as *const NSView).cast::<NSSwitch>().cast_mut();
        return unsafe { Retained::retain(raw) };
    }
    let subviews = view.subviews();
    (0..subviews.count()).find_map(|index| find_switch(&subviews.objectAtIndex(index)))
}

/// Print the verdict and exit. The only place that decides, so the stall path and the complete run
/// cannot describe the same counters differently.
///
/// A short run reports the frames it reached **and** the frames both arms need, because that is the
/// difference between a failure a reader can act on and a bare refusal — the same reason
/// `z_order` says "painted fewer than 6 times" and `glass_hit` says "fewer than 4 frames within 12
/// tries" instead of just failing.
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

/// The stall guard, on a thread of its own and deliberately **not** inside `render`.
///
/// It used to be a `started.elapsed() > 15.` test inside `render`, which is the one place that
/// cannot run once the window stops painting: the guard sat inside the thing it was guarding
/// against, so the case it named was the case it could not see. Measured under a locked session,
/// where `loginwindow` covers the display and GPUI stops its `CVDisplayLink`, only two renders are
/// reachable — and all three arms then hung with no output at all and had to be killed from
/// outside. The comment in `scripts/check.sh` had already flagged that guard as untested; it was
/// untested because it was unreachable.
///
/// A thread that only sleeps has nothing to starve it: no display link, no runloop turn, no `App`
/// borrow to wait on. It costs one thread for the length of the run.
///
/// The fix is **not** to pump frames from inside the probe, tempting though it is: calling
/// `setNeedsDisplay` on GPUI's Metal layer each turn drives its `displayLayer:` path, which runs
/// with the display link stopped and would make this step pass under a locked session. But that
/// path turns `CAMetalLayer.presentsWithTransaction` on and presents inside the running
/// `CATransaction`, so `frame_lag` returns 0 and hosted placement is written straight through
/// instead of queued. The probe would then be exercising a different presentation path from the
/// shipped one, and a green gate that tests the wrong path is worse than a blocked one.
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
    #[allow(refining_impl_trait)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        window.request_animation_frame();
        let frame = self.counters.frame.fetch_add(1, Ordering::Relaxed) + 1;

        // NGD_SABOTAGE proves each arm can fail: `nohandler` never registers one, so the ARMED arm
        // must report VOID; `noclear` never clears it, so the CLEARED arm must report FAIL. A probe
        // whose arms both pass unconditionally certifies nothing.
        let sabotage = std::env::var("NGD_SABOTAGE").unwrap_or_default();

        if frame == CLEAR && sabotage != "noclear" {
            self.armed = false;
        }
        if sabotage == "nohandler" {
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

        let handler = self.armed;
        let mut switch = native_switch("probe", true);
        if handler {
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
        .into_any_element()
    }
}

fn main() {
    let counters = Arc::new(Counters::default());
    let started = Instant::now();
    // Armed before the window exists, so a window that never paints at all is still reported.
    watchdog(counters.clone(), started);
    Application::new().run(move |cx: &mut App| {
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
                    armed: true,
                    counters: counters.clone(),
                    started,
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
