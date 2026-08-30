//! Every component in one window, plus native controls inside a GPUI scroll container.
//!
//! This is the integration arm the per-component examples cannot cover: one [`NativeHost`] pool,
//! one commit pass, mixed native classes, and a scrolling list where views are pooled and clipped.
//! The scroll list is the interesting half — GPUI paints only visible rows, so only visible rows
//! place a view, and the pool count is the observable that proves it.
//!
//! `NG_ROWS` sets the list length (default 400). The header line reports the live pool count.
//!
//! `NG_SKEW=<points-per-frame>` replaces the whole window with the scroll-skew measurement rig at
//! the bottom of this file, which scrolls in-process at a velocity you set. It found that a hosted
//! view runs exactly two frames ahead of the GPUI row it belongs to at every velocity; the comment
//! block above [`SKEW_ROW_H`] has the numbers and why it is built the way it is.

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
    /// Frame-to-frame intervals in milliseconds, so a mitigation that buys a smaller skew by
    /// dropping frames cannot be reported as a win.
    skew_last: Option<Instant>,
    skew_intervals: Vec<f64>,
    /// Key state, sampled every frame. Logged at first paint and on every transition, not only
    /// periodically: a single periodic `false` cannot tell "never acquired" from "acquired then
    /// lost", and those are different mechanisms.
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

    /// The pooling arm.
    ///
    /// `uniform_list` is load-bearing, not a style choice: it paints only the visible range, so
    /// only visible rows place a hosted view and the pool stays in the tens however long the list
    /// is. A plain `div().overflow_y_scroll()` with the same children paints every row — measured
    /// at 811 pooled views for 400 rows, which is past the comfortable budget for switches and far
    /// past it for sliders.
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
    // The skew rig and the gallery are different trees, so `render` hands back an `AnyElement`.
    #[allow(refining_impl_trait)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Keep the frame loop hot so the commit pass is exercised continuously, not just on input.
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
                // Native window-background material, below GPUI's Metal layer.
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
                        // Glass panel over the GPUI-drawn divider and list edge.
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
        // The skew rig scrolls one direction forever; enough rows that a multi-minute run at
        // 12 pt/frame never reaches the end, where the offset would clamp and motion would stop.
        .unwrap_or(if skew.is_some() { 40_000 } else { 400 });

    // `NG_ORIGIN=x,y` places the window somewhere other than the default. The skew rig needs its
    // window actually scanned out — a window on a Space some other app owns fullscreen is not, and
    // a window capture of one reads a re-composite rather than the presented frame, which is
    // precisely the quantity being measured.
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

