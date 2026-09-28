use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Window, div, prelude::*, px};
use native_gpui::{
    ControlSize, LevelStyle, native_level, native_progress, native_slider, native_spinner,
};

pub struct State {
    pub fraction: f64,
    pub rating: f64,
    pub load: f64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            fraction: 0.62,
            rating: 4.,
            load: 41.,
        }
    }
}

const SIZES: [(&str, ControlSize); 5] = [
    ("Mini", ControlSize::Mini),
    ("Small", ControlSize::Small),
    ("Regular", ControlSize::Regular),
    ("Large", ControlSize::Large),
    ("ExtraLarge", ControlSize::ExtraLarge),
];

const LEVEL_STYLES: [LevelStyle; 4] = [
    LevelStyle::Rating,
    LevelStyle::ContinuousCapacity,
    LevelStyle::DiscreteCapacity,
    LevelStyle::Relevancy,
];

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let State {
        fraction,
        rating,
        load,
    } = d.indicators;

    stack(vec![
        knob(
            "fraction",
            format!("{fraction:.2}"),
            native_slider("fraction", fraction)
                .on_change(set(cx, |d, value| d.indicators.fraction = value))
                .into_any_element(),
        ),
        demo(
            "determinate — driven by the slider above",
            sized(260., native_progress("live", fraction).into_any_element()),
        ),
        section(
            "fixed fractions",
            [0., 0.25, 0.5, 0.75, 1.]
                .into_iter()
                .enumerate()
                .map(|(i, step)| {
                    demo(
                        &format!("{step:.2}"),
                        sized(260., native_progress(("step", i), step).into_any_element()),
                    )
                })
                .collect(),
        ),
        demo(
            "indeterminate — the animated barber-pole, fraction ignored",
            sized(
                260.,
                native_progress("barber", fraction)
                    .indeterminate(true)
                    .into_any_element(),
            ),
        ),
        section(
            "bar, every ControlSize",
            SIZES
                .iter()
                .enumerate()
                .map(|(i, (name, size))| {
                    demo(
                        name,
                        sized(
                            260.,
                            native_progress(("bar-size", i), fraction)
                                .control_size(*size)
                                .into_any_element(),
                        ),
                    )
                })
                .collect(),
        ),
        demo(
            "width(px) — an explicit free axis instead of the full parent",
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(native_progress("w-120", fraction).width(px(120.)))
                .child(native_progress("w-200", fraction).width(px(200.)))
                .into_any_element(),
        ),
        row(
            "spinner, every ControlSize",
            SIZES
                .iter()
                .enumerate()
                .map(|(i, (name, size))| {
                    tile(
                        *name,
                        86.,
                        div()
                            .flex()
                            .h(px(40.))
                            .items_center()
                            .child(native_spinner(("spin", i)).control_size(*size))
                            .into_any_element(),
                    )
                })
                .collect(),
        ),
        knob(
            "level value",
            format!("{load:.0} / 100"),
            native_slider("load", load)
                .range(0., 100.)
                .on_change(set(cx, |d, value| d.indicators.load = value))
                .into_any_element(),
        ),
        section(
            "native_level, every LevelStyle — meters over range(0, 100), Rating over range(0, 10)",
            LEVEL_STYLES
                .iter()
                .enumerate()
                .map(|(i, &style)| {
                    let max = if style == LevelStyle::Rating {
                        10.
                    } else {
                        100.
                    };
                    demo(
                        &format!("{style:?} · 0 … {max:.0}"),
                        sized(
                            260.,
                            native_level(("level", i), load * max / 100.)
                                .style(style)
                                .range(0., max)
                                .warning_value(max * 0.6)
                                .critical_value(max * 0.85)
                                .into_any_element(),
                        ),
                    )
                })
                .collect(),
        ),
        demo(
            "width(px(120)) — ContinuousCapacity, pinned instead of stretched",
            native_level("level-narrow", load)
                .style(LevelStyle::ContinuousCapacity)
                .range(0., 100.)
                .warning_value(60.)
                .critical_value(85.)
                .width(px(120.))
                .into_any_element(),
        ),
        demo(
            "editable Rating — click the stars",
            div()
                .flex()
                .items_center()
                .gap_4()
                .child(
                    native_level("stars", rating)
                        .style(LevelStyle::Rating)
                        .range(0., 5.)
                        .editable(true)
                        .on_change(set(cx, |d, value| d.indicators.rating = value)),
                )
                .child(mono(format!("{rating:.0} / 5")))
                .into_any_element(),
        ),
        demo(
            "enabled(false)",
            native_level("stars-off", rating)
                .range(0., 5.)
                .editable(true)
                .enabled(false)
                .into_any_element(),
        ),
        props(&[
            ("native_progress(id, fraction)", format!("{fraction:.2}")),
            ("indeterminate(bool)", "drives startAnimation:".into()),
            ("control_size(ControlSize)", "all five, above".into()),
            ("width(impl Into<Length>)", "defaults to the parent".into()),
            ("native_spinner(id)", "always animating".into()),
            ("native_level(id, value)", format!("{load:.0} / 100")),
            ("style(LevelStyle)", "all four, above".into()),
            ("range(min, max)", "0 … 100, and 0 … 10 for Rating".into()),
            ("warning_value / critical_value", "60% / 85% of max".into()),
            ("editable(bool)", format!("Rating only · {rating:.0} / 5")),
        ]),
    ])
}
