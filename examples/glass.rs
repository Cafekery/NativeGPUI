//! macOS 26 liquid glass: both styles, tints, the corner-radius range, and a `glass_container`.

use gpui::{
    App, Bounds, Context, Div, Render, SharedString, Window, WindowBounds, div, hsla, point,
    prelude::*, px, rgb, size,
};
use native_gpui::{Glass, glass, glass_container, native_root, window_options};

const W: f32 = 940.;
const H: f32 = 520.;

const STRIPE: f32 = 8.;
const STRIPE_COLORS: [u32; 6] = [0xff375f, 0xffd60a, 0x30d158, 0x0a84ff, 0xbf5af2, 0xf2f2f7];

const RADII: [f32; 6] = [0., 8., 16., 26., 40., 44.];

struct Demo;

fn backdrop() -> Div {
    let count = (W / (2. * STRIPE)).ceil() as usize;
    div()
        .absolute()
        .left_0()
        .top_0()
        .size_full()
        .bg(rgb(0x0a0a0a))
        .children((0..count).map(|i| {
            div()
                .absolute()
                .left(px(i as f32 * 2. * STRIPE))
                .top_0()
                .w(px(STRIPE))
                .h_full()
                .bg(rgb(STRIPE_COLORS[i % STRIPE_COLORS.len()]))
        }))
}

fn caption(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.))
        .text_color(rgb(0xf2f2f7))
        .child(text.into())
}

fn cell(width: f32, height: f32, label: impl Into<SharedString>, panel: Glass) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().w(px(width)).h(px(height)).child(panel))
        .child(caption(label))
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let blue = hsla(0.58, 0.9, 0.55, 0.35);
        let orange = hsla(0.06, 0.95, 0.55, 0.35);

        native_root(
            div().relative().size_full().child(backdrop()).child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .p_8()
                    .child(
                        div()
                            .text_size(px(19.))
                            .text_color(rgb(0xffffff))
                            .child("Sharp GPUI text — frosted wherever a panel covers it"),
                    )
                    .child(div().flex().gap_5().children([
                        cell(150., 104., "regular", glass("regular")),
                        cell(150., 104., "clear", glass("clear").clear(true)),
                        cell(150., 104., "tint 0.35", glass("tint").tint(blue)),
                        cell(
                            150.,
                            104.,
                            "tint 1.0",
                            glass("tint-opaque").tint(hsla(0.58, 0.9, 0.55, 1.)),
                        ),
                        cell(
                            150.,
                            104.,
                            "clear + tint",
                            glass("tint-clear").clear(true).tint(orange),
                        ),
                    ]))
                    .child(div().flex().gap_5().children(RADII.map(|radius| {
                        cell(
                            120.,
                            88.,
                            format!("radius {radius}"),
                            glass(("radius", radius as u32)).corner_radius(px(radius)),
                        )
                    })))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div().h(px(88.)).child(
                                    glass_container("batch").child(
                                        div().flex().gap_5().size_full().children((0..4u32).map(
                                            |i| {
                                                div().w(px(170.)).h_full().child(
                                                    glass(("batch", i)).corner_radius(px(18.)),
                                                )
                                            },
                                        )),
                                    ),
                                ),
                            )
                            .child(caption("four panels batched into one render pass")),
                    ),
            ),
        )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(240.), px(160.)),
                    size: size(px(W), px(H)),
                })),
                ..window_options()
            },
            |_, cx| cx.new(|_| Demo),
        )
        .unwrap();
        cx.activate(true);
    });
}
