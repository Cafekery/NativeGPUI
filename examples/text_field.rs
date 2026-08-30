//! The three hosted text controls — plain, secure and search — each mirrored into GPUI-painted
//! text as you type, with Return reporting a submit.

use gpui::{
    App, Application, Bounds, Context, Render, SharedString, Window, WindowBounds, div, point,
    prelude::*, px, rgb, size,
};
use native_gpui::{
    native_root, native_search_field, native_secure_field, native_text_field, window_options,
};

struct Demo {
    plain: String,
    secure: String,
    search: String,
    submitted: String,
}

fn row(label: &'static str, field: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(div().w(px(56.)).text_size(px(11.)).child(label))
        .child(div().w(px(240.)).child(field))
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        native_root(
            div()
                .size_full()
                .bg(rgb(0x1c1c1e))
                .text_color(rgb(0xf2f2f7))
                .flex()
                .flex_col()
                .p_5()
                .gap_4()
                .child(row(
                    "plain",
                    native_text_field("plain", &self.plain)
                        .placeholder("your name")
                        .on_change(cx.listener(|this, text: &String, _, cx| {
                            this.plain = text.clone();
                            cx.notify();
                        }))
                        .on_submit(cx.listener(|this, text: &String, _, cx| {
                            this.submitted = format!("plain={text}");
                            cx.notify();
                        })),
                ))
                .child(row(
                    "secure",
                    native_secure_field("secure", &self.secure)
                        .placeholder("password")
                        .on_change(cx.listener(|this, text: &String, _, cx| {
                            this.secure = text.clone();
                            cx.notify();
                        }))
                        .on_submit(cx.listener(|this, text: &String, _, cx| {
                            this.submitted = format!("secure={text}");
                            cx.notify();
                        })),
                ))
                .child(row(
                    "search",
                    native_search_field("search", &self.search)
                        .placeholder("search")
                        .on_change(cx.listener(|this, text: &String, _, cx| {
                            this.search = text.clone();
                            cx.notify();
                        }))
                        .on_submit(cx.listener(|this, text: &String, _, cx| {
                            this.submitted = format!("search={text}");
                            cx.notify();
                        })),
                ))
                .child(div().text_size(px(11.)).child(SharedString::from(format!(
                    "plain={:?} secure={:?} search={:?}",
                    self.plain, self.secure, self.search
                ))))
                .child(
                    div()
                        .text_size(px(11.))
                        .child(SharedString::from(format!("submitted[{}]", self.submitted))),
                ),
        )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(120.), px(1050.)),
                    size: size(px(560.), px(300.)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|_| Demo {
                    plain: String::new(),
                    secure: String::new(),
                    search: String::new(),
                    submitted: String::new(),
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