// =================================================================================================
// Scroll-skew measurement rig  (NG_SKEW=<points-per-frame>)
// =================================================================================================
//
// The question: during continuous scrolling, does a hosted AppKit view drift vertically from the
// GPUI-painted row it belongs to?
//
// Two things make that hard to measure honestly, and both are why this rig looks the way it does.
//
// * A settled screenshot cannot show a during-motion artefact. So the scroll is driven **in
//   process** — the list offset advances a fixed number of logical points per rendered frame under
//   `request_animation_frame` — and never by synthetic input. Any capture taken while the app runs
//   is therefore mid-motion, captures cannot race an input driver, and the velocity is a number
//   this file *sets* rather than one that has to be recovered from the pixels afterwards.
// * Skew in pixels is uninterpretable. With velocity `v` points/frame and a measured vertical
//   offset `d` points between a row's GPUI-painted band and its hosted view, the lead is `d / v`
//   **frames**. Running several velocities separates the two hypotheses: a presentation-order skew
//   holds the *frame* count constant while the pixel offset scales with `v`; a constant layout
//   offset holds the *pixel* offset constant while the frame count scales with `1/v`.
//
// Each marked row carries four features at a known vertical relationship, all locatable from
// pixels in the same frame, left to right:
//
//   1. a GPUI-painted magenta band          — the reference, drawn by GPUI's own renderer
//   2. a hosted marker view, same top       — the arm under test
//   3. a hosted marker view, `+6.00 pt`     — the known-positive fixture
//   4. a real `NSSwitch`, switched on       — corroboration with a control a user would touch
//
// Feature 3 is the check that disqualifies the instrument: it is the same class, placed through
// the same `NativeHost::place` call, differing only by a layout offset that is known in advance.
// If the edge finder cannot read `+6.00 pt` off it, then the null it reports for feature 2 means
// nothing. Nothing else in the window paints, so every feature is found by scanning for
// non-background columns rather than by indexing a fixed offset into the frame — and each column
// found is then *validated* against the x this file placed it at before it is measured. Scanning
// alone is not enough: with the load arm's extra columns present, occluding only the GPUI reference
// column still leaves four runs, and taking the leftmost four silently re-indexes onto
// native/positive/switch/load. That reports `+6.00 pt` — the known-positive fixture's own value, so
// it reads as plausible rather than as garbage — where the truth was `-24.00`. Validating positions
// keeps occlusion, a shifted window and a wrong-content capture all failing loudly.
//
// One more disqualifier, because the quantity here is the gap between an AppKit layer and a Metal
// surface and the capture path sits directly across it. `screencapture -l` is a *window* capture,
// and a window capture that re-composited the layer tree from current `CALayer` state would report
// a lead this rig had no business seeing. So both methods are run against the same motion,
// alternating so capture order cannot masquerade as a method difference: window capture against
// `-R` region capture, which reads what was actually scanned out. They agree exactly — mode
// `-24.0 pt` from both at v = 12, `-48.0 pt` from both at v = 24 — and the region arm is the
// slightly cleaner of the two, which is the opposite of what a re-compositing artefact predicts.
//
// # What it measured
//
// A hosted view runs **exactly two frames ahead** of the GPUI row it belongs to, at every scroll
// velocity. Measured across 232 window captures carrying 756 paired row measurements, sub-pixel
// edges read from `screencapture -x -o -l`:
//
//     v (pt/frame)      0      4       8      12      24
//     offset (pt)   +0.000 -8.000 -16.000 -24.000 -48.000
//     lead (frames)    n/a  +2.000  +2.000  +2.000  +2.000
//
// Every single capture read an exact integer multiple of `v` — never a fraction — so this is
// presentation order, not a layout offset. The velocity-0 control reads `+0.000` pt and the
// known-positive fixture reads `+6.000` pt in every arm at every velocity, so the residual is
// zero rather than merely small: there is nothing to subtract. The real `NSSwitch` is locked to
// the marker — `switch - marker` carries no `v`-quantised component in any arm — so a toggle
// leads its own row by the same two frames.
//
// It is not GPUI's layout that is late, it is GPUI's *presented* frame: the `CATransaction`
// carrying `setFrame` lands two presented frames before the Metal surface drawn from the same
// layout. Holding hosted geometry back N frames subtracts exactly N — depths 0 to 3 at
// v = 12 pt/frame, depths 0 and 2 confirmed again at v = 24, 55 to 79 samples per cell:
//
//     frames held back    0      1      2      3
//     lead (frames)   +2.00  +1.00   0.00  -1.00
//
// So holding geometry back one frame *halves* the lead rather than cancelling it, and two frames
// is what would cancel the mode. The crate does neither, deliberately. Those same captures show
// each capture reading either `k` or `k-1` frames depending where in the refresh interval it
// samples, so the skew is a steady 2 at depth 0 but straddles 0 and one-frame-*behind* at depth 2;
// and every hosted view's geometry would be two frames stale for every geometry change that is not
// a constant-velocity scroll — a control appearing, a resize, a jump to a new position. A constant
// lead during a fling, snapping to exactly 0 the instant scrolling stops, is the better failure.
//
// # Why two, and what shortening it costs
//
// The two is a **drawable-queue depth**, not a race. GPUI presents asynchronously on the
// display-link path a scrolling frame takes, so the Metal surface reaching the screen is up to
// `CAMetalLayer.maximumDrawableCount - 1` frames behind the `CATransaction` that carried the same
// layout's `setFrame`. Setting that property is a one-line, public-API mitigation, and it moves the
// lead exactly as the queue theory predicts — measured from one binary with `NG_DRAWABLES`, per
// capture gated on `-l` and `-R` agreeing:
//
//     maximumDrawableCount        3 (CA default)      2
//     lead (frames)                    2.000       1.000
//     offset at v = 12 (pt)             -24         -12
//     offset at v = 24 (pt)             -48         -24
//
// It holds with a switch and a slider hosted on every visible row, and rest stays exactly
// `0.000 pt`. `maximumDrawableCount = 1` is unreachable: Core Animation's floor is 2, and asking
// for 1 makes `nextDrawable` unsatisfiable and panics GPUI out of its own entity map before the
// first frame. **One frame of lead is therefore irreducible from here.**
//
// The crate does not set it, because under load it costs frame pacing. Interleaved A/B, two reps,
// same load, alternating so run order cannot masquerade as the effect:
//
//     drawables   p95        p99      frames over 1.5x median
//     2           13.5-15.8  19.5-21.6      5.4-7.8 %
//     3           10.7-11.5  16.0-18.5      2.3-3.5 %
//
// The median interval is the display period in every arm; the cost is entirely in the tail, which
// is what fewer buffers in flight would predict — less slack to absorb a hiccup. Halving a 12-24 pt
// scroll-time artefact that is invisible at normal speeds is not worth roughly doubling the
// long-frame rate, so this is left at Core Animation's default and recorded here instead.
//
// One trap worth keeping, because it silently produced a clean-looking wrong answer first: a
// **window capture of an occluded window reads a re-composite from current `CALayer` state**, which
// collapses the very gap this rig measures. `-l` is true about content and false about timing. With
// another app owning the display's Space the same arm read an unstable 0 or 1 frame; moved to a
// display it was actually scanned out on, `-l` and `-R` agreed to the point. Requiring those two to
// agree per capture is the gate, and it costs one extra capture.

