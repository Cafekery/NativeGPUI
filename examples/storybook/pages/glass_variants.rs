use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Window};

#[cfg(feature = "private-api")]
use crate::pages::glass::stripes;
#[cfg(feature = "private-api")]
use gpui::{div, prelude::*, px};
#[cfg(feature = "private-api")]
use native_gpui::{
    GlassInteractionState, GlassSubvariant, GlassVariant, glass, native_popup, native_segmented,
    native_switch,
};

#[cfg(feature = "private-api")]
const TILE_W: f32 = 130.;
#[cfg(feature = "private-api")]
const TILE_H: f32 = 90.;

#[cfg(feature = "private-api")]
#[derive(Default)]
pub struct State {
    pub variant: usize,
    /// Index into the *accepted* subvariants of the current variant, not [`GlassSubvariant::ALL`].
    pub subvariant: usize,
    pub interaction: usize,
    pub subdued: bool,
    pub scrim: bool,
    pub lensing: bool,
}

#[cfg(not(feature = "private-api"))]
#[derive(Default)]
pub struct State;

#[cfg(not(feature = "private-api"))]
pub fn render(_d: &mut Demo, _window: &mut Window, _cx: &mut Context<Demo>) -> AnyElement {
    stack(vec![note("Run with --features private-api")])
}

