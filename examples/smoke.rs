//! A real `NSSwitch` above GPUI's Metal content, driving the square GPUI paints beside it.

use gpui::{
    App, Application, Bounds, Context, Render, Window, WindowBounds, div, point, prelude::*, px,
    rgb, size,
};
use native_gpui::{native_root, native_switch, window_options};

struct Smoke {
    on: bool,
}

impl Render for Smoke {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        native_root(
            div()
                .size_full()
                .bg(rgb(0x1c1c1e))
                .flex()
                .items_center()
                .justify_center()
                .gap_4()
                .child(div().size(px(44.)).rounded_lg().bg(if self.on {
                    rgb(0x30d158)
                } else {
                    rgb(0xff453a)
                }))
                .child(native_switch("wifi", self.on).on_change(cx.listener(
                    |this, on: &bool, _, cx| {
                        this.on = *on;
                        cx.notify();
                    },
                ))),
        )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(200.), px(200.)),
                    size: size(px(420.), px(300.)),
                })),
                ..window_options()
            },
            |_, cx| cx.new(|_| Smoke { on: false }),
        )
        .unwrap();
        cx.activate(true);
    });
}
