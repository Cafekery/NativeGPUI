use gpui::{
    App, Bounds, Context, Render, Window, WindowBounds, div, point, prelude::*, px, rgb, size,
};
use native_gpui::{
    NativeTextField, install_menus, native_root, native_search_field, native_secure_field,
    native_text_field, window_options,
};

#[derive(Default)]
struct Demo {
    plain: String,
    secure: String,
    search: String,
    submitted: String,
}

fn row(
    label: &'static str,
    field: NativeTextField,
    text: fn(&mut Demo) -> &mut String,
    cx: &Context<Demo>,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(div().w(px(56.)).text_size(px(11.)).child(label))
        .child(
            div().w(px(240.)).child(
                field
                    .on_change(cx.listener(move |this, value: &String, _, cx| {
                        text(this).clone_from(value);
                        cx.notify();
                    }))
                    .on_submit(cx.listener(move |this, value: &String, _, cx| {
                        this.submitted = format!("{label}={value}");
                        cx.notify();
                    })),
            ),
        )
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
                    native_text_field("plain", &self.plain).placeholder("your name"),
                    |demo| &mut demo.plain,
                    cx,
                ))
                .child(row(
                    "secure",
                    native_secure_field("secure", &self.secure).placeholder("password"),
                    |demo| &mut demo.secure,
                    cx,
                ))
                .child(row(
                    "search",
                    native_search_field("search", &self.search).placeholder("search"),
                    |demo| &mut demo.search,
                    cx,
                ))
                .child(div().text_size(px(11.)).child(format!(
                    "plain={:?} secure={:?} search={:?}",
                    self.plain, self.secure, self.search
                )))
                .child(
                    div()
                        .text_size(px(11.))
                        .child(format!("submitted[{}]", self.submitted)),
                ),
        )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        install_menus(cx);
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(120.), px(1050.)),
                    size: size(px(560.), px(300.)),
                })),
                ..window_options()
            },
            |_, cx| cx.new(|_| Demo::default()),
        )
        .unwrap();
        cx.activate(true);
    });
}
