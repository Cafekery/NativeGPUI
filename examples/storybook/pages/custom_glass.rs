use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Hsla, Window, div, hsla, prelude::*, px};
use native_gpui::{
    ColorWellStyle, GlassRecipe, INERT_INPUTS, custom_glass, custom_glass_available,
    native_color_well, native_popup, native_slider, native_switch,
};

#[derive(Default)]
pub struct State {
    pub recipe: GlassRecipe,
}

type Preset = (&'static str, fn() -> GlassRecipe);

const PRESETS: [Preset; 4] = [
    ("Regular (dark)", GlassRecipe::regular),
    ("Regular (light)", GlassRecipe::regular_light),
    ("Clear", GlassRecipe::clear),
    ("Blob", GlassRecipe::blob),
];

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let r = d.custom_glass.recipe;
    let available = custom_glass_available();
    let matched = PRESETS
        .iter()
        .find(|(_, build)| build() == r)
        .map_or("custom", |(name, _)| *name);

    macro_rules! dial {
        ($field:ident, $min:expr, $max:expr) => {
            dial(
                stringify!($field),
                r.$field,
                ($min, $max),
                |r, v| r.$field = v,
                cx,
            )
        };
    }

    let mut page = Vec::new();

    if !available {
        page.push(note(
            "custom_glass_available() is false: at least one of CABackdropLayer, CASDFLayer, \
             CASDFElementLayer or CAFilter did not resolve, so nothing below is guaranteed to \
             reach a pixel. corner_radius is the only input the stock fallback honours.",
        ));
    }

    page.extend([
    row(
        "specimen",
        vec![
            specimen(r),
            checker(
                260.,
                custom_glass("live-checker").recipe(r).into_any_element(),
            ),
        ],
    ),
    row(
        "preset",
        vec![preset_popup(cx), mono(format!("recipe == {matched}"))],
    ),
    section(
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
    ),
    section(
        "Field and geometry",
        vec![
            dial!(corner_radius, 0., 300.),
            dial!(ovalization, 0., 1.),
            dial!(margin_width, 0., 200.),
            dial!(backdrop_scale, 0.05, 1.),
            caption(
                "margin_width is how far outside its own bounds the backdrop samples, so at zero \
                 refraction has nothing to bend in from beyond the edge.",
            ),
        ],
    ),
    section(
        "Blur",
        vec![
            dial!(blur_radius, 0., 60.),
            dial!(blur_opacity0, 0., 1.),
            dial!(blur_opacity1, 0., 1.),
            dial!(blur_opacity2, 0., 1.),
            dial!(blur_distance2, -1., 1.),
            dial!(blur_distance3, -1., 1.),
        ],
    ),
    section(
        "Refraction",
        vec![
            dial!(refraction_opacity, 0., 1.),
            dial!(inner_refraction_amount, -300., 300.),
            dial!(inner_refraction_height, 0., 200.),
            dial!(outer_refraction_amount, -300., 300.),
            dial!(outer_refraction_height, 0., 200.),
            dial!(refraction_distance0, -1., 1.),
            dial!(refraction_distance1, -1., 1.),
            caption("refraction_opacity gates both amounts: at zero the whole group is inert."),
        ],
    ),
    section(
        "Face",
        vec![
            dial!(face_opacity, 0., 1.),
            dial!(face_white, 0., 2.),
            dial!(face_black, 0., 1.),
            dial!(face_saturation, 0., 4.),
            fill("face_fill", r.face_fill, |r, c| r.face_fill = c, cx),
        ],
    ),
    section(
        "Bleed",
        vec![
            dial!(bleed_opacity, 0., 1.),
            dial!(bleed_amount, 0., 200.),
            dial!(bleed_height, 0., 200.),
            dial!(bleed_blur_radius, 0., 300.),
            dial!(bleed_distance0, -1., 1.),
            dial!(bleed_distance1, -1., 1.),
            flag(
                "bleed_darken_blend",
                r.bleed_darken_blend,
                |r, v| r.bleed_darken_blend = v,
                cx,
            ),
            dial!(bleed_white, 0., 2.),
            dial!(bleed_black, 0., 1.),
            dial!(bleed_saturation, 0., 4.),
            fill("bleed_fill", r.bleed_fill, |r, c| r.bleed_fill = c, cx),
            caption("bleed_opacity gates every other input in this group."),
        ],
    ),
    section(
        "Shadow",
        vec![
            dial!(shadow_radius, 0., 300.),
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
            dial!(shadow_amount, 0., 200.),
            dial!(shadow_height, 0., 200.),
            dial!(shadow_blur_radius, 0., 200.),
            dial!(shadow_opacity, 0., 1.),
            dial!(shadow_opacity_sdr, 0., 1.),
            dial!(shadow_vibrancy, 0., 2.),
            dial!(shadow_white, 0., 2.),
            dial!(shadow_black, 0., 1.),
            dial!(shadow_saturation, 0., 4.),
            fill("shadow_fill", r.shadow_fill, |r, c| r.shadow_fill = c, cx),
            caption(
                "shadow_opacity_sdr is the dominant one on an SDR display and is separately live \
                 from shadow_opacity.",
            ),
        ],
    ),
    section(
        "Tone and gradient",
        vec![
            flag(
                "holding_tone",
                r.holding_tone,
                |r, v| r.holding_tone = v,
                cx,
            ),
            dial!(holding_tone_white, 0., 2.),
            headroom(r.max_headroom, cx),
            dial!(gradient_distance0, -1., 1.),
            dial!(gradient_distance1, -1., 1.),
            caption(
                "holding_tone gates the four below it. max_headroom is log-scaled here because \
                 every preset ships 9999, meaning unclamped, and the range that clamps anything \
                 is under about 64.",
            ),
        ],
    ),
    section(
        "No dial",
        vec![
            caption(
                "Eight inputs the filter declares and NSGlassEffectView writes are absent from \
                 GlassRecipe: sweeping each one min to max against the regular preset moved \
                 nothing on macOS 26.5.",
            ),
            mono(INERT_INPUTS.join(", ")),
        ],
    ),
    props(&[
        ("custom_glass(id)", "two specimens".into()),
        ("recipe(GlassRecipe)", matched.into()),
        (
            "corner_radius(Pixels)",
            "unused here; overrides recipe.corner_radius in either order".into(),
        ),
        (
            "frosting_gpui() / frosting_desktop()",
            "neither is called here, so both specimens are on the frosting_gpui default — \
             Layer::Above, frosting the GPUI content beneath. frosting_desktop needs a rect the \
             GPUI scene leaves transparent, which this page has nowhere to put."
                .into(),
        ),
        ("custom_glass_available()", available.to_string()),
        ("GlassRecipe fields", "50, every one on a control".into()),
        ("INERT_INPUTS", INERT_INPUTS.len().to_string()),
    ]),
    ]);

    stack(page)
}

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

// A pull-down hides entry 0 from the list, so re-picking the row you are already on still fires.
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

// The switch expresses `None` — a fully transparent swatch is a clear fill, not an absent one.
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

fn fmt(value: f32) -> String {
    let mut text = format!("{value:.3}");
    text.truncate(text.trim_end_matches('0').trim_end_matches('.').len());
    text
}
