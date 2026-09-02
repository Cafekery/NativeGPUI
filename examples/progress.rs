//! Real `NSProgressIndicator`s (bar and spinner) and `NSLevelIndicator`s inside GPUI.

use std::time::Duration;

use gpui::{
    App, Application, Bounds, Context, Render, Window, WindowBounds, div, point, prelude::*, px,
    rgb, size,
};
use native_gpui::{
    ControlSize, LevelStyle, native_level, native_progress, native_root, native_spinner,
    window_options,
};

struct Demo {
    fraction: f64,
    rating: f64,
}

fn row() -> gpui::Div {
    div().flex().items_center().gap_4()
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stars = self.rating.round() as usize;

        native_root(
            div()
                .size_full()
                .bg(rgb(0x1c1c1e))
                .flex()
                .flex_col()
                .p_6()
                .gap_5()
                .text_color(rgb(0xf2f2f7))
                .child(
                    row()
                        .child(native_progress("determinate", self.fraction).width(px(300.)))
                        .child(div().child(format!("{:.0}%", self.fraction * 100.))),
                )
                .child(
                    row()
                        .child(
                            native_progress("indeterminate", 0.)
                                .indeterminate(true)
                                .width(px(300.)),
                        )
                        .child(div().child("indeterminate")),
                )
                .child(
                    row()
                        .child(native_spinner("spin-regular"))
                        .child(native_spinner("spin-small").control_size(ControlSize::Small))
                        .child(div().child("spinners")),
                )
                .child(
                    row()
                        .child(
                            native_level("capacity", 78.)
                                .style(LevelStyle::ContinuousCapacity)
                                .range(0., 100.)
                                .warning_value(60.)
                                .critical_value(90.)
                                .width(px(200.)),
                        )
                        .child(
                            native_level("relevancy", 0.7)
                                .style(LevelStyle::Relevancy)
                                .range(0., 1.)
                                .width(px(80.)),
                        )
                        .child(div().child("capacity / relevancy")),
                )
                .child(
                    row()
                        .child(
                            native_level("rating", self.rating)
                                .style(LevelStyle::Rating)
                                .range(0., 5.)
                                .editable(true)
                                .on_change(cx.listener(|this, value: &f64, _, cx| {
                                    this.rating = *value;
                                    cx.notify();
                                })),
                        )
                        .child(div().text_size(px(28.)).child(format!("{stars}"))),
                )
                .child(div().flex().gap_2().children((0..5).map(|i| {
                    div().size(px(22.)).rounded_md().bg(if i < stars {
                        rgb(0xffd60a)
                    } else {
                        rgb(0x3a3a3c)
                    })
                }))),
        )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(1400.), px(500.)),
                    size: size(px(520.), px(440.)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|cx| {
                    cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor()
                                .timer(Duration::from_millis(33))
                                .await;
                            let advanced = this
                                .update(cx, |demo: &mut Demo, cx| {
                                    demo.fraction = (demo.fraction + 1. / 120.).fract();
                                    cx.notify();
                                })
                                .is_ok();
                            if !advanced {
                                break;
                            }
                        }
                    })
                    .detach();
                    Demo {
                        fraction: 0.,
                        rating: 2.,
                    }
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