#[cfg(feature = "private-api")]
pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let variant = GlassVariant::ALL[d.glass_variants.variant.min(GlassVariant::ALL.len() - 1)];
    let accepted = variant.subvariants();
    let chosen = d.glass_variants.subvariant.min(accepted.len() - 1);
    let subvariant = accepted[chosen];
    let states = GlassInteractionState::ALL;
    let interaction = states[d.glass_variants.interaction.min(states.len() - 1)];
    let (subdued, scrim, lensing) = (
        d.glass_variants.subdued,
        d.glass_variants.scrim,
        d.glass_variants.lensing,
    );

    stack(vec![
        note(
            "style reads back as Regular under every one of these overrides, so nothing here can \
             report which recipe is live. Every label is the value that was written, not a value \
             that was read.",
        ),
        section(
            "live panel",
            vec![
                stripes(
                    2. * TILE_W,
                    2. * TILE_H,
                    glass("live")
                        .variant(variant)
                        .subvariant(subvariant)
                        .interaction_state(interaction)
                        .subdued(subdued)
                        .scrim(scrim)
                        .content_lensing(lensing)
                        .corner_radius(px(22.))
                        .into_any_element(),
                ),
                knob(
                    "variant",
                    format!("raw {}", variant.raw()),
                    native_popup(
                        "variant",
                        d.glass_variants.variant.min(GlassVariant::ALL.len() - 1),
                    )
                    .items(GlassVariant::ALL.iter().map(|v| v.title()))
                    .on_change(cx.listener(|d: &mut Demo, value: &usize, _, cx| {
                        d.glass_variants.variant = *value;
                        d.glass_variants.subvariant = 0;
                        cx.notify();
                    }))
                    .into_any_element(),
                ),
                knob(
                    "subvariant",
                    format!("{} of {}", chosen + 1, accepted.len()),
                    native_popup("subvariant", chosen)
                        .items(accepted.iter().map(|s| s.title()))
                        .enabled(accepted.len() > 1)
                        .on_change(cx.listener(|d: &mut Demo, value: &usize, _, cx| {
                            d.glass_variants.subvariant = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "interaction_state",
                    interaction.title(),
                    native_segmented(
                        "interaction",
                        d.glass_variants.interaction.min(states.len() - 1),
                    )
                    .segments(states.iter().map(|s| s.title()))
                    .on_change(cx.listener(|d: &mut Demo, value: &usize, _, cx| {
                        d.glass_variants.interaction = *value;
                        cx.notify();
                    }))
                    .into_any_element(),
                ),
                knob(
                    "subdued",
                    subdued.to_string(),
                    native_switch("subdued", subdued)
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                            d.glass_variants.subdued = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "scrim",
                    scrim.to_string(),
                    native_switch("scrim", scrim)
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                            d.glass_variants.scrim = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "content_lensing",
                    lensing.to_string(),
                    native_switch("lensing", lensing)
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                            d.glass_variants.lensing = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                caption(
                    "The subvariant list is GlassVariant::subvariants(); an unaccepted pairing is \
                     dropped to None before it is sent, so the control never offers one.",
                ),
            ],
        ),
        section(
            "the thirteen render classes",
            vec![
                grid(
                    [
                        tile(
                            "no panel — the control",
                            TILE_W,
                            stripes(TILE_W, TILE_H, div().into_any_element()),
                        ),
                        tile(
                            "public regular",
                            TILE_W,
                            specimen(("public", 0), None, false),
                        ),
                        tile(
                            "public clear(true)",
                            TILE_W,
                            specimen(("public", 1), None, true),
                        ),
                    ]
                    .into_iter()
                    .chain(GlassVariant::ALL.iter().map(|&variant| {
                        tile(
                            variant_label(variant),
                            TILE_W,
                            specimen(("variant", variant.raw() as u32), Some(variant), false),
                        )
                    }))
                    .collect(),
                ),
                caption(
                    "Every tile carries the same backdrop, so a difference between two of them is \
                     the variant and nothing else — and a variant whose tile matches the no-panel \
                     control drew nothing.",
                ),
                note(
                    "Keyboard draws nothing at all over content in the same window, and Sidebar \
                     draws only within about 24pt of the panel edge. Neither is broken; both want \
                     a surface this page is not.",
                ),
            ],
        ),
        section(
            &format!("subvariants accepted by {}", variant.title()),
            vec![
                grid(
                    accepted
                        .iter()
                        .enumerate()
                        .map(|(i, &subvariant)| {
                            tile(
                                subvariant.title(),
                                TILE_W,
                                stripes(
                                    TILE_W,
                                    TILE_H,
                                    glass(("subvariant", i as u32))
                                        .variant(variant)
                                        .subvariant(subvariant)
                                        .corner_radius(px(18.))
                                        .into_any_element(),
                                ),
                            )
                        })
                        .collect(),
                ),
                caption(format!(
                    "{} of the {} names patch this variant, and None — always accepted — is the \
                     absence of one. An explicit variant writes a whole recipe and a subvariant \
                     only patches what it leaves open, so most go inert; the property is an open \
                     string enum, so a name outside the set is accepted and draws nothing.",
                    named(accepted),
                    named(GlassSubvariant::ALL),
                )),
            ],
        ),
        section(
            "interaction states",
            vec![
                grid(
                    states
                        .iter()
                        .map(|&state| {
                            tile(
                                state.title(),
                                TILE_W,
                                stripes(
                                    TILE_W,
                                    TILE_H,
                                    glass(("interaction", state.raw() as u32))
                                        .interaction_state(state)
                                        .corner_radius(px(18.))
                                        .into_any_element(),
                                ),
                            )
                        })
                        .collect(),
                ),
                note(
                    "A closed enum rather than an integer for one reason: AppKit switches over \
                     _interactionState exhaustively, and a value outside 0...2 traps uncatchably \
                     and takes the process with it. The crate clamps on the way out, and every \
                     index on this page is clamped before it selects a state.",
                ),
            ],
        ),
        section(
            "the three flags",
            vec![
                grid(vec![
                    tile(
                        "subdued(true)",
                        TILE_W,
                        stripes(
                            TILE_W,
                            TILE_H,
                            glass("flag-subdued")
                                .subdued(true)
                                .corner_radius(px(18.))
                                .into_any_element(),
                        ),
                    ),
                    tile(
                        "scrim(true)",
                        TILE_W,
                        stripes(
                            TILE_W,
                            TILE_H,
                            glass("flag-scrim")
                                .scrim(true)
                                .corner_radius(px(18.))
                                .into_any_element(),
                        ),
                    ),
                    tile(
                        "content_lensing(true)",
                        TILE_W,
                        stripes(
                            TILE_W,
                            TILE_H,
                            glass("flag-lensing")
                                .content_lensing(true)
                                .corner_radius(px(18.))
                                .into_any_element(),
                        ),
                    ),
                ]),
                caption(
                    "Each draws at exactly one value and at no other, which is why they are \
                     booleans here rather than the integers AppKit takes.",
                ),
            ],
        ),
        props(&[
            (
                "variant(GlassVariant)",
                format!("{} — raw {}", variant.title(), variant.raw()),
            ),
            ("subvariant(GlassSubvariant)", subvariant.title().into()),
            (
                "GlassVariant::accepts(..)",
                format!(
                    "{} of {} values, {} named",
                    accepted.len(),
                    GlassSubvariant::ALL.len(),
                    named(accepted),
                ),
            ),
            ("interaction_state(..)", interaction.title().into()),
            ("subdued(bool)", subdued.to_string()),
            ("scrim(bool)", scrim.to_string()),
            ("content_lensing(bool)", lensing.to_string()),
            (
                "GlassVariant::ALL",
                format!("{} render classes", GlassVariant::ALL.len()),
            ),
            ("style read-back", "Regular under every override".into()),
        ]),
    ])
}

#[cfg(feature = "private-api")]
fn variant_label(variant: GlassVariant) -> String {
    match variant {
        GlassVariant::Keyboard => "Keyboard — draws nothing".into(),
        GlassVariant::Sidebar => "Sidebar — edge band only".into(),
        _ => format!("{} — raw {}", variant.title(), variant.raw()),
    }
}

#[cfg(feature = "private-api")]
fn named(subvariants: &[GlassSubvariant]) -> usize {
    subvariants
        .iter()
        .filter(|subvariant| subvariant.api_name().is_some())
        .count()
}

#[cfg(feature = "private-api")]
fn specimen(id: (&'static str, u32), variant: Option<GlassVariant>, clear: bool) -> AnyElement {
    let panel = glass(id).clear(clear).corner_radius(px(18.));
    stripes(
        TILE_W,
        TILE_H,
        match variant {
            Some(variant) => panel.variant(variant).into_any_element(),
            None => panel.into_any_element(),
        },
    )
}
