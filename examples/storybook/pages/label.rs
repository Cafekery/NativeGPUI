//! `native_label`, `native_rule`, and the ordering claim the whole hosted-decoration idea rests on.
//!
//! The hero band is one specimen with its own control built in. A `Material::Header` at
//! `Layer::Above` with `Blending::WithinWindow` samples the GPUI content under it and leaves
//! roughly 1.5% of that content's contrast behind — all of it, a GPUI caption in the same rect
//! included. So the left half of the band draws its text and its hairline with GPUI and the right
//! half draws the identical things with `native_label` and `native_rule`, ordered above the
//! material by paint order. One capture row crosses both: the sharp half is what makes the flat
//! half mean something, because a frame containing only flat content cannot tell attenuation from
//! nothing drawn.
//!
//! "Attenuates" and not "blurs", deliberately. The note this page renders says the material blurs
//! every GPUI pixel in its rect, which names the mechanism; what a capture can actually establish
//! is that the content's contrast is nearly gone and its luminance is still present and confined to
//! where GPUI painted. A spatial spread would need a reading the numbers below do not contain. The
//! rendered strings are left exactly as they are because their wrapped height sets this page's
//! layout, and every y in those numbers with it.
//!
//! The sharp half is necessary and not sufficient, because the two halves are drawn by different
//! code: it proves the frame has ink somewhere, not that the flat half's ink was ever painted.
//! Measured, the flat half keeps 3 counts of contrast against a ground of 29 — about 1.5% of the
//! 192 the hosted half carries — and no reading taken inside it can tell that from nothing drawn.
//! What settles it is a mean over the ink's own x extent against the inkless corridors, because
//! attenuated ink still carries luminance where erased ink carries none. The ink's extent reads
//! 30.143 and BOTH corridors read 29.163, one of them 320pt away: identical to three decimals, so
//! the lift is confined to where GPUI painted rather than being a gradient across the band. Two
//! strips that could have disagreed and did not are what make the +0.98 mean anything.
//!
//! A capture of this page also has to come from a fresh `open -n` of a bundle. Activating an
//! already-running instance leaves the app frontmost with a NON-key window and `AXRaise` does not
//! change that, and the key check has to read the close button's saturated red: the green light
//! reaches only about 169 even when key, so a threshold set for red calls every window non-key.
//!
//! The 1pt hairline is the cleanest of the pairs — a hard step of known height, where the text
//! rows are a superposition of many — and it is the one the material flattens outright.
//!
//! The `hosted` switch collapses the right half back to GPUI drawing, which is the falsification
//! arm: with it off both halves must go flat, and measured, both do.

use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, FontWeight, SharedString, Window, div, prelude::*, px};
use native_gpui::tokens::{SystemColor, system_color, system_font, system_font_tabular};
use native_gpui::{
    Blending, LabelAlign, Layer, Material, material, native_label, native_rule, native_switch,
};

/// Finder's own column widths, so the header specimen is the real geometry.
const COLUMNS: [(&str, f32, LabelAlign); 4] = [
    ("Name", 168., LabelAlign::Natural),
    ("Date Modified", 181., LabelAlign::Natural),
    ("Size", 97., LabelAlign::Right),
    ("Kind", 121., LabelAlign::Natural),
];

const BAND_W: f32 = 620.;
const BAND_H: f32 = 148.;
/// Each half of the band. Both halves are wider than the strings they hold — the 180pt hairline is
/// the widest ink either one carries — so the band has an inkless corridor from x 192 to x 328 and
/// another past x 508. Those are not spacing: they are where a capture reads the backdrop with
/// nothing behind it and nothing over it, and the flat half's mean is meaningless without them.
const HALF: f32 = 280.;

/// A header title, and a high-frequency string whose adjacent-pixel deltas are as large as text
/// gets — the strongest available signal for "is this edge sharp".
const SPECIMENS: [(&str, f32, FontWeight); 3] = [
    ("Date Modified", 11., FontWeight::MEDIUM),
    ("IIIIIIIIIIIIIIIIII", 13., FontWeight::NORMAL),
    ("1111111111", 13., FontWeight::NORMAL),
];

pub struct State {
    pub hosted: bool,
    pub tabular: bool,
}

impl Default for State {
    /// `NG_LABEL_HOSTED=0` starts on the falsification arm, so a capture of it and a capture of
    /// the subject are the same binary on the same page with one bit different — and neither
    /// needs a click to land, which a measurement should not depend on.
    fn default() -> Self {
        Self {
            hosted: std::env::var("NG_LABEL_HOSTED").ok().as_deref() != Some("0"),
            tabular: true,
        }
    }
}

fn gpui_line(text: &str, size: f32, weight: FontWeight) -> AnyElement {
    div()
        .h(px(18.))
        .flex()
        .items_center()
        .text_size(px(size))
        .font(system_font(weight))
        .text_color(system_color(SystemColor::Label))
        .child(SharedString::from(text.to_owned()))
        .into_any_element()
}

