use std::ops::Range;
use std::time::Instant;

use gpui::{
    App, Bounds, Context, Hsla, Render, UniformListScrollHandle, Window, WindowBounds, div, point,
    prelude::*, px, rgb, size, uniform_list,
};

use native_gpui::tokens::{SystemColor, system_color};
use native_gpui::{
    CheckState, Material, NativeHost, glass, material, native_checkbox, native_color_well,
    native_popup, native_progress, native_root, native_segmented, native_slider, native_spinner,
    native_switch, native_text_field, tokens, window_options,
};

struct Row {
    on: bool,
    level: f64,
}

struct Gallery {
    started: Instant,
    scroll: UniformListScrollHandle,
    rows: Vec<Row>,
    switch: bool,
    slider: f64,
    checked: bool,
    segment: usize,
    choice: usize,
    color: Hsla,
    text: String,
}

impl Gallery {
    fn controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = system_color(SystemColor::Label);
        let row = |name: &'static str| {
            div()
                .flex()
                .items_center()
                .gap_4()
                .h(px(40.))
                .child(div().w(px(96.)).child(name))
        };

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .w(px(420.))
            .text_color(label)
            .child(
                row("switch")
                    .child(native_switch("sw", self.switch).on_change(cx.listener(
                        |this, on: &bool, _, cx| {
                            this.switch = *on;
                            cx.notify();
                        },
                    )))
                    .child(
                        native_checkbox("cb", self.checked)
                            .title("checkbox")
                            .on_change(cx.listener(|this, state: &CheckState, _, cx| {
                                this.checked = *state == CheckState::On;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                row("slider")
                    .child(
                        div().w(px(200.)).child(
                            native_slider("sl", self.slider)
                                .range(0., 1.)
                                .on_change(cx.listener(|this, value: &f64, _, cx| {
                                    this.slider = *value;
                                    cx.notify();
                                })),
                        ),
                    )
                    .child(div().w(px(64.)).child(format!("{:.2}", self.slider))),
            )
            .child(
                row("segmented").child(
                    native_segmented("seg", self.segment)
                        .segments(["One", "Two", "Three"])
                        .on_change(cx.listener(|this, index: &usize, _, cx| {
                            this.segment = *index;
                            cx.notify();
                        })),
                ),
            )
            .child(
                row("popup")
                    .child(
                        native_popup("pop", self.choice)
                            .items(["Graphite", "Blue", "Purple"])
                            .on_change(cx.listener(|this, index: &usize, _, cx| {
                                this.choice = *index;
                                cx.notify();
                            })),
                    )
                    .child(native_color_well("well", self.color).on_change(cx.listener(
                        |this, color: &Hsla, _, cx| {
                            this.color = *color;
                            cx.notify();
                        },
                    ))),
            )
            .child(
                row("text").child(
                    div().w(px(220.)).child(
                        native_text_field("tf", &self.text)
                            .placeholder("type here")
                            .on_change(cx.listener(|this, text: &String, _, cx| {
                                this.text.clone_from(text);
                                cx.notify();
                            })),
                    ),
                ),
            )
            .child(
                row("progress")
                    .child(div().w(px(180.)).child(native_progress("pr", self.slider)))
                    .child(native_spinner("spin")),
            )
    }

    fn scroll_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = system_color(SystemColor::Label);
        let separator = system_color(SystemColor::Separator);

        uniform_list(
            "list",
            self.rows.len(),
            cx.processor(move |this, range: Range<usize>, _, cx| {
                range
                    .map(|index| {
                        let row = &this.rows[index];
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .h(px(44.))
                            .px_4()
                            .border_b_1()
                            .border_color(separator)
                            .text_color(label)
                            .child(div().w(px(80.)).child(format!("row {index:03}")))
                            .child(
                                native_switch(("row-sw", index), row.on).on_change(cx.listener(
                                    move |this, on: &bool, _, cx| {
                                        this.rows[index].on = *on;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                div().w(px(140.)).child(
                                    native_slider(("row-sl", index), row.level).range(0., 1.),
                                ),
                            )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .track_scroll(&self.scroll)
    }
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();

        let pooled = NativeHost::get(window, cx).len();
        let elapsed = self.started.elapsed().as_secs_f64();
        let header = format!(
            "pooled native views: {pooled}  ·  rows: {}  ·  t {elapsed:5.1}s",
            self.rows.len()
        );

        native_root(
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    material("bg", Material::WindowBackground)
                        .absolute()
                        .inset_0(),
                )
                .child(
                    div()
                        .h(px(44.))
                        .px_4()
                        .flex()
                        .items_center()
                        .text_color(system_color(SystemColor::SecondaryLabel))
                        .child(header),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .overflow_hidden()
                        .child(self.controls(cx))
                        .child(
                            div()
                                .w(px(24.))
                                .h_full()
                                .flex()
                                .child(glass("edge").corner_radius(px(12.))),
                        )
                        .child(self.scroll_list(cx)),
                ),
        )
    }
}

fn main() {
    let rows: usize = std::env::var("NG_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(400);
    gpui_platform::application().run(move |cx: &mut App| {
        tokens::init(cx);
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(180.), px(140.)),
                    size: size(px(1000.), px(660.)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|_| Gallery {
                    started: Instant::now(),
                    scroll: UniformListScrollHandle::new(),
                    rows: (0..rows)
                        .map(|index| Row {
                            on: index % 3 == 0,
                            level: (index % 10) as f64 / 10.,
                        })
                        .collect(),
                    switch: true,
                    slider: 0.35,
                    checked: false,
                    segment: 1,
                    choice: 0,
                    color: rgb(0x0a84ff).into(),
                    text: String::new(),
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
