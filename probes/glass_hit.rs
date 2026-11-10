//! Gate: a `glass_container` declines mouse points that belong to something else, and a glass
//! member still claims its own. Exits 0 pass, 1 fail.

#[path = "support/hit_probe.rs"]
mod hit_probe;

use std::process::ExitCode;

use gpui::{Context, Render, Window, div, prelude::*, px};
use hit_probe::{centre_in, class_of, covers, fail, find, gpui_view, hit_rows, report_due};
use native_gpui::{glass, glass_container, native_root, native_switch};
use objc2_foundation::NSPoint;

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
                .child(div().absolute().left(px(20.)).top(px(20.)).child(
                    native_switch("knob", self.on).on_change(
                        cx.listener(|probe: &mut Probe, checked: &bool, _, _| probe.on = *checked),
                    ),
                ))
                .child(
                    glass_container("batch").child(
                        div()
                            .absolute()
                            .left(px(150.))
                            .top(px(20.))
                            .w(px(90.))
                            .h(px(40.))
                            .child(glass("panel")),
                    ),
                ),
        )
    }
}

fn report(window: &Window) {
    let gpui = gpui_view(window);
    let content = unsafe { gpui.superview() }.expect("gpui's view is inside a content view");
    // Private glass subviews have zero frames, and decline every point, until laid out.
    content.layoutSubtreeIfNeeded();
    content.displayIfNeeded();

    let (Some(knob), Some(panel), Some(batch)) = (
        find(&content, "NSSwitch"),
        find(&content, "NSGlassEffectView"),
        find(&content, "NSGlassEffectContainerView"),
    ) else {
        fail("the library placed no switch, no glass panel, or no glass container");
        return;
    };
    println!("container class: {}", class_of(&batch));

    let knob_at = centre_in(&knob, &content);
    let panel_at = centre_in(&panel, &content);
    let batch_bounds = batch.bounds();
    if batch_bounds.size.width < 100. || batch_bounds.size.height < 100. {
        fail("the glass container is too small for its own empty area to be probed");
        return;
    }
    let empty_at = batch.convertPoint_toView(
        NSPoint::new(
            batch_bounds.origin.x + batch_bounds.size.width - 20.,
            batch_bounds.origin.y + batch_bounds.size.height - 20.,
        ),
        Some(&content),
    );

    // The container is unflipped between flipped views: a missed conversion mirrors y.
    let mirror_at = NSPoint::new(panel_at.x, content.bounds().size.height - panel_at.y);
    if covers(&panel, &content, mirror_at) || covers(&knob, &content, mirror_at) {
        fail("the mirrored member point lands on a hosted view; move the member or the window");
        return;
    }

    hit_rows(
        &gpui,
        &[
            (
                "over the switch, below the container",
                knob_at,
                &knob,
                false,
            ),
            ("over a glass member", panel_at, &panel, false),
            ("the member's point, y mirrored", mirror_at, &gpui, true),
            ("over bare GPUI inside the container", empty_at, &gpui, true),
        ],
        40,
        34,
    );
}

fn main() -> ExitCode {
    hit_probe::run(260., 200., || Probe {
        frames: 0,
        on: false,
    })
}
