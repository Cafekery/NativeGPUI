//! Every component in one window, plus native controls inside a GPUI scroll container.
//!
//! The integration arm the per-component examples cannot cover: one [`NativeHost`] pool, one
//! commit pass, mixed native classes, and a scrolling list where views are pooled and clipped.
//! GPUI paints only visible rows, so only visible rows place a view — the pool count in the
//! header is the observable that proves it.
//!
//! `NG_ROWS` sets the list length (default 400). `NG_SKEW=<points-per-frame>` replaces the whole
//! window with the scroll-skew rig at the bottom of this file.

use std::panic::Location;
use std::time::Instant;

use gpui::{
    AnyElement, App, Application, Bounds, Context, Element, ElementId, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Pixels, Render, SharedString, Style,
    UniformListScrollHandle, Window, WindowBounds, div, point, prelude::*, px, rgb, size,
    uniform_list,
};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSBezierPath, NSColor, NSScreen, NSView, NSWindow};
use objc2_foundation::{MainThreadMarker, NSRect};
use objc2_quartz_core::CALayer;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use native_gpui::tokens::{SystemColor, system_color};
use native_gpui::{
    Layer, Material, NativeHost, glass, material, native_checkbox, native_color_well, native_popup,
    native_progress, native_root, native_segmented, native_slider, native_spinner, native_switch,
    native_text_field, tokens, window_options,
};

struct Row {
    on: bool,
    level: f64,
}

struct Gallery {
    started: Instant,
    scroll: UniformListScrollHandle,
    rows: Vec<Row>,
    switch: bool,
    slider: f64,
    checked: bool,
    segment: usize,
    choice: usize,
    color: Hsla,
    text: String,
    /// `Some(v)` puts the window in skew-measurement mode at `v` logical points per frame.
    skew: Option<f32>,
    skew_offset: f32,
    skew_frames: u64,
    skew_announced: bool,
    /// Frame intervals in ms, so a smaller skew bought by dropping frames cannot read as a win.
    skew_last: Option<Instant>,
    skew_intervals: Vec<f64>,
    skew_key: Option<bool>,
    skew_key_frames: u64,
}

