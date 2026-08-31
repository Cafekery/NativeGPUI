//! A live editor for [`GlassRecipe`]: every input of the private `glassBackground` filter that
//! moves a pixel, on its own dial, over a checkerboard and over a hue sweep so refraction and
//! blur have structure to act on.
//!
//! Nothing clamps in the filter, so the ranges here are chosen to contain all four presets with
//! room either side rather than to bound anything.

use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Window};

#[cfg(feature = "private-api")]
use gpui::{Hsla, div, hsla, prelude::*, px};
#[cfg(feature = "private-api")]
use native_gpui::{
    ColorWellStyle, GlassRecipe, INERT_INPUTS, custom_glass, custom_glass_available,
    native_color_well, native_popup, native_slider, native_switch,
};

#[cfg(not(feature = "private-api"))]
#[derive(Default)]
pub struct State;

#[cfg(not(feature = "private-api"))]
pub fn render(_d: &mut Demo, _window: &mut Window, _cx: &mut Context<Demo>) -> AnyElement {
    stack(vec![note("Run with --features private-api")])
}

#[cfg(feature = "private-api")]
#[derive(Default)]
pub struct State {
    pub recipe: GlassRecipe,
}

#[cfg(feature = "private-api")]
const PRESETS: [(&str, fn() -> GlassRecipe); 4] = [
    ("Regular (dark)", GlassRecipe::regular),
    ("Regular (light)", GlassRecipe::regular_light),
    ("Clear", GlassRecipe::clear),
    ("Blob", GlassRecipe::blob),
];

