use gpui::{
    App, Bounds, Context, Hsla, Render, Window, WindowBounds, div, hsla, point, prelude::*, px,
    rgb, size,
};
use native_gpui::{
    ColorWellStyle, DateElements, DatePickerStyle, native_color_well, native_date_picker,
    native_path_control, native_root, native_stepper, window_options,
};
use objc2_foundation::{NSDate, NSISO8601DateFormatter, NSTimeZone};

const START: f64 = 1_788_091_200.;

const DRAG_SRC: Hsla = Hsla {
    h: 0.09,
    s: 0.95,
    l: 0.55,
    a: 1.,
};

struct Small {
    count: f64,
    color: Hsla,
    stepper_date: f64,
    graphical_date: f64,
}

impl Small {
    fn took_color(&mut self, color: &Hsla, _: &mut Window, cx: &mut Context<Self>) {
        self.color = *color;
        cx.notify();
    }
}

fn iso8601(seconds: f64) -> String {
    let formatter = NSISO8601DateFormatter::new();
    formatter.setTimeZone(Some(&NSTimeZone::localTimeZone()));
    formatter
        .stringFromDate(&NSDate::dateWithTimeIntervalSince1970(seconds))
        .to_string()
}

impl Render for Small {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let label = |text: &'static str| {
            div()
                .text_size(px(11.))
                .text_color(rgb(0x98989d))
                .child(text)
        };

        native_root(
            div()
                .size_full()
                .bg(rgb(0x1c1c1e))
                .flex()
                .flex_row()
                .p_4()
                .gap_5()
                .text_color(rgb(0xf2f2f7))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .w(px(310.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(label("NSStepper"))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .child(
                                            div()
                                                .w(px(110.))
                                                .text_size(px(30.))
                                                .child(format!("{:.2}", self.count)),
                                        )
                                        .child(
                                            native_stepper("count", self.count)
                                                .range(0., 40.)
                                                .increment(0.25)
                                                .on_change(cx.listener(
                                                    |this, value: &f64, _, cx| {
                                                        this.count = *value;
                                                        cx.notify();
                                                    },
                                                )),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(label("NSColorWell — Swatch / Minimal / Expanded"))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .child(
                                            div().w(px(56.)).h(px(24.)).rounded_sm().bg(self.color),
                                        )
                                        .child(
                                            native_color_well("swatch", self.color)
                                                .style(ColorWellStyle::Swatch)
                                                .on_change(cx.listener(Self::took_color)),
                                        )
                                        .child(
                                            native_color_well("swatch-min", self.color)
                                                .style(ColorWellStyle::Minimal)
                                                .on_change(cx.listener(Self::took_color)),
                                        )
                                        .child(
                                            native_color_well("swatch-src", DRAG_SRC)
                                                .style(ColorWellStyle::Expanded),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(label("NSDatePicker — Stepper, DateTime"))
                                .child(
                                    native_date_picker("date-stepper", self.stepper_date)
                                        .style(DatePickerStyle::Stepper)
                                        .elements(DateElements::DateTime)
                                        .on_change(cx.listener(|this, seconds: &f64, _, cx| {
                                            this.stepper_date = *seconds;
                                            cx.notify();
                                        })),
                                )
                                .child(div().text_size(px(11.)).child(iso8601(self.stepper_date))),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(label("NSPathControl"))
                                .child(
                                    native_path_control("path", "/System/Library/Fonts").on_click(
                                        cx.listener(|_, path: &String, _, _| {
                                            println!("pathcontrol -> {path}");
                                        }),
                                    ),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(label("NSDatePicker — Graphical, DateTime"))
                        .child(
                            native_date_picker("date-graphical", self.graphical_date)
                                .style(DatePickerStyle::Graphical)
                                .elements(DateElements::DateTime)
                                .on_change(cx.listener(|this, seconds: &f64, _, cx| {
                                    this.graphical_date = *seconds;
                                    cx.notify();
                                })),
                        )
                        .child(div().text_size(px(11.)).child(iso8601(self.graphical_date))),
                ),
        )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(760.), px(1050.)),
                    size: size(px(680.), px(370.)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|_| Small {
                    count: 12.75,
                    color: hsla(0.58, 0.82, 0.55, 1.),
                    stepper_date: START,
                    graphical_date: START,
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