fn hosted_line(id: &'static str, text: &str, size: f32, weight: FontWeight) -> AnyElement {
    div()
        .h(px(18.))
        .flex()
        .items_center()
        .child(
            native_label(id, text)
                .font_size(px(size))
                .weight(weight)
                .color(SystemColor::Label),
        )
        .into_any_element()
}

/// A 1pt hairline drawn by GPUI, and its hosted twin. `SystemColor::Label` rather than
/// `Separator`: this pair is what the material acts on, and the widest possible step off the band's
/// own ground is what keeps the surviving 1.5% above the capture's own quantisation.
fn gpui_hairline() -> AnyElement {
    div()
        .h(px(18.))
        .flex()
        .items_center()
        .child(
            div()
                .h(px(1.))
                .w(px(180.))
                .bg(system_color(SystemColor::Label)),
        )
        .into_any_element()
}

fn hosted_hairline(id: &'static str) -> AnyElement {
    div()
        .h(px(18.))
        .flex()
        .items_center()
        .child(
            div()
                .h(px(1.))
                .w(px(180.))
                .child(native_rule(id).color(SystemColor::Label)),
        )
        .into_any_element()
}

/// The band: GPUI text on the left, the same strings on the right, one material over both.
fn hero(hosted: bool) -> AnyElement {
    let left = div()
        .absolute()
        .left(px(12.))
        .top(px(10.))
        .w(px(HALF))
        .flex()
        .flex_col()
        .gap_1()
        .child(caption("gpui text · under the material"))
        .children(
            SPECIMENS
                .iter()
                .map(|(text, size, weight)| gpui_line(text, *size, *weight)),
        )
        .child(gpui_hairline());

    // The right half is built twice from one list so the two arms differ in nothing but who draws
    // the glyphs — same strings, same sizes, same weights, same semantic colour.
    let right_ids = ["hero.a", "hero.b", "hero.c"];
    let right = div()
        .absolute()
        .left(px(BAND_W - HALF - 12.))
        .top(px(10.))
        .w(px(HALF))
        .flex()
        .flex_col()
        .gap_1()
        .child(caption(if hosted {
            "native_label · over the material"
        } else {
            "gpui text · the falsification arm"
        }))
        .children(
            SPECIMENS
                .iter()
                .zip(right_ids)
                .map(|((text, size, weight), id)| {
                    if hosted {
                        hosted_line(id, text, *size, *weight)
                    } else {
                        gpui_line(text, *size, *weight)
                    }
                }),
        )
        .child(if hosted {
            hosted_hairline("hero.rule")
        } else {
            gpui_hairline()
        });

    div()
        .relative()
        .w(px(BAND_W))
        .h(px(BAND_H))
        .bg(system_color(SystemColor::WindowBackground))
        .child(left)
        // Placed before the labels and after the GPUI content: a hosted view's z-order inside the
        // `Layer::Above` container is GPUI paint order, so anything placed after this lands on top
        // of it. `Blending::WithinWindow` is not optional — at the default the material samples
        // what is behind the *window*, which a window-scoped capture composites over nothing, so
        // the band reads as a tint bearing no relation to the content underneath. The tell that
        // `WithinWindow` is sampling at all is the confined lift over the ink's own x extent; a
        // tint that ignored the content would read identical there and in the inkless corridors.
        .child(
            div().absolute().inset_0().child(
                material("hero.glass", Material::Header)
                    .layer(Layer::Above)
                    .blending(Blending::WithinWindow)
                    .size_full(),
            ),
        )
        .child(right)
        .into_any_element()
}

/// Finder's header row, at Finder's own column widths, over the same material.
fn header_band() -> AnyElement {
    // One row of cells, each carrying its title and the divider that closes it — so the divider
    // sits exactly on the column boundary, which is where a resize gesture is aimed and therefore
    // where a claiming view would do the most damage.
    let cells =
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .children(
                COLUMNS
                    .iter()
                    .enumerate()
                    .map(|(i, (title, width, align))| {
                        div()
                            .w(px(*width))
                            .h_full()
                            .flex()
                            .items_center()
                            .child(
                                div().flex_1().px_2().child(
                                    native_label(("hdr", i), *title)
                                        .font_size(px(11.))
                                        .weight(FontWeight::MEDIUM)
                                        .color(SystemColor::SecondaryLabel)
                                        .align(*align),
                                ),
                            )
                            .child(
                                div().w(px(1.)).h(px(19.)).child(
                                    native_rule(("hdr.sep", i)).color(SystemColor::Separator),
                                ),
                            )
                    }),
            );

    div()
        .relative()
        .w(px(BAND_W))
        .h(px(28.))
        .child(
            div().absolute().inset_0().child(
                material("hdr.glass", Material::Header)
                    .layer(Layer::Above)
                    .blending(Blending::WithinWindow)
                    .size_full(),
            ),
        )
        .child(cells)
        .into_any_element()
}