#[cfg(feature = "private-api")]
pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let r = d.custom_glass.recipe;
    let available = custom_glass_available();
    let matched = PRESETS
        .iter()
        .find(|(_, build)| build() == r)
        .map_or("custom", |(name, _)| *name);

    let mut page: Vec<AnyElement> = Vec::new();

    if !available {
        page.push(note(
            "custom_glass_available() is false: at least one of CABackdropLayer, CASDFLayer, \
             CASDFElementLayer or CAFilter did not resolve, so nothing below is guaranteed to \
             reach a pixel. corner_radius is the only input the stock fallback honours.",
        ));
    }

    page.push(row(
        "specimen",
        vec![
            specimen(r),
            checker(
                260.,
                custom_glass("live-checker").recipe(r).into_any_element(),
            ),
        ],
    ));

    page.push(row(
        "preset",
        vec![preset_popup(cx), mono(format!("recipe == {matched}"))],
    ));

    page.push(section(
        "Fidelity against a stock NSGlassEffectView",
        vec![
            caption(
                "Recorded once against a key stock view, not re-measured when this page loads. \
                 Re-check it before trusting it after any change to build_filter.",
            ),
            caption(
                "Mean per-channel difference 2.49/255, against a 0.0000 noise floor between two \
                 captures of the stock view itself.",
            ),
            caption("Every filter input this writes is byte-identical to the stock view's."),
            caption(
                "The residual is two passes this does not build: a vibrantColorMatrix layer, and \
                 the specular rim.",
            ),
        ],
    ));

    page.push(section(
        "Field and geometry",
        vec![
            dial(
                "corner_radius",
                r.corner_radius,
                (0., 300.),
                |r, v| r.corner_radius = v,
                cx,
            ),
            dial(
                "ovalization",
                r.ovalization,
                (0., 1.),
                |r, v| r.ovalization = v,
                cx,
            ),
            dial(
                "margin_width",
                r.margin_width,
                (0., 200.),
                |r, v| r.margin_width = v,
                cx,
            ),
            dial(
                "backdrop_scale",
                r.backdrop_scale,
                (0.05, 1.),
                |r, v| r.backdrop_scale = v,
                cx,
            ),
            caption(
                "margin_width is how far outside its own bounds the backdrop samples, so at zero \
                 refraction has nothing to bend in from beyond the edge.",
            ),
        ],
    ));

    page.push(section(
        "Blur",
        vec![
            dial(
                "blur_radius",
                r.blur_radius,
                (0., 60.),
                |r, v| r.blur_radius = v,
                cx,
            ),
            dial(
                "blur_opacity0",
                r.blur_opacity0,
                (0., 1.),
                |r, v| r.blur_opacity0 = v,
                cx,
            ),
            dial(
                "blur_opacity1",
                r.blur_opacity1,
                (0., 1.),
                |r, v| r.blur_opacity1 = v,
                cx,
            ),
            dial(
                "blur_opacity2",
                r.blur_opacity2,
                (0., 1.),
                |r, v| r.blur_opacity2 = v,
                cx,
            ),
            dial(
                "blur_distance2",
                r.blur_distance2,
                (-1., 1.),
                |r, v| r.blur_distance2 = v,
                cx,
            ),
            dial(
                "blur_distance3",
                r.blur_distance3,
                (-1., 1.),
                |r, v| r.blur_distance3 = v,
                cx,
            ),
        ],
    ));

    page.push(section(
        "Refraction",
        vec![
            dial(
                "refraction_opacity",
                r.refraction_opacity,
                (0., 1.),
                |r, v| r.refraction_opacity = v,
                cx,
            ),
            dial(
                "inner_refraction_amount",
                r.inner_refraction_amount,
                (-300., 300.),
                |r, v| r.inner_refraction_amount = v,
                cx,
            ),
            dial(
                "inner_refraction_height",
                r.inner_refraction_height,
                (0., 200.),
                |r, v| r.inner_refraction_height = v,
                cx,
            ),
            dial(
                "outer_refraction_amount",
                r.outer_refraction_amount,
                (-300., 300.),
                |r, v| r.outer_refraction_amount = v,
                cx,
            ),
            dial(
                "outer_refraction_height",
                r.outer_refraction_height,
                (0., 200.),
                |r, v| r.outer_refraction_height = v,
                cx,
            ),
            dial(
                "refraction_distance0",
                r.refraction_distance0,
                (-1., 1.),
                |r, v| r.refraction_distance0 = v,
                cx,
            ),
            dial(
                "refraction_distance1",
                r.refraction_distance1,
                (-1., 1.),
                |r, v| r.refraction_distance1 = v,
                cx,
            ),
            caption("refraction_opacity gates both amounts: at zero the whole group is inert."),
        ],
    ));

    page.push(section(
        "Face",
        vec![
            dial(
                "face_opacity",
                r.face_opacity,
                (0., 1.),
                |r, v| r.face_opacity = v,
                cx,
            ),
            dial(
                "face_white",
                r.face_white,
                (0., 2.),
                |r, v| r.face_white = v,
                cx,
            ),
            dial(
                "face_black",
                r.face_black,
                (0., 1.),
                |r, v| r.face_black = v,
                cx,
            ),
            dial(
                "face_saturation",
                r.face_saturation,
                (0., 4.),
                |r, v| r.face_saturation = v,
                cx,
            ),
            fill("face_fill", r.face_fill, |r, c| r.face_fill = c, cx),
        ],
    ));

    page.push(section(
        "Bleed",
        vec![
            dial(
                "bleed_opacity",
                r.bleed_opacity,
                (0., 1.),
                |r, v| r.bleed_opacity = v,
                cx,
            ),
            dial(
                "bleed_amount",
                r.bleed_amount,
                (0., 200.),
                |r, v| r.bleed_amount = v,
                cx,
            ),
            dial(
                "bleed_height",
                r.bleed_height,
                (0., 200.),
                |r, v| r.bleed_height = v,
                cx,
            ),
            dial(
                "bleed_blur_radius",
                r.bleed_blur_radius,
                (0., 300.),
                |r, v| r.bleed_blur_radius = v,
                cx,
            ),
            dial(
                "bleed_distance0",
                r.bleed_distance0,
                (-1., 1.),
                |r, v| r.bleed_distance0 = v,
                cx,
            ),
            dial(
                "bleed_distance1",
                r.bleed_distance1,
                (-1., 1.),
                |r, v| r.bleed_distance1 = v,
                cx,
            ),
            flag(
                "bleed_darken_blend",
                r.bleed_darken_blend,
                |r, v| r.bleed_darken_blend = v,
                cx,
            ),
            dial(
                "bleed_white",
                r.bleed_white,
                (0., 2.),
                |r, v| r.bleed_white = v,
                cx,
            ),
            dial(
                "bleed_black",
                r.bleed_black,
                (0., 1.),
                |r, v| r.bleed_black = v,
                cx,
            ),
            dial(
                "bleed_saturation",
                r.bleed_saturation,
                (0., 4.),
                |r, v| r.bleed_saturation = v,
                cx,
            ),
            fill("bleed_fill", r.bleed_fill, |r, c| r.bleed_fill = c, cx),
            caption("bleed_opacity gates every other input in this group."),
        ],
    ));

    page.push(section(
        "Shadow",
        vec![
            dial(
                "shadow_radius",
                r.shadow_radius,
                (0., 300.),
                |r, v| r.shadow_radius = v,
                cx,
            ),
            dial(
                "shadow_offset.x",
                r.shadow_offset.x,
                (-50., 50.),
                |r, v| r.shadow_offset.x = v,
                cx,
            ),
            dial(
                "shadow_offset.y",
                r.shadow_offset.y,
                (-50., 50.),
                |r, v| r.shadow_offset.y = v,
                cx,
            ),
            dial(
                "shadow_amount",
                r.shadow_amount,
                (0., 200.),
                |r, v| r.shadow_amount = v,
                cx,
            ),
            dial(
                "shadow_height",
                r.shadow_height,
                (0., 200.),
                |r, v| r.shadow_height = v,
                cx,
            ),
            dial(
                "shadow_blur_radius",
                r.shadow_blur_radius,
                (0., 200.),
                |r, v| r.shadow_blur_radius = v,
                cx,
            ),
            dial(
                "shadow_opacity",
                r.shadow_opacity,
                (0., 1.),
                |r, v| r.shadow_opacity = v,
                cx,
            ),
            dial(
                "shadow_opacity_sdr",
                r.shadow_opacity_sdr,
                (0., 1.),
                |r, v| r.shadow_opacity_sdr = v,
                cx,
            ),
            dial(
                "shadow_vibrancy",
                r.shadow_vibrancy,
                (0., 2.),
                |r, v| r.shadow_vibrancy = v,
                cx,
            ),
            dial(
                "shadow_white",
                r.shadow_white,
                (0., 2.),
                |r, v| r.shadow_white = v,
                cx,
            ),
            dial(
                "shadow_black",
                r.shadow_black,
                (0., 1.),
                |r, v| r.shadow_black = v,
                cx,
            ),
            dial(
                "shadow_saturation",
                r.shadow_saturation,
                (0., 4.),
                |r, v| r.shadow_saturation = v,
                cx,
            ),
            fill("shadow_fill", r.shadow_fill, |r, c| r.shadow_fill = c, cx),
            caption(
                "shadow_opacity_sdr is the dominant one on an SDR display and is separately live \
                 from shadow_opacity.",
            ),
        ],
    ));

    page.push(section(
        "Tone and gradient",
        vec![
            flag(
                "holding_tone",
                r.holding_tone,
                |r, v| r.holding_tone = v,
                cx,
            ),
            dial(
                "holding_tone_white",
                r.holding_tone_white,
                (0., 2.),
                |r, v| r.holding_tone_white = v,
                cx,
            ),
            headroom(r.max_headroom, cx),
            dial(
                "gradient_distance0",
                r.gradient_distance0,
                (-1., 1.),
                |r, v| r.gradient_distance0 = v,
                cx,
            ),
            dial(
                "gradient_distance1",
                r.gradient_distance1,
                (-1., 1.),
                |r, v| r.gradient_distance1 = v,
                cx,
            ),
            caption(
                "holding_tone gates the four below it. max_headroom is log-scaled here because \
                 every preset ships 9999, meaning unclamped, and the range that clamps anything \
                 is under about 64.",
            ),
        ],
    ));

    page.push(section(
        "No dial",
        vec![
            caption(
                "Eight inputs the filter declares and NSGlassEffectView writes are absent from \
                 GlassRecipe: sweeping each one min to max against the regular preset moved \
                 nothing on macOS 26.5.",
            ),
            mono(INERT_INPUTS.join(", ")),
        ],
    ));

    page.push(props(&[
        ("custom_glass(id)", "two specimens".into()),
        ("recipe(GlassRecipe)", matched.into()),
        (
            "corner_radius(Pixels)",
            "shadowed here by recipe.corner_radius".into(),
        ),
        ("frosting_gpui()", "both specimens".into()),
        ("custom_glass_available()", available.to_string()),
        ("GlassRecipe fields", "50, every one on a control".into()),
        ("INERT_INPUTS", INERT_INPUTS.len().to_string()),
    ]));

    stack(page)
}

