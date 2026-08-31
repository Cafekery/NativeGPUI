use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Window, div, prelude::*, px};
use native_gpui::native_slider;

pub struct State {
    pub continuous: f64,
    pub ranged: f64,
    pub ticked: f64,
    pub snapped: f64,
    pub vertical: f64,
    pub vertical_ticked: f64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            continuous: 0.35,
            ranged: 6.,
            ticked: 40.,
            snapped: 0.5,
            vertical: 0.7,
            vertical_ticked: 0.5,
        }
    }
}

/// A vertical slider gets its length from layout: `fittingSize` reports the free axis as zero, so
/// without an explicit height the specimen collapses to nothing.
fn upright(height: f32, child: AnyElement, value: String) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().h(px(height)).flex().child(child))
        .child(mono(value))
        .into_any_element()
}

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let s = &d.slider;
    let (continuous, ranged, ticked) = (s.continuous, s.ranged, s.ticked);
    let (snapped, vertical, vertical_ticked) = (s.snapped, s.vertical, s.vertical_ticked);

    stack(vec![
        section(
            "Horizontal",
            vec![
                knob(
                    "continuous",
                    format!("{continuous:.3}"),
                    native_slider("continuous", continuous)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.slider.continuous = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "range(-20, 20)",
                    format!("{ranged:+.1} dB"),
                    native_slider("ranged", ranged)
                        .range(-20., 20.)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.slider.ranged = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "ticks(11), free",
                    format!("{ticked:.2}"),
                    native_slider("ticked", ticked)
                        .range(0., 100.)
                        .ticks(11)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.slider.ticked = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "ticks(5), snapping",
                    format!("{snapped:.3}"),
                    native_slider("snapped", snapped)
                        .ticks(5)
                        .snap_to_ticks(true)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.slider.snapped = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                caption(
                    "The free slider stops between marks; the snapping one can only land on 0, \
                     0.25, 0.5, 0.75 or 1.",
                ),
            ],
        ),
        section(
            "Vertical",
            vec![
                div()
                    .flex()
                    .items_start()
                    .gap_8()
                    .child(upright(
                        150.,
                        native_slider("vertical", vertical)
                            .vertical(true)
                            .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                                d.slider.vertical = *value;
                                cx.notify();
                            }))
                            .into_any_element(),
                        format!("{vertical:.3}"),
                    ))
                    .child(upright(
                        150.,
                        native_slider("vertical-ticked", vertical_ticked)
                            .vertical(true)
                            .ticks(5)
                            .snap_to_ticks(true)
                            .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                                d.slider.vertical_ticked = *value;
                                cx.notify();
                            }))
                            .into_any_element(),
                        format!("{vertical_ticked:.3}"),
                    ))
                    .into_any_element(),
                caption("vertical(true), then the same with ticks(5) and snap_to_ticks(true)."),
            ],
        ),
        row(
            "disabled",
            vec![
                sized(
                    180.,
                    native_slider("disabled", continuous)
                        .enabled(false)
                        .into_any_element(),
                ),
                sized(
                    180.,
                    native_slider("disabled-ticked", snapped)
                        .ticks(5)
                        .snap_to_ticks(true)
                        .enabled(false)
                        .into_any_element(),
                ),
            ],
        ),
        demo(
            "clamping",
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(sized(
                    180.,
                    native_slider("clamped", 5.)
                        .range(0., 1.)
                        .into_any_element(),
                ))
                .child(caption(
                    "Value 5.0 on a 0..1 slider. Out-of-range values are clamped rather than \
                     widening the range.",
                ))
                .into_any_element(),
        ),
        props(&[
            ("native_slider(id, value)", format!("{continuous:.3}")),
            ("range(min, max)", "0..1, -20..20, 0..100".into()),
            ("ticks(n)", "11 free, 5 snapping".into()),
            (
                "snap_to_ticks(bool)",
                format!("{snapped:.3} — always a multiple of 0.25"),
            ),
            (
                "vertical(bool)",
                "pins width, takes length from layout".into(),
            ),
            ("enabled(bool)", "false on the disabled pair".into()),
            (
                "on_change(Fn(&f64, ..))",
                "fires continuously while dragging".into(),
            ),
        ]),
    ])
}