impl Gallery {
    fn controls(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = system_color(SystemColor::Label);
        let row = |children: Vec<gpui::AnyElement>| {
            div()
                .flex()
                .items_center()
                .gap_4()
                .h(px(40.))
                .children(children)
        };

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .w(px(420.))
            .text_color(label)
            .child(row(vec![
                div().w(px(96.)).child("switch").into_any_element(),
                native_switch("sw", self.switch)
                    .on_change(cx.listener(|this, on: &bool, _, cx| {
                        this.switch = *on;
                        cx.notify();
                    }))
                    .into_any_element(),
                native_checkbox("cb", self.checked)
                    .title("checkbox")
                    .on_change(cx.listener(|this, state: &native_gpui::CheckState, _, cx| {
                        this.checked = *state == native_gpui::CheckState::On;
                        cx.notify();
                    }))
                    .into_any_element(),
            ]))
            .child(row(vec![
                div().w(px(96.)).child("slider").into_any_element(),
                div()
                    .w(px(200.))
                    .child(
                        native_slider("sl", self.slider)
                            .range(0., 1.)
                            .on_change(cx.listener(|this, value: &f64, _, cx| {
                                this.slider = *value;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
                div()
                    .w(px(64.))
                    .child(SharedString::from(format!("{:.2}", self.slider)))
                    .into_any_element(),
            ]))
            .child(row(vec![
                div().w(px(96.)).child("segmented").into_any_element(),
                native_segmented("seg", self.segment)
                    .segments(["One", "Two", "Three"])
                    .on_change(cx.listener(|this, index: &usize, _, cx| {
                        this.segment = *index;
                        cx.notify();
                    }))
                    .into_any_element(),
            ]))
            .child(row(vec![
                div().w(px(96.)).child("popup").into_any_element(),
                native_popup("pop", self.choice)
                    .items(["Graphite", "Blue", "Purple"])
                    .on_change(cx.listener(|this, index: &usize, _, cx| {
                        this.choice = *index;
                        cx.notify();
                    }))
                    .into_any_element(),
                native_color_well("well", self.color)
                    .on_change(cx.listener(|this, color: &Hsla, _, cx| {
                        this.color = *color;
                        cx.notify();
                    }))
                    .into_any_element(),
            ]))
            .child(row(vec![
                div().w(px(96.)).child("text").into_any_element(),
                div()
                    .w(px(220.))
                    .child(
                        native_text_field("tf", &self.text)
                            .placeholder("type here")
                            .on_change(cx.listener(|this, text: &String, _, cx| {
                                this.text = text.clone();
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            ]))
            .child(row(vec![
                div().w(px(96.)).child("progress").into_any_element(),
                div()
                    .w(px(180.))
                    .child(native_progress("pr", self.slider))
                    .into_any_element(),
                native_spinner("spin").into_any_element(),
            ]))
    }

    /// `uniform_list` is load-bearing, not a style choice: it paints only the visible range, so
    /// the pool stays in the tens however long the list is. A `div().overflow_y_scroll()` with the
    /// same children paints every row — 811 pooled views for 400 rows, past the comfortable budget
    /// for switches and far past it for sliders.
    fn scroll_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = system_color(SystemColor::Label);
        let separator = system_color(SystemColor::Separator);
        let states: Vec<(bool, f64)> = self.rows.iter().map(|row| (row.on, row.level)).collect();
        let entity = cx.entity().downgrade();

        uniform_list("list", states.len(), move |range, _, _| {
            range
                .map(|index| {
                    let (on, level) = states[index];
                    let entity = entity.clone();
                    div()
                        .flex()
                        .items_center()
                        .gap_4()
                        .h(px(44.))
                        .px_4()
                        .border_b_1()
                        .border_color(separator)
                        .text_color(label)
                        .child(
                            div()
                                .w(px(80.))
                                .child(SharedString::from(format!("row {index:03}"))),
                        )
                        .child(native_switch(("row-sw", index), on).on_change(
                            move |on: &bool, _, cx| {
                                let on = *on;
                                entity
                                    .update(cx, |this: &mut Gallery, cx| {
                                        this.rows[index].on = on;
                                        cx.notify();
                                    })
                                    .ok();
                            },
                        ))
                        .child(
                            div()
                                .w(px(140.))
                                .child(native_slider(("row-sl", index), level).range(0., 1.)),
                        )
                })
                .collect::<Vec<_>>()
        })
        .flex_1()
        .track_scroll(self.scroll.clone())
    }
}

impl Render for Gallery {
    #[allow(refining_impl_trait)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Unconditional so the commit pass is exercised every frame, not only on input.
        window.request_animation_frame();

        if self.skew.is_some() {
            return self.skew_frame(window, cx);
        }

        let pooled = NativeHost::get(window, cx).len();
        let elapsed = self.started.elapsed().as_secs_f64();
        let header = format!(
            "pooled native views: {pooled}  ·  rows: {}  ·  t {elapsed:5.1}s",
            self.rows.len()
        );

        native_root(
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    material("bg", Material::WindowBackground)
                        .absolute()
                        .inset_0(),
                )
                .child(
                    div()
                        .h(px(44.))
                        .px_4()
                        .flex()
                        .items_center()
                        .text_color(system_color(SystemColor::SecondaryLabel))
                        .child(SharedString::from(header)),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .overflow_hidden()
                        .child(self.controls(cx))
                        .child(
                            div()
                                .w(px(24.))
                                .h_full()
                                .flex()
                                .child(glass("edge").corner_radius(px(12.))),
                        )
                        .child(self.scroll_list(cx)),
                ),
        )
        .into_any_element()
    }
}

fn main() {
    let skew = std::env::var("NG_SKEW")
        .ok()
        .and_then(|value| value.trim().parse::<f32>().ok());
    let rows: usize = std::env::var("NG_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        // The rig scrolls one way forever; enough rows that a long run never clamps and stops.
        .unwrap_or(if skew.is_some() { 40_000 } else { 400 });

    // `NG_ORIGIN=x,y` places the window somewhere other than the default. The rig needs its window
    // actually scanned out: a capture of a window some other app's fullscreen Space occludes reads
    // a re-composite rather than the presented frame, which is precisely the quantity measured.
    let origin = std::env::var("NG_ORIGIN").ok().and_then(|value| {
        let (x, y) = value.split_once(',')?;
        Some(point(
            px(x.trim().parse().ok()?),
            px(y.trim().parse().ok()?),
        ))
    });
    Application::new().run(move |cx: &mut App| {
        tokens::init(cx);
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: origin.unwrap_or(point(px(180.), px(140.))),
                    size: size(px(1000.), px(660.)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|_| Gallery {
                    started: Instant::now(),
                    scroll: UniformListScrollHandle::new(),
                    rows: (0..rows)
                        .map(|index| Row {
                            on: index % 3 == 0,
                            level: (index % 10) as f64 / 10.,
                        })
                        .collect(),
                    switch: true,
                    slider: 0.35,
                    checked: false,
                    segment: 1,
                    choice: 0,
                    color: rgb(0x0a84ff).into(),
                    text: String::new(),
                    skew,
                    skew_offset: 0.,
                    skew_frames: 0,
                    skew_announced: false,
                    skew_last: None,
                    skew_intervals: Vec::new(),
                    skew_key: None,
                    skew_key_frames: 0,
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

// Scroll-skew rig — `NG_SKEW=<points-per-frame>`
//
// Does a hosted AppKit view drift vertically from the GPUI row it belongs to while scrolling? A
// settled screenshot cannot show a during-motion artefact, so the scroll is driven *in process* —
// the list offset advances a fixed number of points per rendered frame — and never by synthetic
// input: every capture taken while the app runs is mid-motion, captures cannot race an input
// driver, and the velocity is a number this file sets rather than one recovered from the pixels.
// Skew in pixels is uninterpretable: with velocity `v` and measured offset `d` the lead is `d / v`
// *frames*, and sweeping `v` separates the hypotheses — a presentation-order skew holds the frame
// count constant while the pixel offset scales with `v`, a layout offset does the reverse.
//
// Each marked row carries four features at a known vertical relationship, all locatable from one
// frame, left to right: a GPUI-painted magenta band (the reference), a hosted marker at the same
// top (the arm under test), a hosted marker at `+6.00 pt` (the known-positive fixture), and a real
// `NSSwitch` (corroboration with a control a user would touch). The fixture is what disqualifies
// the instrument — same class, same [`NativeHost::place`] call, differing only by an offset known
// in advance — so if the edge finder cannot read `+6.00 pt` off it, its null for the arm under test
// means nothing. Nothing else in the window paints, so features are found by scanning for
// non-background columns; each column found must then be *validated* against the x this file placed
// it at, because scanning alone re-indexes silently. With the load arm present, occluding only the
// reference column still leaves four runs, and taking the leftmost four reports the fixture's own
// `+6.00 pt` — plausible rather than obviously garbage — where the truth was `-24.00`.
//
// What it found: a hosted view runs exactly two frames ahead of its row at every velocity, the
// offset always an exact integer multiple of `v` and never a fraction, so this is presentation
// order and not a layout offset. Rest reads exactly `0.000 pt`, and the switch stays locked to the
// marker, so a toggle leads its own row by the same two frames.
//
// It is not GPUI's layout that is late but its *presented* frame, and the two is a drawable-queue
// depth rather than a race: the Metal surface reaching the screen is up to
// `CAMetalLayer.maximumDrawableCount - 1` frames behind the `CATransaction` that carried the same
// layout's `setFrame`. Setting that to 2 halves the lead exactly as the queue theory predicts.
// 1 is unreachable — Core Animation's floor is 2, and asking for 1 makes `nextDrawable`
// unsatisfiable and panics GPUI out of its own entity map before the first frame — so one frame of
// lead is irreducible from here. The crate leaves the property at the default anyway, because two
// buffers roughly doubles the share of frames over 1.5x the median interval under load: the median
// stays the display period, the whole cost is in the tail, which is what less slack to absorb a
// hiccup predicts. Holding hosted geometry back N frames also subtracts exactly N, and is likewise
// not done: it would leave geometry two frames stale for every change that is *not* a
// constant-velocity scroll — a control appearing, a resize, a jump.
//
// One trap, because it produced a clean-looking wrong answer first: a window capture of an
// *occluded* window reads a re-composite from current `CALayer` state, which collapses the very gap
// being measured. `-l` is true about content and false about timing. The gate is requiring `-l` and
// `-R` to agree per capture, which they do to the point on a display the window is scanned out on.

/// Row pitch. Only every `SKEW_STRIDE`-th row is marked, so marked rows sit 176 pt apart — far
/// more than any plausible skew, so a band can never pair with the neighbouring row's view.
const SKEW_ROW_H: f32 = 44.;
const SKEW_STRIDE: usize = 4;

/// Feature geometry within a marked row, logical points.
const MARK_TOP: f32 = 14.;
const MARK_W: f32 = 56.;
const MARK_H: f32 = 8.;
/// The fixture's deliberate offset: unmistakable, yet too small to read as a neighbouring row.
const POSITIVE_SHIFT: f32 = 6.;

const COL_GPUI: f32 = 40.;
const COL_NATIVE: f32 = 140.;
const COL_POSITIVE: f32 = 240.;
const COL_SWITCH: f32 = 340.;
/// `NG_SKEW_LOAD` controls, far enough right of the switch column to form their own column runs.
const COL_LOAD: f32 = 500.;

/// Deliberately not black: the sub-pixel edge finder projects each pixel onto the
/// background-to-feature axis, and a background clamped at zero on every channel would make an
/// anti-aliased edge row indistinguishable from a slightly dim feature.
const SKEW_BG: u32 = 0x18181c;

struct MarkerIvars {
    color: Retained<NSColor>,
}

define_class!(
    /// A hosted view that is nothing but a solid rectangle.
    ///
    /// Every real control has a bezel, a corner radius or a highlight rim, so its "own edge" is a
    /// judgement call. This has one hard horizontal edge whose only softness is the anti-aliasing
    /// of its own sub-pixel frame origin, which is the quantity being measured. It is placed
    /// through the shipped [`NativeHost::place`] path, so it moves when a real control moves.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSkewMarker"]
    #[ivars = MarkerIvars]
    struct Marker;

    impl Marker {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.ivars().color.setFill();
            NSBezierPath::fillRect(self.bounds());
        }
    }
);

impl Marker {
    fn new(mtm: MainThreadMarker, rgb: (f64, f64, f64)) -> Retained<Self> {
        // sRGB, not device RGB: `colorWithRed:` is a different colour on a P3 display.
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(rgb.0, rgb.1, rgb.2, 1.);
        let this = Self::alloc(mtm).set_ivars(MarkerIvars { color });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        // Its own layer, so a frame change moves a composited layer instead of scheduling a
        // redraw — which is what `NSSwitch` does, keeping redraw scheduling out of the measurement.
        this.setWantsLayer(true);
        this
    }
}

/// Pool key for a marker: hand-made, so it cannot collide with a component's hashed key.
fn skew_key(index: usize, slot: u64) -> u64 {
    0x5CE0_0000_0000_0000u64 | (index as u64) << 4 | slot
}

struct NativeMarker {
    key: u64,
    rgb: (f64, f64, f64),
}

fn marker(key: u64, rgb: (f64, f64, f64)) -> NativeMarker {
    NativeMarker { key, rgb }
}

impl Element for NativeMarker {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size = size(px(MARK_W).into(), px(MARK_H).into());
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let mask = window.content_mask().bounds;
        let rgb = self.rgb;
        NativeHost::get(window, cx).place(self.key, Layer::Above, bounds, mask, move |mtm, _| {
            Marker::new(mtm, rgb)
        });
    }
}

impl IntoElement for NativeMarker {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

fn ns_window(window: &Window) -> Retained<NSWindow> {
    gpui_ns_view(window)
        .window()
        .expect("gpui's view is always in a window")
}

fn gpui_ns_view(window: &Window) -> Retained<NSView> {
    let handle = HasWindowHandle::window_handle(window).expect("gpui window has no native handle");
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        unreachable!("macOS-only example saw a non-AppKit window handle");
    };
    unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }
        .expect("gpui ns_view was null")
}

/// The `CAMetalLayer` GPUI presents through, identified by asking which layer answers
/// `maximumDrawableCount` rather than by assuming where it sits.
fn metal_layer_quiet(view: &NSView) -> Option<Retained<CALayer>> {
    let layer = view.layer()?;
    let responds: bool =
        unsafe { msg_send![&*layer, respondsToSelector: sel!(maximumDrawableCount)] };
    responds.then(|| layer)
}

impl Gallery {
    /// One frame of the rig: advance the offset by exactly the configured velocity, then render.
    fn skew_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        use std::io::Write;

        let velocity = self.skew.expect("skew_frame is only reached in skew mode");

        if !self.skew_announced {
            self.skew_announced = true;
            let ns = ns_window(window);
            let frame = ns.frame();
            // `screencapture -R` takes a CoreGraphics rect (top-left origin, y down) and
            // `NSWindow.frame` is AppKit space (bottom-left, y up); converting here keeps the
            // region a fact the app states rather than one the harness guesses.
            let primary = NSScreen::screens(MainThreadMarker::new().expect("main thread"))
                .firstObject()
                .expect("there is always a display");
            let cg_y = primary.frame().size.height - (frame.origin.y + frame.size.height);
            println!(
                "NG-SKEW-INFO window={} pid={} w={:.3} h={:.3} scale={:.4} velocity={:.4} \
                 cgx={:.3} cgy={:.3} row_h={SKEW_ROW_H} stride={SKEW_STRIDE} \
                 positive_shift={POSITIVE_SHIFT}",
                ns.windowNumber(),
                std::process::id(),
                frame.size.width,
                frame.size.height,
                ns.backingScaleFactor(),
                velocity,
                frame.origin.x,
                cg_y,
            );
            std::io::stdout().flush().ok();

            println!(
                "NG-SKEW-DRAWABLES-PENDING requested={:?}",
                std::env::var("NG_DRAWABLES").ok()
            );
            std::io::stdout().flush().ok();
        }

        // Written straight into the handle `uniform_list` reads, so velocity is exact rather than
        // the result of a momentum curve.
        self.scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), px(-self.skew_offset)));
        let ceiling = self.rows.len() as f32 * SKEW_ROW_H - 2000.;
        if self.skew_offset + velocity <= ceiling {
            self.skew_offset += velocity;
        } else if velocity != 0. {
            // Would clamp, so motion would silently stop and every arm would read like rest.
            println!("NG-SKEW-EXHAUSTED frames={}", self.skew_frames);
            std::io::stdout().flush().ok();
        }
        self.skew_frames += 1;

        let now = Instant::now();
        if let Some(previous) = self.skew_last {
            self.skew_intervals
                .push((now - previous).as_secs_f64() * 1000.);
        }
        self.skew_last = Some(now);

        // `NG_DRAWABLES` overrides the crate's own `METAL_DRAWABLE_COUNT` so both arms run from one
        // binary. Not applied in the announce block: `native_root` has not laid out there, so the
        // crate would install over it afterwards. Read back every tick to prove what the window ran
        // with for the whole run rather than only at startup.
        let mut drawables = 0usize;
        if let Some(layer) = metal_layer_quiet(&gpui_ns_view(window)) {
            if self.skew_frames == 2 {
                if let Some(want) = std::env::var("NG_DRAWABLES")
                    .ok()
                    .and_then(|value| value.trim().parse::<usize>().ok())
                {
                    let _: () = unsafe { msg_send![&*layer, setMaximumDrawableCount: want] };
                }
            }
            drawables = unsafe { msg_send![&*layer, maximumDrawableCount] };
        }

        // Every frame, not only at the periodic tick: a lone periodic `false` cannot separate
        // "never acquired key" from "acquired it and lost it", and those are different mechanisms.
        let key = ns_window(window).isKeyWindow();
        if key {
            self.skew_key_frames += 1;
        }
        if self.skew_key != Some(key) {
            println!(
                "NG-SKEW-KEY frame={} t={:.3} key={key} was={:?} keyed_frames={}",
                self.skew_frames,
                self.started.elapsed().as_secs_f64(),
                self.skew_key,
                self.skew_key_frames,
            );
            std::io::stdout().flush().ok();
            self.skew_key = Some(key);
        }

        if self.skew_frames % 120 == 0 {
            // Thresholds are multiples of the *observed* median rather than an assumed 8.333 ms, so
            // the count means the same thing on any refresh rate.
            let mut sorted = self.skew_intervals.clone();
            sorted.sort_by(f64::total_cmp);
            let at = |q: f64| -> f64 {
                if sorted.is_empty() {
                    return 0.;
                }
                sorted[((sorted.len() as f64 - 1.) * q).round() as usize]
            };
            let median = at(0.5);
            let long = sorted.iter().filter(|ms| **ms > median * 1.5).count();
            let very_long = sorted.iter().filter(|ms| **ms > median * 2.5).count();
            // Key state rides on the same line as the measurement that depends on it: an `NSSwitch`
            // in the ON state renders accent-filled while its window is key and desaturates when it
            // is not, which moves where a luminance edge finder locks onto its capsule.
            println!(
                "NG-SKEW-TICK frames={} t={:.3} offset={:.2} pooled={} dq={drawables} key={key} keyed={} \
                 intervals={} p50={:.3}ms p95={:.3}ms p99={:.3}ms max={:.3}ms \
                 over1.5x={long} over2.5x={very_long}",
                self.skew_frames,
                self.started.elapsed().as_secs_f64(),
                self.skew_offset,
                NativeHost::get(window, cx).len(),
                self.skew_key_frames,
                sorted.len(),
                median,
                at(0.95),
                at(0.99),
                at(1.0),
            );
            std::io::stdout().flush().ok();
        }

        let count = self.rows.len();
        // `NG_SKEW_LOAD=1` gives every visible row a real switch and slider, so throughput is
        // measured under the gallery's own per-frame cost. Never measured themselves: at one per
        // row their 44 pt pitch is smaller than the worst-case skew, so pairing would alias.
        let load = std::env::var_os("NG_SKEW_LOAD").is_some_and(|value| value != "0");
        let levels: Vec<f64> = self.rows.iter().map(|row| row.level).collect();
        let list = uniform_list("skew", count, move |range, _, _| {
            range
                .map(|index| {
                    let mut row = div().h(px(SKEW_ROW_H)).w_full().relative();
                    if load {
                        row = row
                            .child(
                                div()
                                    .absolute()
                                    .left(px(COL_LOAD))
                                    .top(px(MARK_TOP - 6.))
                                    .child(native_switch(("load-sw", index), index % 3 == 0)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(COL_LOAD + 80.))
                                    .top(px(MARK_TOP - 6.))
                                    .w(px(140.))
                                    .child(
                                        native_slider(("load-sl", index), levels[index])
                                            .range(0., 1.),
                                    ),
                            );
                    }
                    if index % SKEW_STRIDE != 0 {
                        return row;
                    }
                    row.child(
                        div()
                            .absolute()
                            .left(px(COL_GPUI))
                            .top(px(MARK_TOP))
                            .w(px(MARK_W))
                            .h(px(MARK_H))
                            .bg(rgb(0xff00ff)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(COL_NATIVE))
                            .top(px(MARK_TOP))
                            .child(marker(skew_key(index, 0), (0., 1., 0.))),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(COL_POSITIVE))
                            .top(px(MARK_TOP + POSITIVE_SHIFT))
                            .child(marker(skew_key(index, 1), (0., 0.8, 1.))),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(COL_SWITCH))
                            .top(px(MARK_TOP - 6.))
                            .child(native_switch(("skew-sw", index), true)),
                    )
                })
                .collect::<Vec<_>>()
        })
        .size_full()
        .track_scroll(self.scroll.clone());

        native_root(div().size_full().bg(rgb(SKEW_BG)).child(list)).into_any_element()
    }
}
