//! Every component in one window, plus native controls inside a GPUI scroll container.
//!
//! The integration arm the per-component examples cannot cover: one [`NativeHost`] pool, one
//! commit pass, mixed native classes, and a scrolling list where views are pooled and clipped.
//! GPUI paints only visible rows, so only visible rows place a view — the pool count in the
//! header is the observable that proves it.
//!
//! `NG_ROWS` sets the list length (default 400). `NG_SKEW=<points-per-frame>` replaces the whole
//! window with the scroll-skew rig at the bottom of this file, whose arms are `NG_LAG`,
//! `NG_SKEW_STRIDE`, `NG_DRAWABLES`, `NG_PWT`, `NG_SKEW_STOP` and `NG_SKEW_LOAD`.

use std::panic::Location;
use std::time::Instant;

use gpui::{
    AnyElement, App, Application, Bounds, Context, Element, ElementId, Entity, GlobalElementId,
    Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Render, SharedString, Style,
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
    /// `Some(v)` puts the window in skew-measurement mode at `v` logical points per **refresh**.
    skew: Option<f32>,
    /// Refreshes between paints — see `wake_after`.
    skew_stride: usize,
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
        if self.skew.is_some() {
            // Stride 1 keeps `request_animation_frame`, byte for byte the cadence the 2.000-frame
            // lead and its 0.000 fix were measured on, so that arm stays comparable to them. Above
            // 1 the paints have to skip refreshes, which that call cannot do.
            match self.skew_stride {
                1 => window.request_animation_frame(),
                stride => wake_after(cx.entity(), window, stride),
            }
            return self.skew_frame(window, cx);
        }
        // Unconditional so the commit pass is exercised every frame, not only on input.
        window.request_animation_frame();

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
    // `NG_SKEW_STRIDE=<n>` paints on every n-th refresh instead of every one — see `wake_after`.
    let skew_stride: usize = std::env::var("NG_SKEW_STRIDE")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(1)
        .max(1);
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
                    skew_stride,
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
// What it found, and the arms that now exist to re-find it. `NG_LAG=0` turns the crate's
// compensation off: a hosted view then runs exactly two frames ahead of its row at every velocity —
// 8.00, 16.00 and 32.00 pt at 4, 8 and 16 pt per frame, the same integer multiple on every row of
// every capture, sd 0.000 — so this is presentation order and not a layout offset, and no easing
// curve can produce it. Left at the default the same rows read 0.000, and rest after an abrupt stop
// from 16 pt per frame reads exactly 0.000 as well. The switch stays locked to the marker in both
// arms, so a real control moves with the markers.
//
// `NG_SKEW_STRIDE=<n>` is the arm the first version of this rig could not have had, because the
// rig itself was the reason: `request_animation_frame` every frame makes GPUI paint on every
// refresh, and a paint per refresh is the one cadence where "the placement from two paints ago"
// and "the placement from two refreshes ago" are the same placement. A scroll driven by real
// trackpad events is not that cadence. At `n` refreshes per paint the offset still advances the
// same points per *refresh*, so every stride is the same on-screen velocity and the arms compare
// directly.
//
// Stride 1 is worse than uninformative about the queue's clock: it holds the precondition that
// keeps the clock healthy. Painting every refresh leaves a placement owed at all times, and the
// chain that counts refreshes used to switch off exactly when nothing was owed — so this rig could
// not drop a tick even in principle, and read 0.000 for a defect a trackpad scroll reproduced.
// Prefer stride 3 or more for anything about timing, and treat a stride-1 green as a null.
//
// Skew has two independent statistics and **neither one can see the other's defect**, which is why
// a clean depth ladder and an unshiftable stutter coexisted here for so long. A wrong queue depth
// is a constant offset: both columns move by the same amount on every frame pair, so per-pair
// mismatch reads zero, and it shows only as a step at motion onset that holds for the whole burst.
// A dropped refresh is the reverse: it redistributes motion between adjacent pairs, so its mean and
// its steady offset are both zero and only the per-pair distribution's tail carries it. Measure
// both — per-pair mismatch as a distribution, and steady excursion against the rest a burst starts
// and ends at — and never report either as a mean. Verified by injecting each defect into a
// synthetic recording and watching the other statistic stay flat.
//
// It is not GPUI's layout that is late but its *presented* frame: the Metal surface reaching the
// screen is `CAMetalLayer.maximumDrawableCount - 1` frames behind the `CATransaction` that carried
// the same layout's `setFrame`. `NG_DRAWABLES=2` halves the lead to 0.914 frames exactly as the
// queue theory predicts, and the crate reads the same property, so that arm also halves the
// compensation and still lands on 0.047. 1 is unreachable — Core Animation's floor is 2, and asking
// for 1 makes `nextDrawable` unsatisfiable and panics GPUI out of its own entity map before the
// first frame. The property is left at its default because the lead no longer depends on it: two
// buffers roughly doubles the share of frames over 1.5x the median interval under load, all of it
// in the tail, and buys nothing now that the geometry is written for the presented frame.
//
// `NG_PWT` sets `CAMetalLayer.presentsWithTransaction`, the remedy Apple documents for this exact
// symptom, and it is why the crate does not use it. Set once it reads back false every frame:
// GPUI turns it on for its own in-transaction draws and off again after, so it overwrites anything
// set from outside. Re-asserted every frame (`NG_PWT=2`) it sticks and removes one of the two
// frames, 1.035 against 1.962 — not both, because GPUI still presents through
// `presentDrawable:` and the flag alone cannot move that call onto the main thread's transaction.
//
// Two traps, both of which produced clean-looking wrong answers first.
//
// A window capture of an *occluded* window reads a re-composite from current `CALayer` state, which
// collapses the very gap being measured. `-l` is true about content and false about timing. A 60x20
// overlap of one corner by another app's window was enough to read the two-frame lead as one — a
// smaller plausible number, not an error. The gate is that the uncompensated arm must read its full
// 2.000: an arm that cannot see the gap it is supposed to see has disqualified itself.
//
// And delivery is not observation. A window-scoped `ScreenCaptureKit` stream on this window keeps
// handing out sample buffers at the display's own rate whether or not the window drew: measured
// 232 buffers in 2.0 s, every one of them `SCFrameStatus.idle` and not one `.complete`, from a
// window that painted nothing because its display link was stopped. Taking those as frames writes
// 232 identical observations and reports a flawless 0.000. Drop everything that is not `.complete`
// — and then count what arrived and why it was dropped, because `frames = 0` on its own is equally
// consistent with "the window drew nothing" and with "the filter rejects everything", and only the
// arrival count separates them.

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

/// Mark the rig dirty again `stride` display refreshes from now.
///
/// `Window::request_animation_frame` makes GPUI paint on **every** refresh, which is the cadence
/// the presentation-lag compensation was first measured against and the only one where a paint
/// count and a refresh count agree. `Window::on_next_frame` runs on every refresh dirty or not, so
/// chaining `stride` of them and notifying on the last reproduces a scroll that paints on some
/// refreshes and not others — without an input driver, and with the velocity still stated by this
/// file rather than recovered from the pixels.
fn wake_after(gallery: Entity<Gallery>, window: &mut Window, stride: usize) {
    window.on_next_frame(
        move |window, cx| match stride.checked_sub(1).filter(|left| *left > 0) {
            Some(left) => wake_after(gallery, window, left),
            None => gallery.update(cx, |_, cx| cx.notify()),
        },
    );
}

impl Gallery {
    /// One frame of the rig: advance the offset by exactly the configured velocity, then render.
    fn skew_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        use std::io::Write;

        // Points per *refresh*, so a stride arm covers the same ground per second as stride 1 and
        // the two are the same on-screen velocity.
        let velocity =
            self.skew.expect("skew_frame is only reached in skew mode") * self.skew_stride as f32;

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

            // Presentation arms are applied here, on the first render: `NativeHost` is created
            // during `native_root`'s layout, which is after this and before frame 2, and it derives
            // its compensation depth from `maximumDrawableCount`. Setting the count later would
            // leave the host compensating for a queue the layer no longer has.
            if let Some(layer) = metal_layer_quiet(&gpui_ns_view(window)) {
                if let Some(want) = std::env::var("NG_DRAWABLES")
                    .ok()
                    .and_then(|value| value.trim().parse::<usize>().ok())
                {
                    let _: () = unsafe { msg_send![&*layer, setMaximumDrawableCount: want] };
                }
                if std::env::var_os("NG_PWT").is_some_and(|value| value != "0") {
                    let _: () = unsafe { msg_send![&*layer, setPresentsWithTransaction: true] };
                    let back: bool = unsafe { msg_send![&*layer, presentsWithTransaction] };
                    println!("NG-SKEW-PWT-SET readback={back}");
                    std::io::stdout().flush().ok();
                }
            }
        }

        // `NG_SKEW_STOP=<frames>` stops the scroll dead after that many frames. The arm that
        // matters at rest: geometry held back N frames has to catch up on its own once GPUI stops
        // drawing, or a hosted view sits `N x final velocity` short of its row indefinitely, which
        // is a defect a still screenshot can see.
        let stopped = std::env::var("NG_SKEW_STOP")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .is_some_and(|at| self.skew_frames >= at);
        let velocity = if stopped { 0. } else { velocity };

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

        // Read back every tick to prove what the window ran with for the whole run rather than only
        // at the announce block that set it. `NG_PWT=2` re-asserts it every frame, which separates
        // "the write never landed" from "something else set it back".
        let mut drawables = 0usize;
        let mut pwt = false;
        if let Some(layer) = metal_layer_quiet(&gpui_ns_view(window)) {
            if std::env::var("NG_PWT").ok().as_deref() == Some("2") {
                let _: () = unsafe { msg_send![&*layer, setPresentsWithTransaction: true] };
            }
            drawables = unsafe { msg_send![&*layer, maximumDrawableCount] };
            pwt = unsafe { msg_send![&*layer, presentsWithTransaction] };
        }

        // `NG_LAG` is the arm that turns the crate's own compensation off (`0`) or pins it to a
        // chosen depth. Set every frame: `set_lag` is idempotent and the host is created lazily on
        // the first layout, after this runs for frame 1.
        let host = NativeHost::get(window, cx);
        if let Some(want) = std::env::var("NG_LAG")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
        {
            if host.lag() != want {
                host.set_lag(want);
            }
        }
        let lag = host.lag();

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
                "NG-SKEW-TICK frames={} t={:.3} offset={:.2} pooled={} dq={drawables} pwt={pwt} \
                 lag={lag} key={key} keyed={} \
                 intervals={} p50={:.3}ms p95={:.3}ms p99={:.3}ms max={:.3}ms \
                 over1.5x={long} over2.5x={very_long}",
                self.skew_frames,
                self.started.elapsed().as_secs_f64(),
                self.skew_offset,
                host.len(),
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
