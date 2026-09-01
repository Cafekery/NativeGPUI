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
//! Exit codes, so a gate can use it: **0** pass, **1** fail, **90** void — the run proved nothing.
//! `NGD_SABOTAGE` proves each arm can fail rather than asserting it:
//!
//! ```text
//! cargo run --release --example ngdispatch                      # 0, verdict=PASS
//! NGD_SABOTAGE=nohandler cargo run --release --example ngdispatch  # 90, verdict=VOID
//! NGD_SABOTAGE=noclear   cargo run --release --example ngdispatch  # 1,  verdict=FAIL
//! ```

use std::time::Instant;

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

struct Probe {
    frame: u32,
    armed: bool,
    fires: u32,
    at_click_one: u32,
    verdict: Option<(u32, u32)>,
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

impl Render for Probe {
    #[allow(refining_impl_trait)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        window.request_animation_frame();
        self.frame += 1;

        // NGD_SABOTAGE proves each arm can fail: `nohandler` never registers one, so the ARMED arm
        // must report VOID; `noclear` never clears it, so the CLEARED arm must report FAIL. A probe
        // whose arms both pass unconditionally certifies nothing.
        let sabotage = std::env::var("NGD_SABOTAGE").unwrap_or_default();

        // Frame 46 onward paints the switch with no handler, which is the clearing path.
        if self.frame == 46 && sabotage != "noclear" {
            self.armed = false;
        }
        if sabotage == "nohandler" {
            self.armed = false;
        }

        if self.frame == 30 || self.frame == 60 {
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

        // Read 15 frames after each click: handlers are dispatched on a later turn, never inline.
        if self.frame == 45 {
            self.at_click_one = self.fires;
        }
        // A window that stops receiving display ticks — occluded, or after a display sleep — never
        // reaches frame 75, and a probe that hangs is worse than one that fails. Whichever comes
        // first, the frame count is reported, so a short run cannot read as a clean one.
        let stalled = self.started.elapsed().as_secs_f64() > 15.;
        if (self.frame == 75 || stalled) && self.verdict.is_none() {
            let cleared = self.fires - self.at_click_one;
            let armed = self.at_click_one;
            self.verdict = Some((armed, cleared));
            let (verdict, code) = if self.frame < 75 {
                ("VOID (window stopped painting before both arms ran)", 90)
            } else if armed == 0 {
                ("VOID (armed arm never fired — nothing was listening)", 90)
            } else if cleared == 0 {
                ("PASS", 0)
            } else {
                ("FAIL", 1)
            };
            println!(
                "NG-DISPATCH frames={} t={:.2}s armed_fires={armed} cleared_fires={cleared} \
                 verdict={verdict}",
                self.frame,
                self.started.elapsed().as_secs_f64(),
            );
            use std::io::Write;
            std::io::stdout().flush().ok();
            std::process::exit(code);
        }

        let handler = self.armed;
        let mut switch = native_switch("probe", true);
        if handler {
            switch = switch.on_change(cx.listener(|this: &mut Probe, _: &bool, _, _| {
                this.fires += 1;
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
    Application::new().run(|cx: &mut App| {
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
                    frame: 0,
                    armed: true,
                    fires: 0,
                    at_click_one: 0,
                    verdict: None,
                    started: Instant::now(),
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
