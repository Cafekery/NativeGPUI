use std::io::Write;

use gpui::{
    App, Bounds, Context, Div, Render, Window, WindowBounds, div, point, prelude::*, px, rgb, rgba,
    size,
};
use objc2::rc::Retained;
use objc2_app_kit::{NSScreen, NSView, NSWindow};
use objc2_foundation::MainThreadMarker;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use native_gpui::{
    GlassInteractionState, GlassSubvariant, GlassVariant, glass, native_root, window_options,
};

const STRIPE: f32 = 8.;
const BAND: f32 = 10.;
const STRIPE_COLORS: [u32; 5] = [0xff375f, 0xffd60a, 0x30d158, 0x0a84ff, 0xf2f2f7];

const PITCH_X: f32 = 160.;
const PITCH_Y: f32 = 120.;
const MARGIN_X: f32 = 96.;
const MARGIN_Y: f32 = 100.;
const INSET_X: f32 = 16.;
const INSET_Y: f32 = 16.;
const COLS: usize = 5;
const ROWS: usize = 5;

const W: f32 = 2. * MARGIN_X + COLS as f32 * PITCH_X;
const H: f32 = 2. * MARGIN_Y + ROWS as f32 * PITCH_Y;

enum Slot {
    Bare,
    Public { clear: bool },
    Variant(GlassVariant),
    Interaction(GlassInteractionState),
    Subdued,
    Scrim,
    Lensing,
}

fn slots() -> Vec<(&'static str, Slot)> {
    let mut slots = vec![("bare-0", Slot::Bare)];
    slots.extend(
        GlassVariant::ALL
            .iter()
            .zip(VARIANT_KEYS)
            .map(|(&variant, key)| (key, Slot::Variant(variant))),
    );
    slots.extend([
        ("bare-1", Slot::Bare),
        ("public-regular", Slot::Public { clear: false }),
        ("public-clear", Slot::Public { clear: true }),
        ("bare-2", Slot::Bare),
        (
            "interactive",
            Slot::Interaction(GlassInteractionState::Interactive),
        ),
        ("pressed", Slot::Interaction(GlassInteractionState::Pressed)),
        ("subdued", Slot::Subdued),
        ("scrim", Slot::Scrim),
        ("lensing", Slot::Lensing),
        ("bare-3", Slot::Bare),
        ("bare-4", Slot::Bare),
    ]);
    slots
}

const _: () = assert!(VARIANT_KEYS.len() == GlassVariant::ALL.len());

const VARIANT_KEYS: [&str; 13] = [
    "automatic",
    "clear",
    "dock",
    "appicons",
    "widgets",
    "avplayer",
    "notificationcenter",
    "monogram",
    "bubbles",
    "keyboard",
    "sidebar",
    "slider",
    "cartouchepopover",
];

struct Catalogue {
    phase: usize,
    quit_at: Option<u64>,
    frame: u64,
    announced: bool,
}

fn backdrop(phase: usize) -> Div {
    let shift = phase as f32 * 5.;
    let stripes = (W / (2. * STRIPE)).ceil() as usize + 1;
    let bands = (H / (2. * BAND)).ceil() as usize + 1;
    div()
        .absolute()
        .left_0()
        .top_0()
        .size_full()
        .bg(rgb(0x080808))
        .children((0..stripes).map(move |i| {
            div()
                .absolute()
                .left(px(i as f32 * 2. * STRIPE - shift))
                .top_0()
                .w(px(STRIPE))
                .h_full()
                .bg(rgb(STRIPE_COLORS[(i + phase) % STRIPE_COLORS.len()]))
        }))
        .children((0..bands).map(|i| {
            div()
                .absolute()
                .left_0()
                .top(px(i as f32 * 2. * BAND))
                .w_full()
                .h(px(BAND))
                .bg(rgba(0xffffff4d))
        }))
}

impl Render for Catalogue {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame += 1;
        if !self.announced {
            self.announced = true;
            self.announce(window);
        }
        if let Some(quit_at) = self.quit_at {
            let ns = ns_window(window);
            println!(
                "GLASS-FRAME {} key={} main={}",
                self.frame,
                ns.isKeyWindow(),
                ns.isMainWindow(),
            );
            std::io::stdout().flush().ok();
            if self.frame >= quit_at {
                cx.quit();
            }
            window.request_animation_frame();
        }