/// Row pitch. Only every `SKEW_STRIDE`-th row is marked, so marked rows are `44 * 4 = 176 pt`
/// apart — far more than any plausible skew, which is what keeps pairing a band with its own
/// hosted view unambiguous instead of aliasing onto the neighbouring row.
const SKEW_ROW_H: f32 = 44.;
const SKEW_STRIDE: usize = 4;

/// Feature geometry within a marked row, logical points.
const MARK_TOP: f32 = 14.;
const MARK_W: f32 = 56.;
const MARK_H: f32 = 8.;
/// The known-positive fixture's deliberate offset. Must be big enough to be unmistakable and
/// small enough that it cannot be confused with a neighbouring row.
const POSITIVE_SHIFT: f32 = 6.;

const COL_GPUI: f32 = 40.;
const COL_NATIVE: f32 = 140.;
const COL_POSITIVE: f32 = 240.;
const COL_SWITCH: f32 = 340.;
/// Where the `NG_SKEW_LOAD` controls go: far enough right of the switch column that they form
/// their own column runs and never merge with a measured one.
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
    /// of its own sub-pixel frame origin — which is exactly the quantity being measured. It is
    /// placed through the shipped [`NativeHost::place`] path, so it moves when and how a real
    /// control moves.
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
        // sRGB, not device RGB: `colorWithRed:` would be a different colour on a P3 display.
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(rgb.0, rgb.1, rgb.2, 1.);
        let this = Self::alloc(mtm).set_ivars(MarkerIvars { color });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        // Its own layer, so a frame change moves a composited layer instead of scheduling a
        // redraw — which is what an `NSSwitch` does, and keeps redraw scheduling out of the
        // measurement.
        this.setWantsLayer(true);
        this
    }
}

/// The pool key for a marker. Hand-made rather than derived from element identity, so it needs no
/// `ElementId` and cannot collide with a component's hashed key.
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
    let handle = HasWindowHandle::window_handle(window).expect("gpui window has no native handle");
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        unreachable!("macOS-only example saw a non-AppKit window handle");
    };
    let view = unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }
        .expect("gpui ns_view was null");
    view.window().expect("gpui's view is always in a window")
}

