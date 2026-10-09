//! Gate: `commit` leaves each container's `subviews` array and the key loop equal to paint order.
//! Exits 0 pass, 1 fail.

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui::{
    App, AsyncApp, Bounds, Context, Render, Window, WindowBounds, div, point, prelude::*, px, size,
};
use native_gpui::{Layer, NativeHost, native_root, window_options};
use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{NSSwitch, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize};

const FIRST: u64 = 101;
const MIDDLE: u64 = 102;
const LAST: u64 = 103;

struct Phase {
    what: &'static str,
    order: [u64; 3],
    middle_is_switch: bool,
}

const SCRIPT: [Phase; 5] = [
    Phase {
        what: "first frame",
        order: [FIRST, MIDDLE, LAST],
        middle_is_switch: false,
    },
    Phase {
        what: "middle view replaced",
        order: [FIRST, MIDDLE, LAST],
        middle_is_switch: true,
    },
    Phase {
        what: "middle view replaced back",
        order: [FIRST, MIDDLE, LAST],
        middle_is_switch: false,
    },
    Phase {
        what: "pure reorder",
        order: [LAST, MIDDLE, FIRST],
        middle_is_switch: false,
    },
    Phase {
        what: "nothing changed",
        order: [LAST, MIDDLE, FIRST],
        middle_is_switch: false,
    },
];

struct Reading {
    stack: Vec<u64>,
    chain: Vec<u64>,
}

struct Probe {
    phase: usize,
    readings: Vec<Reading>,
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let host = NativeHost::get(window, cx);
        if self.phase > 0 {
            self.readings.push(Reading {
                stack: host.stacking(Layer::Above),
                chain: host.key_loop(),
            });
        }

        if let Some(phase) = SCRIPT.get(self.phase) {
            let mask = Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(4000.), px(4000.)),
            };
            for (row, &key) in phase.order.iter().enumerate() {
                let bounds = Bounds {
                    origin: point(px(20.), px(20. + 40. * row as f32)),
                    size: size(px(60.), px(24.)),
                };
                if key == MIDDLE && phase.middle_is_switch {
                    let _: Retained<NSSwitch> =
                        host.place(key, Layer::Above, bounds, mask, |mtm, _| NSSwitch::new(mtm));
                } else {
                    let _: Retained<NSView> =
                        host.place(key, Layer::Above, bounds, mask, |mtm, _| {
                            NSView::initWithFrame(
                                NSView::alloc(mtm),
                                NSRect::new(NSPoint::ZERO, NSSize::new(24., 24.)),
                            )
                        });
                }
            }
        }

        self.phase += 1;
        if self.phase > SCRIPT.len() {
            report(&self.readings);
        }

        native_root(div().size_full())
    }
}

static FAILED: AtomicBool = AtomicBool::new(false);
static REPORTED: AtomicBool = AtomicBool::new(false);

fn keys(list: &[u64]) -> String {
    list.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

fn report(readings: &[Reading]) {
    REPORTED.store(true, Ordering::Relaxed);
    println!(
        "{:<26} {:<16} {:<16} {:<16} VERDICT",
        "PHASE", "PAINT ORDER", "SUBVIEWS", "KEY LOOP"
    );
    for (phase, reading) in SCRIPT.iter().zip(readings) {
        let stacked = reading.stack == phase.order;
        let chained = reading.chain == phase.order;
        if !stacked || !chained {
            FAILED.store(true, Ordering::Relaxed);
        }
        println!(
            "{:<26} {:<16} {:<16} {:<16} {}",
            phase.what,
            keys(&phase.order),
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
}

fn main() -> ExitCode {
    gpui_platform::application().run(|cx: &mut App| {
        let window = cx
            .open_window(
                gpui::WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(64.), px(48.)),
                    })),
                    // gpui starts its display link only once occlusion reports visible.
                    focus: false,
                    ..window_options()
                },
                |_, cx| {
                    cx.new(|_| Probe {
                        phase: 0,
                        readings: Vec::new(),
                    })
                },
            )
            .unwrap();

        // A `notify` raised during render is swallowed.
        cx.spawn(async move |cx: &mut AsyncApp| {
            for _ in 0..SCRIPT.len() + 4 {
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                let open = window.update(cx, |_, _, cx| cx.notify()).is_ok();
                if !open || REPORTED.load(Ordering::Relaxed) {
                    break;
                }
            }
            if !REPORTED.load(Ordering::Relaxed) {
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
