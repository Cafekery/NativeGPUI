use gpui::{
    App, AppContext, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div,
    point, px, rgb, rgba, size,
};
use native_gpui::{Material, RawMaterial, material, native_root, window_options};

const WIN_X: f32 = 120.;
const WIN_Y: f32 = 160.;
const WIN_W: f32 = 900.;
const WIN_H: f32 = 680.;

const BACK_INSET: f32 = 20.;

const STRIPE_W: f32 = 16.;
const STRIPE_COLORS: [u32; 8] = [
    0xff0000, 0x00ffff, 0xffff00, 0x0000ff, 0xff00ff, 0x00ff00, 0xffffff, 0x000000,
];

const CARD_RADIUS: f32 = 14.;

const CYCLE: [(&str, Material); 9] = [
    ("WindowBackground", Material::WindowBackground),
    ("Sidebar", Material::Sidebar),
    ("Titlebar", Material::Titlebar),
    ("Header", Material::Header),
    ("Popover", Material::Popover),
    ("Menu", Material::Menu),
    ("Hud", Material::Hud),
    ("Selection", Material::Selection),
    ("Card", Material::Card),
];

struct Backdrop;

impl Render for Backdrop {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let count = ((WIN_W + 2. * BACK_INSET) / STRIPE_W).ceil() as usize + 1;
        div()
            .relative()
            .size_full()
            .bg(rgb(0x000000))
            .children((0..count).map(|i| {
                div()
                    .absolute()
                    .left(px(i as f32 * STRIPE_W))
                    .top_0()
                    .w(px(STRIPE_W))
                    .h_full()
                    .bg(rgb(STRIPE_COLORS[i % STRIPE_COLORS.len()]))
            }))
    }
}

struct Demo {
    rounded: bool,
    card: usize,
    emphasized: bool,
}

impl Demo {
    fn button(
        id: &'static str,
        label: impl Into<SharedString>,
        ink: gpui::Rgba,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .h(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(rgba(0x00000066))
            .text_color(ink)
            .text_size(px(11.))
            .cursor_pointer()
            .child(label.into())
    }

    fn swatch(
        id: (&'static str, usize),
        label: &'static str,
        entry: Option<RawMaterial>,
    ) -> gpui::Div {
        let mut cell = div().w(px(64.)).h(px(44.));
        if let Some(entry) = entry {
            cell = cell.child(material(id, entry).size_full());
        }
        let ink = if entry.is_some_and(RawMaterial::is_deprecated) {
            rgb(0x8e8e93)
        } else {
            rgb(0xf2f2f7)
        };
        div().flex().flex_col().gap_1().child(cell).child(
            div()
                .w(px(64.))
                .h(px(22.))
                .overflow_hidden()
                .text_center()
                .text_size(px(8.))
                .text_color(ink)
                .child(label),
        )
    }
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ink = rgb(0xf2f2f7);
        let (card_name, card_material) = CYCLE[self.card];
        let radius = if self.rounded { CARD_RADIUS } else { 0. };

        native_root(
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(
                    div()
                        .h(px(40.))
                        .child(material("titlebar", Material::Titlebar).size_full()),
                )
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .child(
                            material("sidebar", Material::Sidebar)
                                .w(px(180.))
                                .h_full()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .p_3()
                                .text_color(ink)
                                .text_size(px(13.))
                                .child("Library")
                                .child("All Items")
                                .child(
                                    Self::button(
                                        "btn-card",
                                        format!("card {card_name}"),
                                        rgb(0x0a84ff),
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.card = (this.card + 1) % CYCLE.len();
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    Self::button(
                                        "btn-radius",
                                        format!("radius {radius}"),
                                        rgb(0x30d158),
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.rounded = !this.rounded;
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    Self::button(
                                        "btn-emph",
                                        format!("emphasized {}", self.emphasized),
                                        rgb(0xbf5af2),
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.emphasized = !this.emphasized;
                                            cx.notify();
                                        },
                                    )),
                                ),
                        )
                        .child(
                            material("content", Material::WindowBackground)
                                .flex_1()
                                .h_full()
                                .flex()
                                .flex_col()
                                .gap_6()
                                .p_6()
                                .child(
                                    div()
                                        .text_color(ink)
                                        .text_size(px(13.))
                                        .child("GPUI text composites crisply over a material"),
                                )
                                .child(
                                    div().w(px(260.)).h(px(100.)).child(
                                        material("card", card_material)
                                            .size_full()
                                            .corner_radius(px(radius))
                                            .emphasized(self.emphasized),
                                    ),
                                )
                                .child(div().flex().flex_wrap().gap_1().children(
                                    RawMaterial::all().enumerate().map(|(i, entry)| {
                                        Self::swatch(("catalogue", i), entry.title(), Some(entry))
                                    }),
                                )),
                        ),
                )
                .child(
                    material("tray", Material::Header)
                        .h(px(96.))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_4()
                        .children(CYCLE.iter().enumerate().map(|(i, (name, kind))| {
                            Self::swatch(("semantic", i), name, Some((*kind).into()))
                        }))
                        .child(Self::swatch(("semantic", CYCLE.len()), "none", None)),
                ),
        )
    }
}

fn main() {
    let catalogue = RawMaterial::all()
        .map(RawMaterial::title)
        .collect::<Vec<_>>();
    println!("{} materials: {}", catalogue.len(), catalogue.join(", "));

    gpui_platform::application().run(|cx: &mut App| {
        // Backdrop first, so the demo window ends up in front of it.
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(WIN_X - BACK_INSET), px(WIN_Y - BACK_INSET)),
                    size: size(px(WIN_W + 2. * BACK_INSET), px(WIN_H + 2. * BACK_INSET)),
                })),
                titlebar: None,
                is_resizable: false,
                is_minimizable: false,
                ..Default::default()
            },
            |_, cx| cx.new(|_| Backdrop),
        )
        .unwrap();

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(WIN_X), px(WIN_Y)),
                    size: size(px(WIN_W), px(WIN_H)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|_| Demo {
                    rounded: true,
                    card: 4,
                    emphasized: false,
                })
            },
        )
        .unwrap();

        cx.activate(true);
    });
}
