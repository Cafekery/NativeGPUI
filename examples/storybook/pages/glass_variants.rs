use crate::Demo;
use crate::pages::glass::{specimen, stripes};
use crate::ui::*;
use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*, px};
use native_gpui::{
    Glass, GlassInteractionState, GlassSubvariant, GlassVariant, glass, native_popup,
    native_segmented, native_switch,
};

const TILE: (f32, f32) = (130., 90.);

#[derive(Default)]
pub struct State {
    pub variant: usize,
    pub subvariant: usize,
    pub interaction: usize,
    pub subdued: bool,
    pub scrim: bool,
    pub lensing: bool,
}

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let State {
        variant,
        subvariant,
        interaction,
        subdued,
        scrim,
        lensing,
    } = d.glass_variants;
    let variant_index = variant.min(GlassVariant::ALL.len() - 1);
    let variant = GlassVariant::ALL[variant_index];
    let accepted = variant.subvariants();
    let chosen = subvariant.min(accepted.len() - 1);
    let subvariant = accepted[chosen];
    let states = GlassInteractionState::ALL;
    let interaction_index = interaction.min(states.len() - 1);
    let interaction = states[interaction_index];

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
                    2. * TILE.0,
                    2. * TILE.1,
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
                    native_popup("variant", variant_index)
                        .items(GlassVariant::ALL.iter().map(|v| v.title()))
                        .on_change(set(cx, |d, value| {
                            d.glass_variants.variant = value;
                            d.glass_variants.subvariant = 0;
                        }))
                        .into_any_element(),
                ),
                knob(
                    "subvariant",
                    format!("{} of {}", chosen + 1, accepted.len()),
                    native_popup("subvariant", chosen)
                        .items(accepted.iter().map(|s| s.title()))
                        .enabled(accepted.len() > 1)
                        .on_change(set(cx, |d, value| d.glass_variants.subvariant = value))
                        .into_any_element(),
                ),
                knob(
                    "interaction_state",
                    interaction.title(),
                    native_segmented("interaction", interaction_index)
                        .segments(states.iter().map(|s| s.title()))
                        .on_change(set(cx, |d, value| d.glass_variants.interaction = value))
                        .into_any_element(),
                ),
                knob(
                    "subdued",
                    subdued.to_string(),
                    native_switch("subdued", subdued)
                        .on_change(set(cx, |d, value| d.glass_variants.subdued = value))
                        .into_any_element(),
                ),
                knob(
                    "scrim",
                    scrim.to_string(),
                    native_switch("scrim", scrim)
                        .on_change(set(cx, |d, value| d.glass_variants.scrim = value))
                        .into_any_element(),
                ),
                knob(
                    "content_lensing",
                    lensing.to_string(),
                    native_switch("lensing", lensing)
                        .on_change(set(cx, |d, value| d.glass_variants.lensing = value))
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
                        specimen("no panel — the control", TILE, div()),
                        panel("public regular", glass(("public", 0u32))),
                        panel("public clear(true)", glass(("public", 1u32)).clear(true)),
                    ]
                    .into_iter()
                    .chain(GlassVariant::ALL.iter().map(|&variant| {
                        panel(
                            variant_label(variant),
                            glass(("variant", variant.raw() as u32)).variant(variant),
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
                            panel(
                                subvariant.title(),
                                glass(("subvariant", i as u32))
                                    .variant(variant)
                                    .subvariant(subvariant),
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
                            panel(
                                state.title(),
                                glass(("interaction", state.raw() as u32)).interaction_state(state),
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
                    panel("subdued(true)", glass("flag-subdued").subdued(true)),
                    panel("scrim(true)", glass("flag-scrim").scrim(true)),
                    panel(
                        "content_lensing(true)",
                        glass("flag-lensing").content_lensing(true),
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

fn variant_label(variant: GlassVariant) -> String {
    match variant {
        GlassVariant::Keyboard => "Keyboard — draws nothing".into(),
        GlassVariant::Sidebar => "Sidebar — edge band only".into(),
        _ => format!("{} — raw {}", variant.title(), variant.raw()),
    }
}

fn named(subvariants: &[GlassSubvariant]) -> usize {
    subvariants
        .iter()
        .filter(|subvariant| subvariant.api_name().is_some())
        .count()
}

fn panel(label: impl Into<SharedString>, panel: Glass) -> AnyElement {
    specimen(label, TILE, panel.corner_radius(px(18.)))
}
