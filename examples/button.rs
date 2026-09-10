use gpui::{
    App, Bounds, Context, Render, Window, WindowBounds, div, point, prelude::*, px, rgb, size,
};
use native_gpui::{
    CheckState, TrackingMode, native_button, native_checkbox, native_radio, native_root,
    native_segmented, window_options,
};

struct Buttons {
    check: CheckState,
    radio: usize,
    default_clicks: usize,
    plain_clicks: usize,
    segment: usize,
    presses: usize,
}

impl Render for Buttons {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let label = |text: &'static str| {
            div()
                .text_size(px(11.))
                .text_color(rgb(0x98989d))
                .child(text)
        };
        let readout = |text: String| div().text_size(px(11.)).child(text);
        let row = || div().flex().flex_col().gap_1();

        native_root(
            div()
                .size_full()
                .bg(rgb(0x1c1c1e))
                .flex()
                .flex_col()
                .p_5()
                .gap_5()
                .text_color(rgb(0xf2f2f7))
                .child(
                    row()
                        .child(label("NSButton — tri-state checkbox"))
                        .child(
                            native_checkbox("check", false)
                                .title("Show hidden files")
                                .allows_mixed(true)
                                .state(self.check)
                                .on_change(cx.listener(|this, state: &CheckState, _, cx| {
                                    this.check = *state;
                                    cx.notify();
                                })),
                        )
                        .child(readout(format!("{:?}", self.check))),
                )
                .child(
                    row()
                        .child(label("NSButton — radio group, grouped in Rust"))
                        .child(
                            native_radio("radio-0", self.radio == 0)
                                .title("Sort by name")
                                .on_change(cx.listener(|this, _: &bool, _, cx| {
                                    this.radio = 0;
                                    cx.notify();
                                })),
                        )
                        .child(
                            native_radio("radio-1", self.radio == 1)
                                .title("Sort by date")
                                .on_change(cx.listener(|this, _: &bool, _, cx| {
                                    this.radio = 1;
                                    cx.notify();
                                })),
                        )
                        .child(readout(format!("selected {}", self.radio))),
                )
                .child(
                    row()
                        .child(label("NSButton — push, one of them the window's default"))
                        .child(
                            div()
                                .flex()
                                .gap_3()
                                .child(
                                    native_button("default", "Continue")
                                        .default_button(true)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.default_clicks += 1;
                                            cx.notify();
                                        })),
                                )
                                .child(native_button("plain", "Cancel").on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.plain_clicks += 1;
                                        cx.notify();
                                    },
                                ))),
                        )
                        .child(readout(format!(
                            "Continue {} · Cancel {}",
                            self.default_clicks, self.plain_clicks
                        ))),
                )
                .child(
                    row()
                        .child(label("NSSegmentedControl — SelectOne"))
                        .child(
                            native_segmented("segmented", self.segment)
                                .segments(["Day", "Week", "Month"])
                                .tracking_mode(TrackingMode::SelectOne)
                                .on_change(cx.listener(|this, index: &usize, _, cx| {
                                    this.segment = *index;
                                    cx.notify();
                                })),
                        )
                        .child(readout(format!("segment {}", self.segment))),
                )
                .child(
                    row()
                        .child(label(
                            "NSSegmentedControl — Momentary, no persistent selection",
                        ))
                        .child(
                            native_segmented("momentary", 0)
                                .segments(["Undo", "Redo"])
                                .tracking_mode(TrackingMode::Momentary)
                                .on_change(cx.listener(|this, _: &usize, _, cx| {
                                    this.presses += 1;
                                    cx.notify();
                                })),
                        )
                        .child(readout(format!("{} presses", self.presses))),
                ),
        )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(760.), px(700.)),
                    size: size(px(420.), px(540.)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|_| Buttons {
                    check: CheckState::Off,
                    radio: 0,
                    default_clicks: 0,
                    plain_clicks: 0,
                    segment: 0,
                    presses: 0,
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