/// A hue sweep and a line of text for the glass to refract and frost. A flat fill would leave
/// every refraction and blur dial looking dead.
#[cfg(feature = "private-api")]
fn specimen(recipe: GlassRecipe) -> AnyElement {
    let bars = (0..24).map(|i| {
        div()
            .w(px(520. / 24.))
            .h(px(260.))
            .bg(hsla(i as f32 / 24., 0.85, 0.5, 1.))
    });

    div()
        .relative()
        .w(px(520.))
        .h(px(260.))
        .rounded_lg()
        .overflow_hidden()
        .child(div().absolute().inset_0().flex().children(bars))
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .text_2xl()
                .text_color(hsla(0., 0., 1., 0.9))
                .child("glassBackground"),
        )
        .child(
            div()
                .absolute()
                .left(px(60.))
                .top(px(45.))
                .w(px(400.))
                .h(px(170.))
                .child(custom_glass("live-stripes").recipe(recipe)),
        )
        .into_any_element()
}

/// A pull-down rather than a pop-up: AppKit keeps entry 0 on the face and hides it from the list,
/// so it never claims an edited recipe is still the preset it came from, and re-picking the row
/// you are already on still fires.
#[cfg(feature = "private-api")]
fn preset_popup(cx: &mut Context<Demo>) -> AnyElement {
    native_popup("preset", 0)
        .pull_down(true)
        .items(std::iter::once("Preset").chain(PRESETS.iter().map(|(name, _)| *name)))
        .on_change(cx.listener(|d: &mut Demo, index: &usize, _, cx| {
            if let Some((_, build)) = PRESETS.get(index.wrapping_sub(1)) {
                d.custom_glass.recipe = build();
                cx.notify();
            }
        }))
        .into_any_element()
}

