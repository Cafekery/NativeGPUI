//! `native_label` and `native_rule`: real AppKit text ordered above a material.

use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, FontWeight, SharedString, Window, div, prelude::*, px};
use native_gpui::tokens::{SystemColor, system_color, system_font, system_font_tabular};
use native_gpui::{
    Blending, LabelAlign, Layer, Material, material, native_label, native_rule, native_switch,
};

const COLUMNS: [(&str, f32, LabelAlign); 4] = [
    ("Name", 168., LabelAlign::Natural),
    ("Date Modified", 181., LabelAlign::Natural),
    ("Size", 97., LabelAlign::Right),
    ("Kind", 121., LabelAlign::Natural),
];

const BAND_W: f32 = 620.;
const BAND_H: f32 = 148.;
const HALF: f32 = 280.;

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
        // Z-order inside the `Layer::Above` container is GPUI paint order: `right` goes after.
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

fn header_band() -> AnyElement {
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
                                    native_label(("hdr", i), title)
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
