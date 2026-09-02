//! The `NSVisualEffectView` catalogue and the knobs that change how one draws.

use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, ElementId, Window, prelude::*, px};
use native_gpui::tokens::{SystemColor, system_color};
use native_gpui::{
    Blending, Layer, RawMaterial, material, native_popup, native_segmented, native_slider,
    native_switch,
};

const SPECIMEN: f32 = 128.;
const HERO: f32 = 240.;

const REFERENCE: RawMaterial = RawMaterial::HudWindow;

pub struct State {
    pub selected: usize,
    pub within_window: bool,
    pub emphasized: bool,
    pub radius: f64,
    pub dims_when_inactive: bool,
    pub above: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            selected: RawMaterial::all().position(|m| m == REFERENCE).unwrap_or(0),
            within_window: true,
            emphasized: false,
            radius: 12.,
            dims_when_inactive: false,
            above: false,
        }
    }
}

fn over_checker(id: impl Into<ElementId>, m: RawMaterial) -> AnyElement {
    checker(
        SPECIMEN,
        material(id, m)
            .layer(Layer::Above)
            .blending(Blending::WithinWindow)
            .size_full()
            .into_any_element(),
    )
}

fn catalogue_tile(m: RawMaterial) -> AnyElement {
    let label = if m.is_deprecated() {
        format!("{} · deprecated", m.title())
    } else {
        m.title().to_string()
    };
    tile(label, SPECIMEN, over_checker(m.title(), m))
}

fn collisions(id: &'static str, group: &[RawMaterial]) -> AnyElement {
    grid(
        group
            .iter()
            .enumerate()
            .map(|(i, m)| tile(m.title(), SPECIMEN, over_checker((id, i), *m)))
            .collect(),
    )
}

#[cfg(feature = "private-api")]
fn undocumented() -> Vec<AnyElement> {
    vec![
        caption(
            "Nine materials NSVisualEffectView draws that AppKit never named. They render today \
             and an OS update is free to change them.",
        ),
        grid(
            RawMaterial::UNDOCUMENTED
                .iter()
                .copied()
                .map(catalogue_tile)
                .collect(),
        ),
        caption(
            "artificial_chameleon is this page's positive control: it flattens the material to an \
             opaque dark fill, backdrop and vibrancy gone. The right-hand tile is what a collapsed \
             material looks like, so every tile still showing the checkerboard is sampling.",
        ),
        grid(vec![
            tile(
                "artificial_chameleon(false)",
                SPECIMEN,
                over_checker("chameleon-off", REFERENCE),
            ),
            tile(
                "artificial_chameleon(true)",
                SPECIMEN,
                checker(
                    SPECIMEN,
                    material("chameleon-on", REFERENCE)
                        .layer(Layer::Above)
                        .blending(Blending::WithinWindow)
                        .artificial_chameleon(true)
                        .size_full()
                        .into_any_element(),
                ),
            ),
        ]),
    ]
}

