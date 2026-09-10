use gpui::{
    App, Bounds, Context, Div, Render, SharedString, Window, WindowBounds, div, point, prelude::*,
    px, rgb, size,
};
use native_gpui::{GlassRecipe, custom_glass, glass, native_root, window_options};

const STRIPE: f32 = 8.;
const PITCH: f32 = 160.;
const PANEL_W: f32 = 144.;
const PANEL_H: f32 = 120.;
const MARGIN: f32 = 16.;
const BAND: f32 = 190.;
const TOP: f32 = 26.;
const RADIUS: f32 = 24.;
const COUNT: usize = 7;

const W: f32 = 2. * MARGIN + COUNT as f32 * PANEL_W + (COUNT - 1) as f32 * (PITCH - PANEL_W);
const H: f32 = 2. * BAND + 10.;

const WARM: [u32; 5] = [0xff375f, 0xffd60a, 0x30d158, 0x0a84ff, 0xbf5af2];
const COOL: [u32; 5] = [0x00c7be, 0xff9f0a, 0x5e5ce6, 0xff2d55, 0x32d74b];

struct Demo;

fn stripes(top: f32, colors: [u32; 5], vertical: bool) -> Div {
    let span = if vertical { W } else { BAND };
    let count = (span / (2. * STRIPE)).ceil() as usize;
    div()
        .absolute()
        .left_0()
        .top(px(top))
        .w(px(W))
        .h(px(BAND))
        .bg(rgb(0x0a0a0a))
        .children((0..count).map(|i| {
            let offset = px(i as f32 * 2. * STRIPE);
            let bar = div().absolute().bg(rgb(colors[i % colors.len()]));
            if vertical {
                bar.left(offset).top_0().w(px(STRIPE)).h_full()
            } else {
                bar.left_0().top(offset).w_full().h(px(STRIPE))
            }
        }))
}

fn slot(index: usize, band: f32, panel: impl IntoElement) -> Div {
    div()
        .absolute()
        .left(px(MARGIN + index as f32 * PITCH))
        .top(px(band + TOP))
        .w(px(PANEL_W))
        .h(px(PANEL_H))
        .child(panel)
}

fn caption(index: usize, text: impl Into<SharedString>) -> Div {
    div()
        .absolute()
        .left(px(MARGIN + index as f32 * PITCH))
        .top(px(TOP + PANEL_H + 8.))
        .text_size(px(11.))
        .text_color(rgb(0xf2f2f7))
        .child(text.into())
}

fn row(band: f32) -> impl IntoIterator<Item = Div> {
    let regular = GlassRecipe::regular();
    let key = |name: &'static str| (name, band as u32);
    let tuned = move |name: &'static str, recipe: GlassRecipe| {
        custom_glass(key(name)).recipe(GlassRecipe {
            corner_radius: RADIUS,
            ..recipe
        })
    };

    vec![
        slot(0, band, glass(key("stock")).corner_radius(px(RADIUS))),
        slot(1, band, tuned("regular", regular)),
        slot(2, band, tuned("clear", GlassRecipe::clear())),
        slot(
            3,
            band,
            tuned(
                "blur",
                GlassRecipe {
                    blur_radius: 40.,
                    ..regular
                },
            ),
        ),
        slot(
            4,
            band,
            tuned(
                "saturation",
                GlassRecipe {
                    face_saturation: 3.,
                    ..regular
                },
            ),
        ),
        slot(
            5,
            band,
            tuned(
                "refraction",
                GlassRecipe {
                    inner_refraction_amount: -300.,
                    inner_refraction_height: 60.,
                    ..regular
                },
            ),
        ),
        slot(
            6,
            band,
            custom_glass(key("blob")).recipe(GlassRecipe::blob()),
        ),
    ]
}

const LABELS: [&str; COUNT] = [
    "stock NSGlassEffectView",
    "regular",
    "clear",
    "blur 40",
    "saturation 3",
    "refraction -300",
    "blob",
];

impl Render for Demo {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        native_root(
            div()
                .relative()
                .size_full()
                .bg(rgb(0x0a0a0a))
                .child(stripes(0., WARM, true))
                .child(stripes(BAND, COOL, false))
                .children(row(0.))
                .children(row(BAND))
                .children(
                    LABELS
                        .iter()
                        .enumerate()
                        .map(|(index, label)| caption(index, *label)),
                ),
        )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(140.), px(700.)),
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