#[cfg(feature = "private-api")]
fn dial(
    field: &'static str,
    value: f32,
    (min, max): (f64, f64),
    write: fn(&mut GlassRecipe, f32),
    cx: &mut Context<Demo>,
) -> AnyElement {
    knob(
        field,
        fmt(value),
        native_slider(field, value as f64)
            .range(min, max)
            .on_change(cx.listener(move |d: &mut Demo, value: &f64, _, cx| {
                write(&mut d.custom_glass.recipe, *value as f32);
                cx.notify();
            }))
            .into_any_element(),
    )
}

#[cfg(feature = "private-api")]
fn flag(
    field: &'static str,
    value: bool,
    write: fn(&mut GlassRecipe, bool),
    cx: &mut Context<Demo>,
) -> AnyElement {
    knob(
        field,
        value.to_string(),
        native_switch(field, value)
            .on_change(cx.listener(move |d: &mut Demo, value: &bool, _, cx| {
                write(&mut d.custom_glass.recipe, *value);
                cx.notify();
            }))
            .into_any_element(),
    )
}

/// The switch is the whole point: a `None` fill means the matrix takes no fill colour, which the
/// colour well cannot express — a fully transparent swatch is a clear fill, not an absent one.
#[cfg(feature = "private-api")]
fn fill(
    field: &'static str,
    value: Option<Hsla>,
    write: fn(&mut GlassRecipe, Option<Hsla>),
    cx: &mut Context<Demo>,
) -> AnyElement {
    let shown = value.unwrap_or(hsla(0., 0., 0.5, 0.4));
    knob(
        field,
        match value {
            None => "none".to_string(),
            Some(color) => format!("l {} a {}", fmt(color.l), fmt(color.a)),
        },
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                native_switch((field, 0usize), value.is_some()).on_change(cx.listener(
                    move |d: &mut Demo, on: &bool, _, cx| {
                        write(&mut d.custom_glass.recipe, on.then_some(shown));
                        cx.notify();
                    },
                )),
            )
            .child(
                native_color_well((field, 1usize), shown)
                    .style(ColorWellStyle::Minimal)
                    .enabled(value.is_some())
                    .on_change(cx.listener(move |d: &mut Demo, color: &Hsla, _, cx| {
                        write(&mut d.custom_glass.recipe, Some(*color));
                        cx.notify();
                    })),
            )
            .into_any_element(),
    )
}

/// Log-scaled, because 1 to 9999 linear puts the whole range that clamps anything in the first
/// pixel of travel.
#[cfg(feature = "private-api")]
fn headroom(value: f32, cx: &mut Context<Demo>) -> AnyElement {
    const TOP: f32 = 9999.;
    let travel = value.clamp(1., TOP).ln() / TOP.ln();

    knob(
        "max_headroom",
        fmt(value),
        native_slider("max_headroom", travel as f64)
            .range(0., 1.)
            .on_change(cx.listener(|d: &mut Demo, travel: &f64, _, cx| {
                d.custom_glass.recipe.max_headroom = TOP.powf(*travel as f32);
                cx.notify();
            }))
            .into_any_element(),
    )
}

#[cfg(feature = "private-api")]
fn fmt(value: f32) -> String {
    let text = format!("{value:.3}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    match trimmed {
        "" | "-" => "0".to_string(),
        text => text.to_string(),
    }
}