        let panels = slots().into_iter().enumerate().map(|(index, (key, slot))| {
            let (x, y) = cell_origin(index);
            let panel = div()
                .absolute()
                .left(px(x + INSET_X))
                .top(px(y + INSET_Y))
                .w(px(PITCH_X - 2. * INSET_X))
                .h(px(PITCH_Y - 2. * INSET_Y));
            match slot {
                Slot::Bare => panel,
                Slot::Public { clear } => panel.child(glass(key).clear(clear)),
                Slot::Variant(variant) => panel.child(
                    glass(key)
                        .variant(variant)
                        .subvariant(GlassSubvariant::None),
                ),
                Slot::Interaction(state) => panel.child(glass(key).interaction_state(state)),
                Slot::Subdued => panel.child(glass(key).subdued(true)),
                Slot::Scrim => panel.child(glass(key).scrim(true)),
                Slot::Lensing => panel.child(glass(key).content_lensing(true)),
            }
        });

        native_root(
            div()
                .relative()
                .size_full()
                .child(backdrop(self.phase))
                .children(panels),
        )
    }
}

fn cell_origin(index: usize) -> (f32, f32) {
    let (col, row) = (index % COLS, index / COLS);
    (
        MARGIN_X + col as f32 * PITCH_X,
        MARGIN_Y + row as f32 * PITCH_Y,
    )
}

impl Catalogue {
    fn announce(&self, window: &Window) {
        let ns = ns_window(window);
        let frame = ns.frame();
        let primary = NSScreen::screens(MainThreadMarker::new().expect("main thread"))
            .firstObject()
            .expect("there is always a display");
        let cg_y = primary.frame().size.height - (frame.origin.y + frame.size.height);
        println!(
            "GLASS-CAT window={} pid={} phase={} w={:.0} h={:.0} scale={:.1} cgx={:.0} cgy={:.0}",
            ns.windowNumber(),
            std::process::id(),
            self.phase,
            frame.size.width,
            frame.size.height,
            ns.backingScaleFactor(),
            frame.origin.x,
            cg_y,
        );
        for (index, (key, _)) in slots().into_iter().enumerate() {
            let (x, y) = cell_origin(index);
            println!(
                "GLASS-CELL {key} x={:.0} y={:.0} w={:.0} h={:.0}",
                x + INSET_X,
                y + INSET_Y,
                PITCH_X - 2. * INSET_X,
                PITCH_Y - 2. * INSET_Y,
            );
        }
        for variant in GlassVariant::ALL {
            let accepted: Vec<&str> = variant
                .subvariants()
                .iter()
                .map(|subvariant| subvariant.title())
                .collect();
            println!(
                "GLASS-VARIANT raw={} title={:?} subvariants={}",
                variant.raw(),
                variant.title(),
                accepted.join("|"),
            );
        }
        for subvariant in GlassSubvariant::ALL {
            println!(
                "GLASS-SUBVARIANT api={:?} title={:?}",
                subvariant.api_name(),
                subvariant.title(),
            );
        }
        for state in GlassInteractionState::ALL {
            println!(
                "GLASS-INTERACTION raw={} title={:?}",
                state.raw(),
                state.title(),
            );
        }
        std::io::stdout().flush().ok();
    }
}

fn ns_window(window: &Window) -> Retained<NSWindow> {
    let handle = HasWindowHandle::window_handle(window).expect("gpui window has no native handle");
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        unreachable!("macOS-only example saw a non-AppKit window handle");
    };
    let view = unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }
        .expect("gpui ns_view was null");
    view.window().expect("gpui's view is always in a window")
}

fn main() {
    let phase = std::env::var("NG_PHASE")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    let quit_at = std::env::var("NG_QUIT")
        .ok()
        .and_then(|value| value.trim().parse().ok());

    gpui_platform::application().run(move |cx: &mut App| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(1000.), px(120.)),
                    size: size(px(W), px(H)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|_| Catalogue {
                    phase,
                    quit_at,
                    frame: 0,
                    announced: false,
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
