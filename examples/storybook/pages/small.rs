use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Hsla, Window, div, hsla, prelude::*, px};
use native_gpui::{
    ColorWellStyle, DateElements, DatePickerStyle, PathStyle, native_color_well,
    native_date_picker, native_path_control, native_stepper,
};

pub struct State {
    pub count: f64,
    pub fine: f64,
    pub wrapped: f64,
    pub color: Hsla,
    pub date: f64,
    pub clicked: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            count: 7.,
            fine: 1.,
            wrapped: 0.,
            color: hsla(0.58, 0.85, 0.55, 1.),
            date: now_seconds(),
            clicked: None,
        }
    }
}

const COLOR_STYLES: [(&str, ColorWellStyle); 3] = [
    ("Swatch", ColorWellStyle::Swatch),
    ("Minimal", ColorWellStyle::Minimal),
    ("Expanded", ColorWellStyle::Expanded),
];

const DATE_STYLES: [(&str, DatePickerStyle); 3] = [
    ("Stepper", DatePickerStyle::Stepper),
    ("Text", DatePickerStyle::Text),
    ("Graphical", DatePickerStyle::Graphical),
];

const DATE_ELEMENTS: [(&str, DateElements); 6] = [
    ("Date", DateElements::Date),
    ("YearMonth", DateElements::YearMonth),
    ("Time", DateElements::Time),
    ("TimeSeconds", DateElements::TimeSeconds),
    ("DateTime", DateElements::DateTime),
    ("DateTimeSeconds", DateElements::DateTimeSeconds),
];

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let (count, fine, wrapped) = (d.small.count, d.small.fine, d.small.wrapped);
    let (color, date) = (d.small.color, d.small.date);
    let clicked = d.small.clicked.clone();
    let path = demo_path();

    stack(vec![
        section(
            "native_stepper",
            vec![
                stepper_row(
                    "range(0, 20), increment(1)",
                    format!("{count:.0}"),
                    native_stepper("count", count)
                        .range(0., 20.)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.small.count = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                stepper_row(
                    "increment(0.25)",
                    format!("{fine:.2}"),
                    native_stepper("fine", fine)
                        .range(0., 4.)
                        .increment(0.25)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.small.fine = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                stepper_row(
                    "wraps(true) over range(0, 3)",
                    format!("{wrapped:.0}"),
                    native_stepper("wrapped", wrapped)
                        .range(0., 3.)
                        .wraps(true)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.small.wrapped = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                stepper_row(
                    "autorepeat(false) — hold it, nothing repeats",
                    format!("{count:.0}"),
                    native_stepper("no-repeat", count)
                        .range(0., 20.)
                        .autorepeat(false)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.small.count = *value;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                stepper_row(
                    "enabled(false)",
                    format!("{count:.0}"),
                    native_stepper("stepper-off", count)
                        .enabled(false)
                        .into_any_element(),
                ),
            ],
        ),
        section(
            "native_color_well, every ColorWellStyle",
            vec![
                row(
                    "all three share one Hsla",
                    COLOR_STYLES
                        .iter()
                        .enumerate()
                        .map(|(i, (name, style))| {
                            tile(
                                *name,
                                86.,
                                native_color_well(("well", i), color)
                                    .style(*style)
                                    .on_change(cx.listener(|d: &mut Demo, color: &Hsla, _, cx| {
                                        d.small.color = *color;
                                        cx.notify();
                                    }))
                                    .into_any_element(),
                            )
                        })
                        .collect(),
                ),
                row(
                    "supports_alpha(false), and enabled(false)",
                    vec![
                        tile(
                            "no alpha",
                            86.,
                            native_color_well("well-opaque", color)
                                .supports_alpha(false)
                                .on_change(cx.listener(|d: &mut Demo, color: &Hsla, _, cx| {
                                    d.small.color = *color;
                                    cx.notify();
                                }))
                                .into_any_element(),
                        ),
                        tile(
                            "disabled",
                            86.,
                            native_color_well("well-off", color)
                                .enabled(false)
                                .into_any_element(),
                        ),
                    ],
                ),
                caption(format!(
                    "h {:.3}  s {:.3}  l {:.3}  a {:.3}",
                    color.h, color.s, color.l, color.a
                )),
            ],
        ),
        section(
            "native_date_picker — every style against every element set, all one f64",
            {
                let mut children = vec![caption(format!(
                    "{}  ·  {date:.0} s since the epoch",
                    utc_stamp(date)
                ))];
                children.extend(DATE_STYLES.iter().enumerate().map(
                    |(style_index, (style_name, style))| {
                        let tiles = DATE_ELEMENTS
                            .iter()
                            .enumerate()
                            .map(|(element_index, (element_name, elements))| {
                                tile(
                                    *element_name,
                                    200.,
                                    native_date_picker(
                                        ("date", style_index * DATE_ELEMENTS.len() + element_index),
                                        date,
                                    )
                                    .style(*style)
                                    .elements(*elements)
                                    .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                                        d.small.date = *value;
                                        cx.notify();
                                    }))
                                    .into_any_element(),
                                )
                            })
                            .collect();
                        demo(style_name, grid(tiles))
                    },
                ));
                children.push(demo(
                    "enabled(false)",
                    sized(
                        200.,
                        native_date_picker("date-off", date)
                            .enabled(false)
                            .into_any_element(),
                    ),
                ));
                children
            },
        ),
        section(
            "native_path_control, every PathStyle",
            vec![
                demo(
                    "Standard — the Finder-style breadcrumb",
                    sized(
                        420.,
                        native_path_control("path-standard", path.clone())
                            .style(PathStyle::Standard)
                            .on_click(cx.listener(|d: &mut Demo, path: &String, _, cx| {
                                d.small.clicked = Some(path.clone());
                                cx.notify();
                            }))
                            .into_any_element(),
                    ),
                ),
                demo(
                    "PopUp — the leaf, with its ancestors in the menu",
                    sized(
                        420.,
                        native_path_control("path-popup", path.clone())
                            .style(PathStyle::PopUp)
                            .on_click(cx.listener(|d: &mut Demo, path: &String, _, cx| {
                                d.small.clicked = Some(path.clone());
                                cx.notify();
                            }))
                            .into_any_element(),
                    ),
                ),
                demo(
                    "enabled(false)",
                    sized(
                        420.,
                        native_path_control("path-off", path.clone())
                            .enabled(false)
                            .into_any_element(),
                    ),
                ),
                match &clicked {
                    Some(path) => mono(format!("clicked {path}")),
                    None => caption("click a component — a click on no component is not reported"),
                },
            ],
        ),
        props(&[
            ("native_stepper(id, value)", format!("{count:.0}")),
            ("range(min, max)", "0 … 20".into()),
            ("increment(f64)", format!("0.25 · {fine:.2}")),
            ("wraps(bool)", format!("0 … 3 · {wrapped:.0}")),
            ("autorepeat(bool)", "default true".into()),
            (
                "native_color_well(id, Hsla)",
                format!("{:.3}, {:.3}, {:.3}", color.h, color.s, color.l),
            ),
            ("style(ColorWellStyle)", "all three, above".into()),
            ("supports_alpha(bool)", "default true".into()),
            ("native_date_picker(id, seconds)", utc_stamp(date)),
            ("style(DatePickerStyle)", "all three, above".into()),
            ("elements(DateElements)", "all six, per style".into()),
            ("native_path_control(id, path)", path),
            ("style(PathStyle)", "Standard and PopUp, above".into()),
            (
                "on_click(Fn(&String, ..))",
                clicked.unwrap_or_else(|| "nothing clicked yet".into()),
            ),
        ]),
    ])
}

fn stepper_row(label: &str, value: String, control: AnyElement) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(div().w(px(320.)).child(caption(label.to_string())))
        .child(control)
        .child(div().w(px(60.)).child(mono(value)))
        .into_any_element()
}

fn demo_path() -> String {
    std::env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".into())
}

fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or(0.)
}

/// Epoch seconds to `YYYY-MM-DD hh:mm:ssZ`, by Hinnant's `civil_from_days`.
fn utc_stamp(seconds: f64) -> String {
    let total = seconds.floor() as i64;
    let (days, time) = (total.div_euclid(86_400), total.rem_euclid(86_400));

    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        time / 3_600,
        (time % 3_600) / 60,
        time % 60,
    )
}