fn boxed(width: f32, child: AnyElement) -> AnyElement {
    div()
        .w(px(width))
        .h(px(20.))
        .px_1()
        .border_1()
        .border_color(system_color(SystemColor::Separator))
        .child(child)
        .into_any_element()
}

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let (hosted, tabular) = (d.label.hosted, d.label.tabular);

    stack(vec![
        note(
            "A Layer::Above material blurs every GPUI pixel in its rect, its own caption included. \
             Text that has to stay sharp over one must be a real AppKit view placed after the \
             material — z-order inside the layer container is GPUI paint order.",
        ),
        demo("the ordering claim", hero(hosted)),
        row(
            "control",
            vec![
                native_switch("hosted", hosted)
                    .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                        d.label.hosted = *value;
                        cx.notify();
                    }))
                    .into_any_element(),
                caption(if hosted {
                    "right half is native_label — it must be sharp while the left half blurs"
                } else {
                    "right half is gpui text — both halves must blur"
                }),
            ],
        ),
        demo("a header band at Finder's column widths", header_band()),
        row(
            "align, in a box the label fills",
            [
                LabelAlign::Natural,
                LabelAlign::Left,
                LabelAlign::Center,
                LabelAlign::Right,
            ]
            .iter()
            .enumerate()
            .map(|(i, align)| {
                boxed(
                    120.,
                    native_label(("align", i), &format!("{align:?}"))
                        .font_size(px(11.))
                        .align(*align)
                        .into_any_element(),
                )
            })
            .collect(),
        ),
        demo(
            "tabular — ten 1s and ten 4s, right-aligned so the ragged pair is obvious",
            div()
                .flex()
                .flex_col()
                .gap_1()
                .children(
                    ["1111111111", "4444444444"]
                        .iter()
                        .enumerate()
                        .map(|(i, digits)| {
                            boxed(
                                120.,
                                native_label(("tab", i), digits)
                                    .tabular(tabular)
                                    .align(LabelAlign::Right)
                                    .into_any_element(),
                            )
                        }),
                )
                .into_any_element(),
        ),
        row(
            "tabular control",
            vec![
                native_switch("tabular", tabular)
                    .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                        d.label.tabular = *value;
                        cx.notify();
                    }))
                    .into_any_element(),
                caption(if tabular {
                    "on — both rows end at the same x"
                } else {
                    "off — at 13pt ten 4s run 87.00pt against ten 1s at 64.00pt"
                }),
            ],
        ),
        row(
            "truncates",
            vec![
                boxed(
                    90.,
                    native_label("trunc.on", "Portable Network Graphics image")
                        .font_size(px(11.))
                        .into_any_element(),
                ),
                boxed(
                    90.,
                    native_label("trunc.off", "Portable Network Graphics image")
                        .font_size(px(11.))
                        .truncates(false)
                        .into_any_element(),
                ),
                caption("tail ellipsis, then clipped mid-glyph"),
            ],
        ),
        row(
            "weight and colour, resolved as roles",
            [
                (FontWeight::NORMAL, SystemColor::Label),
                (FontWeight::MEDIUM, SystemColor::SecondaryLabel),
                (FontWeight::SEMIBOLD, SystemColor::TertiaryLabel),
                (FontWeight::BOLD, SystemColor::ControlAccent),
            ]
            .iter()
            .enumerate()
            .map(|(i, (weight, color))| {
                boxed(
                    130.,
                    native_label(("wc", i), "Date Modified")
                        .font_size(px(11.))
                        .weight(*weight)
                        .color(*color)
                        .into_any_element(),
                )
            })
            .collect(),
        ),
        demo(
            "the same digits GPUI's own tabular token draws, for comparison",
            div()
                .flex()
                .flex_col()
                .gap_1()
                .text_size(px(13.))
                .font(system_font_tabular(FontWeight::NORMAL))
                .text_color(system_color(SystemColor::Label))
                .child("1111111111")
                .child("4444444444")
                .into_any_element(),
        ),
        props(&[
            (
                "native_label(id, text)",
                "3 hero rows + 4 header titles".into(),
            ),
            ("font_size(Pixels)", "11 and 13".into()),
            (
                "weight(FontWeight)",
                "NORMAL, MEDIUM, SEMIBOLD, BOLD".into(),
            ),
            ("color(SystemColor)", "Label, SecondaryLabel, …".into()),
            ("align(LabelAlign)", "Natural, Left, Center, Right".into()),
            ("tabular(bool)", tabular.to_string()),
            ("truncates(bool)", "true, then false".into()),
            (
                "native_rule(id)",
                "1 hero hairline + 4 header dividers".into(),
            ),
            ("color(SystemColor)", "Label, Separator".into()),
        ]),
    ])
}
