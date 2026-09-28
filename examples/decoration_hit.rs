//! Gate: the hosted decorations `material`, `native_label` and `native_rule` decline mouse points
//! that belong to GPUI, while a switch under them still claims its own. Exits 0 pass, 1 fail.

#[path = "support/hit_probe.rs"]
mod hit_probe;

use std::process::ExitCode;

use gpui::{Context, Render, Window, div, prelude::*, px};
use hit_probe::{centre_in, class_of, covers, fail, find, gpui_view, hit_rows, report_due};
use native_gpui::tokens::SystemColor;
use native_gpui::{
    Blending, Layer, Material, material, native_label, native_root, native_rule, native_switch,
};
use objc2_foundation::NSPoint;

const BAND_H: f32 = 60.;

struct Probe {
    frames: usize,
    on: bool,
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frames += 1;
        if report_due(self.frames) {
            report(window);
        }
        native_root(
            div()
                .size_full()
                .relative()
                .child(div().absolute().left(px(16.)).top(px(16.)).child(
                    native_switch("knob", self.on).on_change(
                        cx.listener(|probe: &mut Probe, checked: &bool, _, _| probe.on = *checked),
                    ),
                ))
                .child(
                    div()
                        .absolute()
                        .left(px(0.))
                        .top(px(0.))
                        .w_full()
                        .h(px(BAND_H))
                        .child(
                            material("band", Material::Header)
                                .layer(Layer::Above)
                                .blending(Blending::WithinWindow)
                                .size_full(),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(120.))
                        .top(px(20.))
                        .w(px(110.))
                        .h(px(18.))
                        .child(native_label("title", "Date Modified")),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(96.))
                        .top(px(14.))
                        .w(px(1.))
                        .h(px(30.))
                        .child(native_rule("divider").color(SystemColor::Separator)),
                ),
        )
    }
}

fn report(window: &Window) {
    let gpui = gpui_view(window);
    let content = unsafe { gpui.superview() }.expect("gpui's view is inside a content view");
    content.layoutSubtreeIfNeeded();
    content.displayIfNeeded();

    let (Some(knob), Some(band), Some(label), Some(rule)) = (
        find(&content, "NSSwitch"),
        find(&content, "NSVisualEffectView"),
        find(&content, "NSTextField"),
        find(&content, "NSBox"),
    ) else {
        fail("the library placed no switch, no material, no label or no rule");
        return;
    };
    println!(
        "classes: material {}  label {}  rule {}",
        class_of(&band),
        class_of(&label),
        class_of(&rule)
    );

    let knob_at = centre_in(&knob, &content);
    let label_at = centre_in(&label, &content);
    let rule_at = centre_in(&rule, &content);
    let band_rect = band.convertRect_toView(band.bounds(), Some(&content));
    let bare_at = NSPoint::new(
        band_rect.origin.x + band_rect.size.width - 12.,
        band_rect.origin.y + band_rect.size.height / 2.,
    );

    for (what, at) in [
        ("the switch", knob_at),
        ("the label", label_at),
        ("the rule", rule_at),
        ("the bare band point", bare_at),
    ] {
        if !covers(&band, &content, at) {
            fail(&format!(
                "{what} at {:.0},{:.0} is outside the material, so its row cannot fail",
                at.x, at.y
            ));
            return;
        }
    }
    if covers(&label, &content, bare_at) || covers(&rule, &content, bare_at) {
        fail("the bare band point lands on the label or the rule");
        return;
    }
    if covers(&label, &content, knob_at) || covers(&rule, &content, knob_at) {
        fail("the switch is under the label or the rule, so its row tests the wrong view");
        return;
    }

    hit_rows(
        &gpui,
        &[
            ("over the switch, under the material", knob_at, &knob, false),
            ("over a hosted label", label_at, &gpui, true),
            ("over a hosted rule", rule_at, &gpui, true),
            ("over the material, on nothing else", bare_at, &gpui, true),
        ],
        44,
        26,
    );
}

fn main() -> ExitCode {
    hit_probe::run(280., 160., || Probe {
        frames: 0,
        on: false,
    })
}
