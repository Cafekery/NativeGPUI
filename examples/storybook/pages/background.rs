//! The one-background rule: a window has **one** background colour, and every opaque surface in it
//! resolves to that.
//!
//! The rule is short and the reason is not. A window's toolbar band and its body are one surface
//! interrupted by a strip of chrome, so two values are visible as a step — a line across the window
//! where no line exists. The one exception is a sidebar, which is vibrancy over this and not a
//! colour at all.
//!
//! Nothing on this page is a claim about the reader's Mac that the page cannot show them. Every
//! swatch carries the value it resolved to in this appearance, the step demo prints the two hexes
//! it is drawing so a reader can see whether there is a step at all, and the material demonstration
//! puts the same material over two different tones side by side.

use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Hsla, Rgba, SharedString, Window, div, prelude::*, px, rgb};
use native_gpui::sidebar::NativeSidebar;
use native_gpui::tokens::{
    Appearance, SystemColor, WINDOW_BACKGROUND, system_color, window_background,
};
use native_gpui::{
    Blending, Layer, RawMaterial, TrackingMode, material, native_segmented, native_slider,
};

/// Big enough that the material patch has a border of the tone it is sampling on every side.
const TONE: f32 = 132.;
const PATCH: f32 = 76.;

/// The two greys the sampling measurement in `tokens.rs` was taken over, so the numbers quoted in
/// the caption belong to what is on screen rather than to some other pair.
const DARK_TONE: u32 = 0x1d1d1d;
const LIGHT_TONE: u32 = 0x3c3c3c;

/// The dial's start: a plausible dark grey somebody typed once. Not a system colour, so it cannot
/// accidentally equal the window background and leave the seam demo showing nothing.
const HAND_PICKED: f64 = 37.;

pub struct State {
    /// The hand-picked grey, 0…255, on a dial so a reader can hunt for the value that closes the
    /// seam instead of being told what it is.
    pub grey: f64,
}

impl Default for State {
    fn default() -> Self {
        Self { grey: HAND_PICKED }
    }
}

