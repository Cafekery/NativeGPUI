//! Real `NSSlider`s driving GPUI state: horizontal, 5-tick snapping, and vertical.

use gpui::{
    App, Application, Bounds, Context, Render, Window, WindowBounds, div, point, prelude::*, px,
    rgb, size,
};
use native_gpui::{native_root, native_slider, window_options};

const TRACK: f32 = 300.;
const VTRACK: f32 = 180.;

struct Demo {
    volume: f64,
    balance: f64,
    steps: f64,
}

fn bar(fraction: f64, colour: u32) -> gpui::Div {
    div().w(px(TRACK)).h(px(16.)).bg(rgb(0x000000)).child(
        div()
            .w(px(TRACK * fraction as f32))
            .h(px(16.))
            .bg(rgb(colour)),
    )
}

fn label(text: &'static str) -> gpui::Div {
    div().text_color(rgb(0xf2f2f7)).child(text)
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        native_root(
            div()
                .size_full()
                .bg(rgb(0x101014))
                .p_5()
                .flex()
                .flex_row()
                .gap_6()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(label("volume"))
                        .child(div().w(px(TRACK)).child(
                            native_slider("volume", self.volume).on_change(cx.listener(
                                |this, value: &f64, _, cx| {
                                    this.volume = *value;
                                    cx.notify();
                                },
                            )),
                        ))
                        .child(bar(self.volume, 0x30d158))
                        .child(div().h(px(12.)))
                        .child(label("steps (5 ticks, snapping)"))
                        .child(
                            div().w(px(TRACK)).child(
                                native_slider("steps", self.steps)
                                    .ticks(5)
                                    .snap_to_ticks(true)
                                    .on_change(cx.listener(|this, value: &f64, _, cx| {
                                        this.steps = *value;
                                        cx.notify();
                                    })),
                            ),
                        )
                        .child(bar(self.steps, 0xbf5af2)),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_3()
                        .child(
                            div().h(px(VTRACK)).child(
                                native_slider("balance", self.balance)
                                    .vertical(true)
                                    .on_change(cx.listener(|this, value: &f64, _, cx| {
                                        this.balance = *value;
                                        cx.notify();
                                    })),
                            ),
                        )
                        .child(
                            div()
                                .w(px(16.))
                                .h(px(VTRACK))
                                .bg(rgb(0x000000))
                                .flex()
                                .flex_col()
                                .justify_end()
                                .child(
                                    div()
                                        .w(px(16.))
                                        .h(px(VTRACK * self.balance as f32))
                                        .bg(rgb(0xff375f)),
                                ),
                        ),
                ),
        )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(120.), px(120.)),
                    size: size(px(620.), px(400.)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|_| Demo {
                    volume: 0.25,
                    balance: 0.75,
                    steps: 0.5,
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