#[cfg(not(feature = "private-api"))]
fn undocumented() -> Vec<AnyElement> {
    vec![note("Run with --features private-api")]
}

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let materials: Vec<RawMaterial> = RawMaterial::all().collect();
    let index = d.material.selected.min(materials.len() - 1);
    let chosen = materials[index];
    let within = d.material.within_window;
    let emphasized = d.material.emphasized;
    let radius = d.material.radius;
    let dims = d.material.dims_when_inactive;
    let above = d.material.above;
    let layer_name = if above { "Above" } else { "Below" };
    let deprecated = RawMaterial::DOCUMENTED
        .iter()
        .filter(|m| m.is_deprecated())
        .count();

    stack(vec![
        caption(
            "Every specimen over a checkerboard is layer(Layer::Above) with \
             blending(Blending::WithinWindow), so the checkerboard under it is what the material \
             samples. A material that is sampling takes the checkerboard's own average and keeps \
             its softened edges; one that has collapsed reads as an even system fill.",
        ),
        section(
            &format!(
                "Documented — {} materials, {deprecated} of them deprecated",
                RawMaterial::DOCUMENTED.len()
            ),
            vec![grid(
                RawMaterial::DOCUMENTED
                    .iter()
                    .copied()
                    .map(catalogue_tile)
                    .collect(),
            )],
        ),
        section(
            "Undocumented — nine more, behind private-api",
            undocumented(),
        ),
        section(
            "Exact collisions",
            vec![
                caption(
                    "The documented set is not 19 distinct looks. Each group below is laid out \
                     adjacent over one checkerboard so the claim is checkable here rather than \
                     taken on trust — if two tiles in a group differ, the grouping is wrong.",
                ),
                caption(
                    "Sidebar and Under Window Background are one surface on macOS 26, which is \
                     why the Material::Sidebar role maps onto it. Appearance Based and Tool Tip \
                     land on the same look:",
                ),
                collisions(
                    "collide-window",
                    &[
                        RawMaterial::AppearanceBased,
                        RawMaterial::Sidebar,
                        RawMaterial::ToolTip,
                        RawMaterial::UnderWindowBackground,
                    ],
                ),
                caption(
                    "And the opaque set — no blur at all, so the checkerboard vanishes under \
                     every one of them. Material::Card is Content Background; if you wanted blur \
                     you wanted Material::Popover:",
                ),
                collisions(
                    "collide-content",
                    &[
                        RawMaterial::Sheet,
                        RawMaterial::WindowBackground,
                        RawMaterial::ContentBackground,
                        RawMaterial::UnderPageBackground,
                    ],
                ),
            ],
        ),
        section(
            "blending(Blending)",
            vec![
                caption(
                    "BehindWindow is the default and samples past the window entirely, which is \
                     why the left tile hides the checkerboard rather than blurring it. \
                     WithinWindow costs several times as much and is billed in the WindowServer, \
                     so a CPU-side measurement of this page calls it free. It is also a no-op at \
                     Layer::Below: nothing in the window is beneath the view, so AppKit falls \
                     back to the window's backdrop.",
                ),
                grid(vec![
                    tile(
                        "BehindWindow — the desktop",
                        SPECIMEN,
                        checker(
                            SPECIMEN,
                            material("blend-behind", REFERENCE)
                                .layer(Layer::Above)
                                .size_full()
                                .into_any_element(),
                        ),
                    ),
                    tile(
                        "WithinWindow — the checkerboard",
                        SPECIMEN,
                        over_checker("blend-within", REFERENCE),
                    ),
                ]),
            ],
        ),
        section(
            "layer(Layer)",
            vec![
                caption(
                    "Below is the default and is what lets GPUI content composite over a \
                     material. Above covers every GPUI pixel in its rect, so the middle tile \
                     paints the same text and then hides it — children do not combine with \
                     Above. The third tile follows the knob below.",
                ),
                grid(vec![
                    tile(
                        "Below — GPUI text on top",
                        SPECIMEN,
                        material("layer-below", REFERENCE)
                            .size(px(SPECIMEN))
                            .p_2()
                            .child("GPUI text")
                            .into_any_element(),
                    ),
                    tile(
                        "Above — covers the same text",
                        SPECIMEN,
                        material("layer-above", REFERENCE)
                            .layer(Layer::Above)
                            .size(px(SPECIMEN))
                            .p_2()
                            .child("GPUI text")
                            .into_any_element(),
                    ),
                    tile(
                        format!("knob — layer({layer_name})"),
                        SPECIMEN,
                        material("layer-live", REFERENCE)
                            .layer(if above { Layer::Above } else { Layer::Below })
                            .size(px(SPECIMEN))
                            .p_2()
                            .child("GPUI text")
                            .into_any_element(),
                    ),
                ]),
                knob(
                    "layer",
                    layer_name,
                    native_segmented("layer-knob", usize::from(above))
                        .segments(["Below", "Above"])
                        .on_change(cx.listener(|d: &mut Demo, value: &usize, _, cx| {
                            d.material.above = *value == 1;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
            ],
        ),
        section(
            "emphasized(bool)",
            vec![
                caption(
                    "emphasized is AppKit's \"first responder is on this surface\" look, and on \
                     Selection it is what turns the accent tint on. Without it a focused row \
                     renders grey.",
                ),
                grid(vec![
                    tile(
                        "Selection — emphasized(false)",
                        SPECIMEN,
                        over_checker("emph-off", RawMaterial::Selection),
                    ),
                    tile(
                        "Selection — emphasized(true)",
                        SPECIMEN,
                        checker(
                            SPECIMEN,
                            material("emph-on", RawMaterial::Selection)
                                .layer(Layer::Above)
                                .blending(Blending::WithinWindow)
                                .emphasized(true)
                                .size_full()
                                .into_any_element(),
                        ),
                    ),
                ]),
            ],
        ),
        section(
            "corner_radius(Pixels)",
            vec![
                caption(
                    "The radius is applied as the view's maskImage, a nine-part resizable rounded \
                     rect. layer.cornerRadius would also clip a behind-window backdrop, but it \
                     measures a softer arc and clips subviews with it.",
                ),
                grid(
                    [0., 10., 20., 30.]
                        .iter()
                        .enumerate()
                        .map(|(i, r)| {
                            tile(
                                format!("{r:.0} px"),
                                SPECIMEN,
                                checker(
                                    SPECIMEN,
                                    material(("radius-sample", i), REFERENCE)
                                        .layer(Layer::Above)
                                        .blending(Blending::WithinWindow)
                                        .corner_radius(px(*r))
                                        .size_full()
                                        .into_any_element(),
                                ),
                            )
                        })
                        .collect(),
                ),
            ],
        ),
        section(
            "dims_when_inactive(bool)",
            vec![
                caption(
                    "Click another window: the right tile collapses to a flat fill and the left \
                     one does not. A non-key NSVisualEffectView flattens, which is why this crate \
                     pins Active by default — a sidebar should not go grey the moment you click \
                     away — and why every specimen on this page sits on a checkerboard.",
                ),
                grid(vec![
                    tile(
                        "false — pins Active",
                        SPECIMEN,
                        over_checker("dims-off", REFERENCE),
                    ),
                    tile(
                        "true — AppKit's default",
                        SPECIMEN,
                        checker(
                            SPECIMEN,
                            material("dims-on", REFERENCE)
                                .layer(Layer::Above)
                                .blending(Blending::WithinWindow)
                                .dims_when_inactive(true)
                                .size_full()
                                .into_any_element(),
                        ),
                    ),
                ]),
            ],
        ),
        section(
            "Styled — .bg(), borders, radius",
            vec![
                caption(
                    "NativeMaterial is Styled and ParentElement like div(), and the style is \
                     painted rather than only laid out. A .bg() or .border_1() that silently drew \
                     nothing would fail only visually — it compiles, it type-checks, and the \
                     layout even insets by the border width, so the one symptom is a missing \
                     line.",
                ),
                caption(
                    "These four are at the default Layer::Below, so GPUI's paint lands over the \
                     material — which is exactly what a style has to do to be visible at all. No \
                     checkerboard here: at Below the material is behind the Metal view, so a \
                     checkerboard would cover it rather than be sampled by it.",
                ),
                grid(vec![
                    tile(
                        "no style — untouched",
                        SPECIMEN,
                        material("style-none", REFERENCE)
                            .size(px(SPECIMEN))
                            .into_any_element(),
                    ),
                    tile(
                        ".border_2() — a GPUI line on it",
                        SPECIMEN,
                        material("style-border", REFERENCE)
                            .size(px(SPECIMEN))
                            .border_2()
                            .border_color(system_color(SystemColor::ControlAccent))
                            .into_any_element(),
                    ),
                    tile(
                        ".bg() — hides the material",
                        SPECIMEN,
                        material("style-bg", REFERENCE)
                            .size(px(SPECIMEN))
                            .bg(system_color(SystemColor::ControlAccent))
                            .into_any_element(),
                    ),
                    tile(
                        "same .bg(), but layer(Above)",
                        SPECIMEN,
                        material("style-bg-above", REFERENCE)
                            .layer(Layer::Above)
                            .size(px(SPECIMEN))
                            .bg(system_color(SystemColor::ControlAccent))
                            .into_any_element(),
                    ),
                ]),
                caption(
                    "The first has no fill written, and Style::paint skips a transparent \
                     background, so a material nobody has styled is untouched. The third is the \
                     trap: an opaque fill on a material hides the material and draws a third \
                     window surface — see the Backgrounds page. The fourth writes the same fill \
                     and looks like the first, because at Above the material covers every GPUI \
                     pixel in its rect and the fill is one of them. A style, like a child, does \
                     not combine with Above.",
                ),
                caption(
                    "Only NativeMaterial does this. glass() and custom_glass() are not Styled, so \
                     their size comes from the layout around them and neither takes a .bg().",
                ),
            ],
        ),
        section(
            "Customiser",
            vec![
                checker(
                    HERO,
                    material("preview", chosen)
                        .layer(Layer::Above)
                        .blending(if within {
                            Blending::WithinWindow
                        } else {
                            Blending::BehindWindow
                        })
                        .emphasized(emphasized)
                        .corner_radius(px(radius as f32))
                        .dims_when_inactive(dims)
                        .size_full()
                        .into_any_element(),
                ),
                knob(
                    "material",
                    chosen.title(),
                    native_popup("picker", index)
                        .items(materials.iter().map(|m| m.title()))
                        .on_change(cx.listener(|d: &mut Demo, value: &usize, _, cx| {
                            d.material.selected = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "blending",
                    if within {
                        "WithinWindow"
                    } else {
                        "BehindWindow"
                    },
                    native_segmented("blending-knob", usize::from(within))
                        .segments(["BehindWindow", "WithinWindow"])
                        .on_change(cx.listener(|d: &mut Demo, value: &usize, _, cx| {
                            d.material.within_window = *value == 1;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "emphasized",
                    emphasized.to_string(),
                    native_switch("emphasized-knob", emphasized)
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                            d.material.emphasized = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "corner_radius",
                    format!("{radius:.0} px"),
                    native_slider("radius-knob", radius)
                        .range(0., 40.)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.material.radius = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "dims_when_inactive",
                    dims.to_string(),
                    native_switch("dims-knob", dims)
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                            d.material.dims_when_inactive = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
            ],
        ),
        props(&[
            ("material(id, RawMaterial)", chosen.title().to_string()),
            (
                "RawMaterial::DOCUMENTED",
                format!(
                    "{} materials, {deprecated} deprecated",
                    RawMaterial::DOCUMENTED.len()
                ),
            ),
            (
                "RawMaterial::UNDOCUMENTED",
                format!(
                    "{} materials, private-api only",
                    RawMaterial::UNDOCUMENTED.len()
                ),
            ),
            (
                "RawMaterial::all()",
                format!("{} in this build", materials.len()),
            ),
            (
                "blending(Blending)",
                if within {
                    "WithinWindow".into()
                } else {
                    "BehindWindow".into()
                },
            ),
            ("emphasized(bool)", emphasized.to_string()),
            ("corner_radius(Pixels)", format!("{radius:.0} px")),
            ("dims_when_inactive(bool)", dims.to_string()),
            (
                "layer(Layer)",
                format!("{layer_name} on the knob specimen; Above on the checkerboards"),
            ),
            (
                "RawMaterial::title() / is_deprecated()",
                "label every tile above".into(),
            ),
            (
                "Material",
                "nine named roles that resolve into this catalogue".into(),
            ),
            (
                "Styled — .bg() / .border_*() / .rounded_*()",
                "painted, not just laid out; a transparent fill is skipped".into(),
            ),
            (
                "ParentElement — .child()",
                "GPUI content, over the material at Layer::Below only".into(),
            ),
        ]),
    ])
}