pub fn render(d: &mut Demo, window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let metrics = NativeSidebar::get(window, cx).map(|s| s.metrics());
    let here = window_background();
    let control = system_color(SystemColor::ControlBackground);
    let grey = d.background.grey;
    let picked = grey_hsla(grey);

    stack(vec![
        note(
            "Two surfaces, never three. A sidebar pane in vibrancy, and everything else — the \
             toolbar band, the content pane under it, and whatever the app draws in the body — in \
             one colour.",
        ),
        section(
            "This window",
            vec![
                caption(match metrics {
                    Some(m) => format!(
                        "All three are on screen right now. The sidebar strip is {:.0}pt wide and \
                         the toolbar band {:.0}pt tall — read off the installed pane, not \
                         constants. The crate paints the band and the content pane the same \
                         colour, which is why the top edge of this text column has no line above \
                         it.",
                        m.thickness.to_f64(),
                        m.top.to_f64()
                    ),
                    None => "No sidebar is installed in this window, so the crate paints nothing \
                             and an app would paint tokens::window_background() itself."
                        .into(),
                }),
                caption(
                    "A sidebar's own surface is SidebarOptions::backdrop, and it is vibrancy \
                     rather than this colour: it samples the desktop behind the window, which is \
                     what makes it read as glass. Swapping it for an opaque material to match the \
                     content side moved the sidebar 40 → 27 in the band and 43 → 32 in its \
                     interior — a flatter panel, with the step closed at the pane instead. The \
                     Sidebar page has that on a knob.",
                ),
            ],
        ),
        section(
            "tokens::WINDOW_BACKGROUND",
            vec![
                caption(
                    "One token, resolved per appearance. Left half is Light, right half is Dark, \
                     both resolved explicitly rather than read off the live appearance — a \
                     single-column swatch would document one world and quietly contradict the \
                     other.",
                ),
                grid(vec![
                    swatch("WINDOW_BACKGROUND", WINDOW_BACKGROUND),
                    swatch("ControlBackground", SystemColor::ControlBackground),
                    swatch("UnderPageBackground", SystemColor::UnderPageBackground),
                ]),
                caption(format!(
                    "window_background() is system_color(WINDOW_BACKGROUND) — {} in this \
                     appearance. WINDOW_BACKGROUND.ns_color() is the same token for a caller \
                     handing it back to AppKit.",
                    hex(here)
                )),
            ],
        ),
        section(
            "Two values are a seam",
            vec![
                caption(
                    "Three panels butted together, each painted with what its label says and \
                     printing the value it drew. A line between two of them is the failure this \
                     rule exists to prevent.",
                ),
                div()
                    .flex()
                    .w(px(3. * 168.))
                    .child(step_panel("window_background()", here))
                    .child(step_panel("ControlBackground", control))
                    .child(step_panel("hand-picked", picked))
                    .into_any_element(),
                knob(
                    "hand-picked grey",
                    hex(picked),
                    native_slider("bg-grey", grey)
                        .range(0., 255.)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.background.grey = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                row(
                    "or land on it exactly",
                    vec![
                        native_segmented("bg-snap", 0)
                            .segments(["Snap the dial to the token"])
                            .tracking_mode(TrackingMode::Momentary)
                            .on_change(cx.listener(move |d: &mut Demo, _: &usize, _, cx| {
                                d.background.grey = grey_byte(here) as f64;
                                cx.notify();
                            }))
                            .into_any_element(),
                        mono(if hex(picked) == hex(here) {
                            "on the token — the seam is closed"
                        } else {
                            "off the token — the seam is open"
                        }),
                    ],
                ),
                caption(format!(
                    "Drag until the seam against the left panel closes. There is exactly one \
                     value that does it, and it is the token: {} in this appearance — 256 steps \
                     across a 180pt track is finer than a hand, so the button lands on it. Every \
                     grey either side of it is a line across the window.",
                    hex(here)
                )),
                caption(if here == control {
                    format!(
                        "ControlBackground resolves to {} here as well, so the first seam is \
                         invisible in this appearance — flip the appearance on the System Tokens \
                         page and it appears. Two tokens agreeing today is not a licence to use \
                         either.",
                        hex(control)
                    )
                } else {
                    format!(
                        "ControlBackground is {} against the window's {} in this appearance, so \
                         that seam is visible too. It is a control's fill, not a window's.",
                        hex(control),
                        hex(here)
                    )
                }),
            ],
        ),
        section(
            "Why it has to be a colour and not a material",
            vec![
                caption(
                    "A material's value tracks whatever it samples; a colour's does not. Both \
                     rows below put the same thing over two different tones. The material patch \
                     comes out as two different greys, the colour patch as one.",
                ),
                grid(vec![
                    tone_tile(
                        "material over #1D1D1D",
                        DARK_TONE,
                        material("bg-mat-dark", RawMaterial::UnderWindowBackground)
                            .layer(Layer::Above)
                            .blending(Blending::WithinWindow)
                            .size_full()
                            .into_any_element(),
                    ),
                    tone_tile(
                        "material over #3C3C3C",
                        LIGHT_TONE,
                        material("bg-mat-light", RawMaterial::UnderWindowBackground)
                            .layer(Layer::Above)
                            .blending(Blending::WithinWindow)
                            .size_full()
                            .into_any_element(),
                    ),
                    tone_tile(
                        "window_background() over #1D1D1D",
                        DARK_TONE,
                        div().size_full().bg(here).into_any_element(),
                    ),
                    tone_tile(
                        "window_background() over #3C3C3C",
                        LIGHT_TONE,
                        div().size_full().bg(here).into_any_element(),
                    ),
                ]),
                caption(
                    "Measured: UnderWindowBackground over a flat grey 29 reads 38 and over grey \
                     60 reads 44, while windowBackgroundColor reads 30 over both. The \
                     window-spanning backdrop samples behind the window, so what it tracks is the \
                     desktop — a band on that and a body on a colour cannot be reconciled by any \
                     constant, because only one of them moves.",
                ),
                caption(
                    "A second, cheaper tell that a surface is the first kind: it is \
                     state-dependent. Click another window and a material that has not pinned \
                     Active flattens — 43 to 55 in dark — and a colour does not. The Materials \
                     page has that pair side by side under dims_when_inactive.",
                ),
                caption(
                    "Do not try to re-measure any of this from a window-scoped screen capture. It \
                     composites over nothing behind the window, so a behind-window material \
                     reports its own flat tint however colourful the desktop is, and the null \
                     reads as \"not sampling\".",
                ),
            ],
        ),
        section(
            "What an app paints",
            vec![
                caption(
                    "With a sidebar the crate paints the window, band included, so an app should \
                     paint no background at all. Without one, paint the token and nothing else.",
                ),
                mono(
                    "native_root(div().size_full().child(body))                          // sidebar",
                ),
                mono("native_root(div().size_full().bg(tokens::window_background()).child(body))"),
                caption(
                    "NativeMaterial is Styled, so .bg() on a material paints over it — which is \
                     the one way to draw a third value without meaning to. The Materials page \
                     shows what that looks like.",
                ),
            ],
        ),
        props(&[
            (
                "tokens::WINDOW_BACKGROUND",
                format!("{WINDOW_BACKGROUND:?}"),
            ),
            (
                "WINDOW_BACKGROUND.objc_name()",
                WINDOW_BACKGROUND.objc_name().into(),
            ),
            ("tokens::window_background()", hex(here)),
            (
                "WINDOW_BACKGROUND.try_resolve(Light / Dark)",
                format!(
                    "{} / {}",
                    opt_hex(WINDOW_BACKGROUND.try_resolve(Appearance::Light)),
                    opt_hex(WINDOW_BACKGROUND.try_resolve(Appearance::Dark))
                ),
            ),
            (
                "SidebarMetrics.thickness / .top",
                metrics.map_or("no sidebar in this window".into(), |m| {
                    format!("{:.0}pt / {:.0}pt", m.thickness.to_f64(), m.top.to_f64())
                }),
            ),
            (
                "SidebarOptions::backdrop",
                "the sidebar's vibrancy, not this colour".into(),
            ),
            (
                "surfaces in a window",
                "two — a sidebar in vibrancy, everything else in one colour".into(),
            ),
            (
                "hand-picked grey on the dial",
                // Rounded bytes, not `Hsla` equality: the token comes back through
                // `colorWithSRGBRed:` and the dial's grey is an integer, so two colours that
                // paint the same pixel are not bit-equal and this row could only ever say "not".
                format!(
                    "{} — {} the token",
                    hex(picked),
                    if hex(picked) == hex(here) {
                        "matches"
                    } else {
                        "not"
                    }
                ),
            ),
        ]),
    ])
}

/// A Light | Dark split swatch over the checkerboard, so an alpha below 1 is visible as such.
fn swatch(label: &str, color: SystemColor) -> AnyElement {
    let (light, dark) = (
        color.try_resolve(Appearance::Light),
        color.try_resolve(Appearance::Dark),
    );
    let half = |value: Option<Hsla>| {
        let cell = div().flex_1().h_full();
        match value {
            Some(value) => cell.bg(value),
            None => cell,
        }
    };

    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .w(px(168.))
        .child(checker(
            168.,
            div()
                .flex()
                .size_full()
                .child(half(light))
                .child(half(dark))
                .into_any_element(),
        ))
        .child(mono(label.to_string()))
        .child(caption(format!("{} {}", opt_hex(light), opt_hex(dark))))
        .into_any_element()
}

/// One panel of the step demo, printing what it painted.
fn step_panel(label: &str, color: Hsla) -> AnyElement {
    div()
        .w(px(168.))
        .h(px(96.))
        .bg(color)
        .flex()
        .flex_col()
        .justify_end()
        .gap_0p5()
        .p_2()
        .child(
            div()
                .text_xs()
                .text_color(system_color(SystemColor::Label))
                .child(SharedString::from(label.to_string())),
        )
        .child(mono(hex(color)))
        .into_any_element()
}

/// A patch centred in a flat tone, with a margin of that tone on every side.
fn tone_tile(label: &str, tone: u32, patch: AnyElement) -> AnyElement {
    tile(
        label,
        TONE,
        div()
            .size(px(TONE))
            .bg(rgb(tone))
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(PATCH)).child(patch))
            .into_any_element(),
    )
}

/// The dial position that reproduces `color`, taking the red channel — every colour the button is
/// pointed at here is neutral, and the dial has no way to express one that is not.
fn grey_byte(color: Hsla) -> u8 {
    let c = Rgba::from(color);
    (c.r.clamp(0., 1.) * 255.).round() as u8
}

/// A neutral grey from the dial's 0…255, rounded so the printed hex is the value the dial reads.
fn grey_hsla(level: f64) -> Hsla {
    let byte = level.round().clamp(0., 255.) as u32;
    rgb(byte << 16 | byte << 8 | byte).into()
}

fn hex(c: Hsla) -> String {
    let c = Rgba::from(c);
    let q = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02X}{:02X}{:02X}{:02X}", q(c.r), q(c.g), q(c.b), q(c.a))
}

fn opt_hex(c: Option<Hsla>) -> String {
    c.map_or_else(|| "refused".to_string(), hex)
}
