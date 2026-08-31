//! Whether `commit` leaves each container's `subviews` array equal to paint order.
//!
//! Every verdict is read in-process out of AppKit, through [`NativeHost::stacking`] and
//! [`NativeHost::key_loop`], so nothing here needs a screenshot or the keyboard. Exits non-zero
//! naming the first phase whose order is wrong.
//!
//! It cannot be a unit test: libtest runs every test body on a spawned thread, where
//! `MainThreadMarker::new()` is `None` and no `NSView` can honestly be built — and the observable
//! is AppKit's own array, which a model of it could not have caught.
//!
//! The phases that matter are the ones where paint order is byte-identical between frames and a
//! view is replaced anyway. `place` is called directly with fixed keys rather than through a
//! component, so the printed arrays read as paint order.

use std::process::ExitCode;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use gpui::{
    App, Application, AsyncApp, Bounds, Context, Render, Window, WindowBounds, div, point,
    prelude::*, px, size,
};
use native_gpui::{Layer, NativeHost, native_root, window_options};
use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{NSSwitch, NSView};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

const FIRST: u64 = 101;
const MIDDLE: u64 = 102;
const LAST: u64 = 103;

/// What the middle key's view is this frame. Its pool key and its position in paint order are
/// identical either way, so changing it is exactly `place_in`'s stale branch: dropped and rebuilt
/// under an unchanged key, at an unchanged place in paint order.
#[derive(Copy, Clone, PartialEq)]
enum Middle {
    Plain,
    Switch,
}

struct Phase {
    what: &'static str,
    order: [u64; 3],
    middle: Middle,
}

const SCRIPT: [Phase; 5] = [
    Phase {
        what: "first frame",
        order: [FIRST, MIDDLE, LAST],
        middle: Middle::Plain,
    },
    Phase {
        what: "middle view replaced",
        order: [FIRST, MIDDLE, LAST],
        middle: Middle::Switch,
    },
    Phase {
        what: "middle view replaced back",
        order: [FIRST, MIDDLE, LAST],
        middle: Middle::Plain,
    },
    Phase {
        what: "pure reorder",
        order: [LAST, MIDDLE, FIRST],
        middle: Middle::Plain,
    },
    Phase {
        what: "nothing changed",
        order: [LAST, MIDDLE, FIRST],
        middle: Middle::Plain,
    },
];

struct Reading {
    phase: usize,
    stack: Vec<u64>,
    chain: Vec<u64>,
}

struct Probe {
    /// Which phase this frame paints. One past the script means the run is over.
    phase: usize,
    readings: Vec<Reading>,
    done: bool,
}

fn plain(mtm: MainThreadMarker) -> Retained<NSView> {
    NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::ZERO, NSSize::new(24., 24.)),
    )
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The previous phase's `commit` has run and this frame has placed nothing yet, so this is
        // that phase's settled answer.
        if self.phase > 0 {
            let host = NativeHost::get(window, cx);
            self.readings.push(Reading {
                phase: self.phase - 1,
                stack: host.stacking(Layer::Above),
                chain: host.key_loop(),
            });
        }

        if let Some(phase) = SCRIPT.get(self.phase) {
            let mask = Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(4000.), px(4000.)),
            };
            let host = NativeHost::get(window, cx);
            for (row, key) in phase.order.iter().enumerate() {
                let bounds = Bounds {
                    origin: point(px(20.), px(20. + 40. * row as f32)),
                    size: size(px(60.), px(24.)),
                };
                if *key == MIDDLE && phase.middle == Middle::Switch {
                    let _: Retained<NSSwitch> =
                        host.place(*key, Layer::Above, bounds, mask, |mtm, _| {
                            NSSwitch::new(mtm)
                        });
                } else {
                    let _: Retained<NSView> =
                        host.place(*key, Layer::Above, bounds, mask, |mtm, _| plain(mtm));
                }
            }
        }

        self.phase += 1;
        if self.phase > SCRIPT.len() {
            report(&self.readings);
            self.done = true;
        }

        native_root(div().size_full())
    }
}

static FAILED: AtomicU8 = AtomicU8::new(0);
/// Set by [`report`]. Without it, a run that never reached the last phase — because the window
/// only ever painted once — exits 0 with no output at all, and the gate reads that as a pass
/// having tested nothing.
static REPORTED: AtomicU8 = AtomicU8::new(0);

fn keys(list: &[u64]) -> String {
    list.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

fn report(readings: &[Reading]) {
    REPORTED.store(1, Ordering::Relaxed);
    println!(
        "{:<26} {:<16} {:<16} {:<16} {}",
        "PHASE", "PAINT ORDER", "SUBVIEWS", "KEY LOOP", "VERDICT"
    );
    for reading in readings {
        let phase = &SCRIPT[reading.phase];
        let want = phase.order.to_vec();
        let stacked = reading.stack == want;
        let chained = reading.chain == want;
        if !stacked || !chained {
            FAILED.store(1, Ordering::Relaxed);
        }
        println!(
            "{:<26} {:<16} {:<16} {:<16} {}",
            phase.what,
            keys(&want),
            keys(&reading.stack),
            keys(&reading.chain),
            match (stacked, chained) {
                (true, true) => "ok",
                (false, true) => "WRONG STACK",
                (true, false) => "WRONG KEY LOOP",
                (false, false) => "WRONG STACK + KEY LOOP",
            }
        );
    }
    if readings.len() != SCRIPT.len() {
        println!("only {} of {} phases ran", readings.len(), SCRIPT.len());
        FAILED.store(1, Ordering::Relaxed);
    }
}

fn main() -> ExitCode {
    Application::new().run(|cx: &mut App| {
        let window = cx
            .open_window(
                gpui::WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(64.), px(48.)),
                    })),
                    // gpui starts its display link only once the window's occlusion state reports
                    // visible, so `show: false` never renders a frame. Hence a 64x48 corner window
                    // and `focus: false`: the smallest thing that gets frames, taking the keyboard
                    // away from nothing.
                    focus: false,
                    ..window_options()
                },
                |_, cx| {
                    cx.new(|_| Probe {
                        phase: 0,
                        readings: Vec::new(),
                        done: false,
                    })
                },
            )
            .unwrap();

        // One frame per phase, asked for from outside `render`: a `notify` raised *during* render
        // is swallowed, so a self-driving tree renders exactly once.
        cx.spawn(async move |cx: &mut AsyncApp| {
            for _ in 0..SCRIPT.len() + 4 {
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                let done = window
                    .update(cx, |probe, _, cx| {
                        cx.notify();
                        probe.done
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
            // `Application::run` returning exits 0 regardless, so the verdict has to leave here.
            // Fail closed: the loop also ends by running out of tries, and a window that painted
            // once — an occluded window, or a machine whose display link has stalled — reaches
            // neither the last phase nor `report`, so there is nothing to be right about.
            if REPORTED.load(Ordering::Relaxed) == 0 {
                println!(
                    "FAIL: no phase was reported — the window painted fewer than {} times, so no \
                     paint order was ever read",
                    SCRIPT.len() + 1
                );
                std::process::exit(1);
            }
            std::process::exit(FAILED.load(Ordering::Relaxed) as i32);
        })
        .detach();
    });
    ExitCode::SUCCESS
}
