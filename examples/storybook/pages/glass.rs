//! Public liquid glass: the container that batches many panels, then one panel with its knobs.
//!
//! Every specimen sits on something busy. Glass frosting GPUI's own Metal content is the thing
//! worth seeing, and a flat fill cannot show whether a panel is sampling or has collapsed.

use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Hsla, Window, div, hsla, prelude::*, px, rgb};
use native_gpui::{
    glass, glass_container, native_color_well, native_segmented, native_slider, native_switch,
};

/// Stripe half-period. Fine enough that a blur has plenty of structure to eat.
const STRIPE: f32 = 8.;
const STRIPE_COLORS: [u32; 6] = [0xff375f, 0xffd60a, 0x30d158, 0x0a84ff, 0xbf5af2, 0xf2f2f7];

const RADII: [f32; 6] = [0., 8., 16., 26., 40., 44.];

/// The batch band. A fixed column count keeps the row count exact at any window width, so the
/// band's height is stated rather than left to a percentage against an auto-sized parent.
const COLS: usize = 12;
const PANEL_W: f32 = 44.;
const PANEL_H: f32 = 30.;
const GAP: f32 = 6.;
const PAD: f32 = 12.;
const COUNTS: [usize; 3] = [12, 36, 96];

pub struct State {
    pub radius: f64,
    pub clear: bool,
    pub tint_on: bool,
    pub tint: Hsla,
    pub spacing: f64,
    /// Index into [`COUNTS`].
    pub panels: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            radius: 26.,
            clear: false,
            tint_on: false,
            tint: hsla(0.58, 0.9, 0.55, 0.35),
            spacing: 0.,
            panels: 1,
        }
    }
}

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let (radius, clear, tint_on, tint) =
        (d.glass.radius, d.glass.clear, d.glass.tint_on, d.glass.tint);
    let spacing = d.glass.spacing;
    let selected = d.glass.panels.min(COUNTS.len() - 1);
    let count = COUNTS[selected];

    let live = |id: &'static str| -> AnyElement {
        let panel = glass(id).corner_radius(px(radius as f32)).clear(clear);
        if tint_on { panel.tint(tint) } else { panel }.into_any_element()
    };

    stack(vec![
        note(
            "Every pixel is AppKit's. Glass samples whatever GPUI painted beneath it, so over the \
             pane's flat background it would have nothing to bend — each specimen here sits on a \
             checkerboard or on stripes.",
        ),
        section(
            "glass_container",
            vec![
                band(count, spacing),
                knob(
                    "spacing",
                    format!("{spacing:.0} px"),
                    native_slider("spacing", spacing)
                        .range(0., 40.)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.glass.spacing = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "panels",
                    count.to_string(),
                    native_segmented("count", selected)
                        .segments(COUNTS.map(|count| count.to_string()))
                        .on_change(cx.listener(|d: &mut Demo, value: &usize, _, cx| {
                            d.glass.panels = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                caption(
                    "Members find the container themselves and are placed inside its contentView, \
                     which is what does the batching — a container merely present in the hierarchy \
                     buys nothing. 400 panels go from 18 fps to 120.",
                ),
                caption(format!(
                    "spacing is how close two members must be before macOS merges them into one \
                     blob of glass. These sit {GAP:.0}pt apart, so anything past that fuses them; \
                     0, the default, batches without merging."
                )),
                note(
                    "The container is placed at the whole window, not at this band, and it claims \
                     every mouse point inside it — a hosted control painted before it never sees a \
                     click. That is why the batch leads this page: every knob below it stays live, \
                     and events the container does not handle still reach GPUI.",
                ),
            ],
        ),
        section(
            "live panel",
            vec![
                row(
                    "the same panel over a checkerboard and over GPUI stripes",
                    vec![
                        checker(120., live("live-checker")),
                        stripes(240., 120., live("live-stripes")),
                    ],
                ),
                knob(
                    "corner_radius",
                    format!("{radius:.0} px"),
                    native_slider("radius", radius)
                        .range(0., 44.)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.glass.radius = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "clear",
                    clear.to_string(),
                    native_switch("clear", clear)
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                            d.glass.clear = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "tint",
                    if tint_on { "on" } else { "none" },
                    native_switch("tint-on", tint_on)
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                            d.glass.tint_on = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "tint colour",
                    format!("α {:.2}", tint.a),
                    native_color_well("tint", tint)
                        .on_change(cx.listener(|d: &mut Demo, value: &Hsla, _, cx| {
                            d.glass.tint = *value;
                            d.glass.tint_on = true;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
            ],
        ),
        section(
            "corner_radius",
            vec![
                grid(
                    RADII
                        .iter()
                        .map(|&radius| {
                            tile(
                                format!("{radius:.0} px"),
                                116.,
                                stripes(
                                    116.,
                                    84.,
                                    glass(("radius", radius as u32))
                                        .corner_radius(px(radius))
                                        .into_any_element(),
                                ),
                            )
                        })
                        .collect(),
                ),
                caption("The panel rounds its own shape; no GPUI clipping is involved."),
            ],
        ),
        section(
            "style",
            vec![
                grid(vec![
                    tile(
                        "regular",
                        150.,
                        stripes(
                            150.,
                            100.,
                            glass("style-regular")
                                .corner_radius(px(20.))
                                .into_any_element(),
                        ),
                    ),
                    tile(
                        "clear(true)",
                        150.,
                        stripes(
                            150.,
                            100.,
                            glass("style-clear")
                                .clear(true)
                                .corner_radius(px(20.))
                                .into_any_element(),
                        ),
                    ),
                ]),
                caption(
                    "clear tints rather than blurs, so most of the backdrop's structure survives \
                     it. For imagery that has to stay legible, never for a lighter frost.",
                ),
            ],
        ),
        section(
            "tint",
            vec![
                grid(vec![
                    tile(
                        "none",
                        150.,
                        stripes(
                            150.,
                            100.,
                            glass("tint-none").corner_radius(px(20.)).into_any_element(),
                        ),
                    ),
                    tile(
                        "blue α 0.30",
                        150.,
                        stripes(
                            150.,
                            100.,
                            glass("tint-blue")
                                .corner_radius(px(20.))
                                .tint(hsla(0.58, 0.9, 0.55, 0.3))
                                .into_any_element(),
                        ),
                    ),
                    tile(
                        "orange α 0.30",
                        150.,
                        stripes(
                            150.,
                            100.,
                            glass("tint-orange")
                                .corner_radius(px(20.))
                                .tint(hsla(0.06, 0.95, 0.55, 0.3))
                                .into_any_element(),
                        ),
                    ),
                    tile(
                        "blue α 1.00",
                        150.,
                        stripes(
                            150.,
                            100.,
                            glass("tint-full")
                                .corner_radius(px(20.))
                                .tint(hsla(0.58, 0.9, 0.55, 1.))
                                .into_any_element(),
                        ),
                    ),
                    tile(
                        "clear + orange",
                        150.,
                        stripes(
                            150.,
                            100.,
                            glass("tint-clear")
                                .clear(true)
                                .corner_radius(px(20.))
                                .tint(hsla(0.06, 0.95, 0.55, 0.3))
                                .into_any_element(),
                        ),
                    ),
                ]),
                caption(
                    "Alpha is the strength: at 1.0 no frost is left, so start around 0.3. macOS \
                     drops the chroma entirely while the window is not key and no property pins \
                     that — click another app and only the luminance shift stays.",
                ),
            ],
        ),
        section(
            "ordering",
            vec![
                grid(vec![
                    tile(
                        "frosting_gpui",
                        150.,
                        stripes(
                            150.,
                            100.,
                            glass("order-above")
                                .frosting_gpui()
                                .corner_radius(px(20.))
                                .into_any_element(),
                        ),
                    ),
                    tile(
                        "frosting_desktop",
                        150.,
                        stripes(
                            150.,
                            100.,
                            glass("order-below")
                                .frosting_desktop()
                                .corner_radius(px(20.))
                                .into_any_element(),
                        ),
                    ),
                ]),
                note(
                    "frosting_desktop orders the panel under GPUI's Metal view, so it frosts the \
                     desktop and a GPUI label on it stays sharp — and it can never receive input. \
                     The second specimen is present and invisible: it needs a rect the scene \
                     leaves transparent, and this pane paints an opaque background over every \
                     pixel.",
                ),
            ],
        ),
        props(&[
            (
                "glass(id)",
                "NSGlassEffectView, fills the box you give it".into(),
            ),
            ("corner_radius(Pixels)", format!("{radius:.0}")),
            (
                "tint(impl Into<Hsla>)",
                format!("α {:.2}, {}", tint.a, if tint_on { "live" } else { "off" }),
            ),
            ("clear(bool)", clear.to_string()),
            (
                "frosting_gpui()",
                "default — frosts the GPUI content beneath".into(),
            ),
            (
                "frosting_desktop()",
                "under GPUI's view; needs a transparent rect".into(),
            ),
            (
                "glass_container(id)",
                format!("{count} panels, one render pass"),
            ),
            ("spacing(Pixels)", format!("{spacing:.0}")),
        ]),
    ])
}

/// GPUI-drawn stripes behind a specimen. A checkerboard shows whether a material collapsed; fine
/// stripes also show the glass bending what is underneath it.
pub fn stripes(width: f32, height: f32, child: AnyElement) -> AnyElement {
    let count = (width / (2. * STRIPE)).ceil() as usize;
    div()
        .relative()
        .w(px(width))
        .h(px(height))
        .overflow_hidden()
        .bg(rgb(0x0a0a0a))
        .children((0..count).map(|i| {
            div()
                .absolute()
                .left(px(i as f32 * 2. * STRIPE))
                .top_0()
                .w(px(STRIPE))
                .h_full()
                .bg(rgb(STRIPE_COLORS[i % STRIPE_COLORS.len()]))
        }))
        .child(div().absolute().inset_0().child(child))
        .into_any_element()
}

fn band(count: usize, spacing: f64) -> AnyElement {
    let rows = count.div_ceil(COLS);
    let width = 2. * PAD + COLS as f32 * PANEL_W + (COLS - 1) as f32 * GAP;
    let height = 2. * PAD + rows as f32 * PANEL_H + (rows - 1) as f32 * GAP;

    let members = (0..rows).map(move |row| {
        let first = row * COLS;
        div()
            .flex()
            .gap(px(GAP))
            .children((first..(first + COLS).min(count)).map(|i| {
                div()
                    .w(px(PANEL_W))
                    .h(px(PANEL_H))
                    .child(glass(("member", i as u32)).corner_radius(px(10.)))
            }))
    });

    stripes(
        width,
        height,
        div()
            .size_full()
            .p(px(PAD))
            .child(
                glass_container("batch")
                    .spacing(px(spacing as f32))
                    .child(div().flex().flex_col().gap(px(GAP)).children(members)),
            )
            .into_any_element(),
    )
}