fn gpui_ns_view(window: &Window) -> Retained<NSView> {
    let handle = HasWindowHandle::window_handle(window).expect("gpui window has no native handle");
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        unreachable!("macOS-only example saw a non-AppKit window handle");
    };
    unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }
        .expect("gpui ns_view was null")
}

/// The `CAMetalLayer` GPUI presents its drawables through, identified by asking which layer
/// answers `maximumDrawableCount` rather than by assuming where it sits. Only the view's own
/// root layer is tested, which is where every measurement has found it.
fn metal_layer_quiet(view: &NSView) -> Option<Retained<CALayer>> {
    let layer = view.layer()?;
    let responds: bool =
        unsafe { msg_send![&*layer, respondsToSelector: sel!(maximumDrawableCount)] };
    responds.then(|| layer)
}

impl Gallery {
    /// One frame of the skew rig: advance the scroll offset by exactly the configured velocity,
    /// then render the marked list.
    fn skew_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        use std::io::Write;

        let velocity = self.skew.expect("skew_frame is only reached in skew mode");

        if !self.skew_announced {
            self.skew_announced = true;
            let ns = ns_window(window);
            let frame = ns.frame();
            // `screencapture -R` takes a rect in CoreGraphics space: origin top-left of the
            // primary display, y down. `NSWindow.frame` is AppKit space: origin bottom-left of
            // the primary display, y up. Printing the converted rect from inside the app keeps
            // the region a fact the app states rather than one the harness guesses.
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

            // `NG_DRAWABLES` overrides the crate's own `METAL_DRAWABLE_COUNT` so both arms run from
            // one binary. It is applied on the SECOND announce pass, not here: this block runs
            // before `native_root` has laid out, so the host has not installed yet and anything
            // written now would be overwritten by the crate — and a readback taken here reports the
            // Core Animation default rather than what the window actually runs with. The live value
            // rides on every tick line instead, which also proves the setting held for the whole
            // run rather than only at startup.
            println!(
                "NG-SKEW-DRAWABLES-PENDING requested={:?}",
                std::env::var("NG_DRAWABLES").ok()
            );
            std::io::stdout().flush().ok();
        }

        // The offset the frame about to be laid out will use, then advance for the next one. Written
        // straight into the handle `uniform_list` reads, so the velocity is exact rather than the
        // result of a momentum curve.
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

        // Applied here rather than in the announce block: by frame 2 `native_root` has installed the
        // host, so the crate's own `METAL_DRAWABLE_COUNT` is in place and this overrides it. Read
        // back every tick so the log proves what the window actually ran with.
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

        // Every frame, not only at the periodic tick. A lone periodic `false` cannot separate
        // "never acquired key" from "acquired it and lost it", and those point at different
        // mechanisms — so first paint and every transition are both printed, and the running count
        // of keyed frames makes persistence readable rather than inferred.
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
            // The long-frame thresholds are multiples of the *observed* median interval rather
            // than an assumed 8.333 ms, so the count means the same thing on any refresh rate and
            // cannot be gamed by a display that is not running at 120 Hz.
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
            // Key state rides on the same line as the measurement that could depend on it: an
            // `NSSwitch` in the ON state renders accent-filled while its window is key and
            // desaturates when it is not, which moves where a luminance edge finder locks onto its
            // capsule. `keyed` is the running count of keyed frames, so a reader can tell a run
            // that held key throughout from one that never had it.
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
        // The load arm. `NG_SKEW_LOAD=1` gives *every* visible row a real switch and slider, which
        // is the per-frame cost of the gallery's own list, so the throughput numbers are measured
        // under load rather than on an almost-empty window. They sit right of every measurement
        // column and are never measured: at one per row their 44 pt pitch is smaller than the
        // worst-case skew, so pairing against them would alias onto the neighbouring row.
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
