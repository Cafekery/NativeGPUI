//! Component browser: native macOS sidebar, GPUI content.

use gpui::{
    AnyElement, App, Application, Bounds, Context, Hsla, Render, SharedString, Window,
    WindowBounds, div, point, prelude::*, px, rgb, size,
};
use native_gpui::sidebar::{
    NativeSidebar, SidebarMetrics, SidebarOptions, sidebar_group, sidebar_item,
};
use native_gpui::tokens::{SystemColor, system_color};
use native_gpui::{
    CheckState, Material, glass, install_menus, material, native_button, native_checkbox,
    native_color_well, native_date_picker, native_level, native_popup, native_progress,
    native_radio, native_root, native_search_field, native_secure_field, native_segmented,
    native_slider, native_spinner, native_stepper, native_switch, native_text_field, tokens,
    window_options,
};

struct Page {
    id: &'static str,
    title: &'static str,
    symbol: &'static str,
    class: &'static str,
    blurb: &'static str,
}

const PAGES: &[Page] = &[
    Page {
        id: "switch",
        title: "Switch",
        symbol: "switch.2",
        class: "NSSwitch",
        blurb: "The macOS toggle, with the 26 glass knob and its press capsule.",
    },
    Page {
        id: "slider",
        title: "Slider",
        symbol: "slider.horizontal.3",
        class: "NSSlider",
        blurb: "Continuous, tick-snapping and vertical. Tracks continuously while dragging.",
    },
    Page {
        id: "segmented",
        title: "Segmented",
        symbol: "square.split.2x1",
        class: "NSSegmentedControl",
        blurb: "Select-one or momentary. Restyled in macOS 26.",
    },
    Page {
        id: "buttons",
        title: "Buttons",
        symbol: "capsule",
        class: "NSButton",
        blurb: "Push, default, checkbox with a mixed state, and radios grouped in Rust.",
    },
    Page {
        id: "popup",
        title: "Popup & Menu",
        symbol: "chevron.up.chevron.down",
        class: "NSPopUpButton / NSMenu",
        blurb: "Real menus, so they get the current glass panel and metrics.",
    },
    Page {
        id: "text",
        title: "Text Fields",
        symbol: "character.cursor.ibeam",
        class: "NSTextField",
        blurb: "Real IME, spell check, emoji picker, services and the system focus ring.",
    },
    Page {
        id: "indicators",
        title: "Indicators",
        symbol: "progress.indicator",
        class: "NSProgressIndicator / NSLevelIndicator",
        blurb: "Determinate and indeterminate bars, spinners, and an editable rating.",
    },
    Page {
        id: "small",
        title: "Steppers & Pickers",
        symbol: "calendar",
        class: "NSStepper / NSColorWell / NSDatePicker",
        blurb: "The graphical date picker is the clearest proof macOS is drawing.",
    },
    Page {
        id: "material",
        title: "Materials",
        symbol: "square.stack.3d.up",
        class: "NSVisualEffectView",
        blurb: "Vibrancy beneath GPUI's Metal layer, so GPUI ink composites on top.",
    },
    Page {
        id: "glass",
        title: "Liquid Glass",
        symbol: "drop",
        class: "NSGlassEffectView",
        blurb: "Refracts GPUI's own rendered content, not just the desktop.",
    },
    Page {
        id: "tokens",
        title: "System Tokens",
        symbol: "paintpalette",
        class: "NSColor / NSFont",
        blurb: "Semantic colours and SF Symbols, so GPUI-drawn surfaces match the controls.",
    },
];

struct Demo {
    sidebar: Option<NativeSidebar>,
    selected: SharedString,
    switch: bool,
    slider: f64,
    ticked: f64,
    vertical: f64,
    segment: usize,
    check: CheckState,
    radio: usize,
    clicks: usize,
    choice: usize,
    text: String,
    secure: String,
    search: String,
    progress: f64,
    rating: f64,
    stepper: f64,
    color: Hsla,
    date: f64,
}

impl Demo {
    fn install_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = NativeSidebar::install(window, cx, SidebarOptions::default());

        let group = |id: &'static str, title: &'static str, ids: &[&str]| {
            sidebar_group(id, title).children(
                PAGES
                    .iter()
                    .filter(|page| ids.contains(&page.id))
                    .map(|page| sidebar_item(page.id, page.title).symbol(page.symbol)),
            )
        };

        sidebar.update(&[
            group(
                "g-controls",
                "Controls",
                &["switch", "slider", "segmented", "buttons", "popup", "text"],
            ),
            group("g-status", "Status", &["indicators", "small"]),
            group("g-surfaces", "Surfaces", &["material", "glass", "tokens"]),
        ]);
        sidebar.set_selected(Some(&self.selected));

        let entity = cx.entity().downgrade();
        sidebar.on_select(move |id, _, cx| {
            let Some(id) = id.clone() else { return };
            entity
                .update(cx, |this: &mut Demo, cx| {
                    this.selected = id;
                    cx.notify();
                })
                .ok();
        });

        self.sidebar = Some(sidebar);
    }

    fn page(&self) -> &'static Page {
        PAGES
            .iter()
            .find(|page| page.id == self.selected.as_ref())
            .unwrap_or(&PAGES[0])
    }

    fn body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.selected.as_ref() {
            "slider" => self.slider_page(cx),
            "segmented" => self.segmented_page(cx),
            "buttons" => self.buttons_page(cx),
            "popup" => self.popup_page(cx),
            "text" => self.text_page(cx),
            "indicators" => self.indicators_page(cx),
            "small" => self.small_page(cx),
            "material" => Self::material_page(),
            "glass" => Self::glass_page(),
            "tokens" => Self::tokens_page(),
            _ => self.switch_page(cx),
        }
    }

    fn switch_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.switch;
        stack(vec![
            demo(
                "state",
                native_switch("sw", on)
                    .on_change(cx.listener(|this, on: &bool, _, cx| {
                        this.switch = *on;
                        cx.notify();
                    }))
                    .into_any_element(),
            ),
            demo(
                "disabled",
                native_switch("sw-off", true)
                    .enabled(false)
                    .into_any_element(),
            ),
            props(&[
                ("native_switch(id, on)", on.to_string()),
                ("on_change(Fn(&bool, ..))", "wired".into()),
                ("enabled(bool)", "true / false above".into()),
            ]),
        ])
    }

    fn slider_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (a, b, c) = (self.slider, self.ticked, self.vertical);
        stack(vec![
            demo(
                "continuous",
                sized(
                    260.,
                    native_slider("sl", a)
                        .range(0., 1.)
                        .on_change(cx.listener(|this, v: &f64, _, cx| {
                            this.slider = *v;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
            ),
            demo(
                "5 ticks, snapping",
                sized(
                    260.,
                    native_slider("sl-t", b)
                        .range(0., 1.)
                        .ticks(5)
                        .snap_to_ticks(true)
                        .on_change(cx.listener(|this, v: &f64, _, cx| {
                            this.ticked = *v;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
            ),
            demo(
                "vertical",
                div()
                    .h(px(140.))
                    .flex()
                    .child(
                        native_slider("sl-v", c)
                            .range(0., 1.)
                            .vertical(true)
                            .on_change(cx.listener(|this, v: &f64, _, cx| {
                                this.vertical = *v;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            ),
            props(&[
                ("range(min, max)", "0.0 .. 1.0".into()),
                ("value", format!("{a:.3} / {b:.3} / {c:.3}")),
                ("ticks(n) + snap_to_ticks(bool)", "5, snapping".into()),
                ("vertical(bool)", "axes swap from fittingSize".into()),
            ]),
        ])
    }

    fn segmented_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.segment;
        stack(vec![
            demo(
                "select one",
                native_segmented("seg", selected)
                    .segments(["Design", "Build", "Ship"])
                    .on_change(cx.listener(|this, index: &usize, _, cx| {
                        this.segment = *index;
                        cx.notify();
                    }))
                    .into_any_element(),
            ),
            props(&[
                ("segments(iter)", "Design, Build, Ship".into()),
                ("selected", selected.to_string()),
                (
                    "tracking_mode(..)",
                    "SelectOne | SelectAny | Momentary".into(),
                ),
            ]),
        ])
    }

    fn buttons_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (check, radio, clicks) = (self.check, self.radio, self.clicks);
        let radio_row = div()
            .flex()
            .gap_4()
            .children((0..3).map(|index| {
                native_radio(("radio", index), radio == index)
                    .title(["Small", "Medium", "Large"][index])
                    .on_change(cx.listener(move |this, on: &bool, _, cx| {
                        if *on {
                            this.radio = index;
                            cx.notify();
                        }
                    }))
            }))
            .into_any_element();

        stack(vec![
            demo(
                "push",
                div()
                    .flex()
                    .gap_3()
                    .child(
                        native_button("btn", "Continue")
                            .default_button(true)
                            .on_click(cx.listener(|this, _: &(), _, cx| {
                                this.clicks += 1;
                                cx.notify();
                            })),
                    )
                    .child(native_button("btn2", "Cancel"))
                    .into_any_element(),
            ),
            demo(
                "checkbox",
                native_checkbox("cb", check == CheckState::On)
                    .title("Include drafts")
                    .allows_mixed(true)
                    .state(check)
                    .on_change(cx.listener(|this, state: &CheckState, _, cx| {
                        this.check = *state;
                        cx.notify();
                    }))
                    .into_any_element(),
            ),
            demo("radio group", radio_row),
            props(&[
                ("clicks", clicks.to_string()),
                ("checkbox state", format!("{check:?}")),
                ("radio selected", radio.to_string()),
                (
                    "default_button(bool)",
                    "asks NSWindow.defaultButtonCell".into(),
                ),
            ]),
        ])
    }

    fn popup_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let choice = self.choice;
        stack(vec![
            demo(
                "pop up",
                native_popup("pop", choice)
                    .items(["Graphite", "Blue", "Purple", "Green"])
                    .on_change(cx.listener(|this, index: &usize, _, cx| {
                        this.choice = *index;
                        cx.notify();
                    }))
                    .into_any_element(),
            ),
            demo(
                "pull down",
                native_popup("pull", 0)
                    .items(["Actions", "Duplicate", "Export", "Delete"])
                    .pull_down(true)
                    .into_any_element(),
            ),
            props(&[
                ("selected", choice.to_string()),
                (
                    "items(iter) / entries(iter)",
                    "separators, disabled rows".into(),
                ),
                ("index source", "indexOfSelectedItem via on_event".into()),
            ]),
        ])
    }

    fn text_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (text, secure, search) = (self.text.clone(), self.secure.clone(), self.search.clone());
        stack(vec![
            demo(
                "text",
                sized(
                    280.,
                    native_text_field("tf", &text)
                        .placeholder("Project name")
                        .on_change(cx.listener(|this, value: &String, _, cx| {
                            this.text = value.clone();
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
            ),
            demo(
                "secure",
                sized(
                    280.,
                    native_secure_field("sf", &secure)
                        .placeholder("Password")
                        .on_change(cx.listener(|this, value: &String, _, cx| {
                            this.secure = value.clone();
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
            ),
            demo(
                "search",
                sized(
                    280.,
                    native_search_field("qf", &search)
                        .placeholder("Filter")
                        .on_change(cx.listener(|this, value: &String, _, cx| {
                            this.search = value.clone();
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
            ),
            props(&[
                ("mirrored into GPUI", format!("{text:?}")),
                ("on_change / on_submit", "keystroke / Return".into()),
                (
                    "free",
                    "IME, spell check, emoji, services, focus ring".into(),
                ),
                ("limitation", "Cmd-A/C/V do not reach a hosted field".into()),
            ]),
        ])
    }

    fn indicators_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (progress, rating) = (self.progress, self.rating);
        stack(vec![
            demo(
                "determinate",
                sized(260., native_progress("pr", progress).into_any_element()),
            ),
            demo(
                "indeterminate",
                sized(
                    260.,
                    native_progress("pri", 0.)
                        .indeterminate(true)
                        .into_any_element(),
                ),
            ),
            demo(
                "spinner",
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(native_spinner("spin"))
                    .child(native_spinner("spin-s").control_size(native_gpui::ControlSize::Small))
                    .into_any_element(),
            ),
            demo(
                "rating",
                native_level("lv", rating)
                    .range(0., 5.)
                    .editable(true)
                    .on_change(cx.listener(|this, value: &f64, _, cx| {
                        this.rating = *value;
                        cx.notify();
                    }))
                    .into_any_element(),
            ),
            props(&[
                ("fraction", format!("{progress:.2}")),
                ("rating", format!("{rating:.0} / 5")),
                ("indeterminate(bool)", "drives startAnimation:".into()),
            ]),
        ])
    }

    fn small_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (stepper, color, date) = (self.stepper, self.color, self.date);
        stack(vec![
            demo(
                "stepper",
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        native_stepper("st", stepper)
                            .range(0., 20.)
                            .on_change(cx.listener(|this, value: &f64, _, cx| {
                                this.stepper = *value;
                                cx.notify();
                            })),
                    )
                    .child(SharedString::from(format!("{stepper:.0}")))
                    .into_any_element(),
            ),
            demo(
                "colour well",
                native_color_well("cw", color)
                    .on_change(cx.listener(|this, color: &Hsla, _, cx| {
                        this.color = *color;
                        cx.notify();
                    }))
                    .into_any_element(),
            ),
            demo(
                "date, graphical",
                native_date_picker("dp", date)
                    .style(native_gpui::DatePickerStyle::Graphical)
                    .into_any_element(),
            ),
            props(&[
                ("stepper value", format!("{stepper:.0}")),
                ("colour", format!("{:.3}, {:.3}", color.h, color.s)),
                ("date", "seconds since epoch, UTC".into()),
            ]),
        ])
    }

    fn material_page() -> AnyElement {
        let swatch = |id: &'static str, label: &'static str, kind: Material| {
            div()
                .relative()
                .w(px(150.))
                .h(px(96.))
                .child(material(id, kind).absolute().inset_0())
                .child(
                    div()
                        .absolute()
                        .bottom_2()
                        .left_2()
                        .text_color(system_color(SystemColor::Label))
                        .child(label),
                )
        };

        stack(vec![
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .child(swatch("m1", "Sidebar", Material::Sidebar))
                .child(swatch("m2", "Titlebar", Material::Titlebar))
                .child(swatch("m3", "Popover", Material::Popover))
                .child(swatch("m4", "HUD", Material::Hud))
                .into_any_element(),
            props(&[
                ("material(id, Material)", "9 semantic variants".into()),
                ("layer", "Below, so GPUI paints on top".into()),
                ("blending(..)", "BehindWindow | WithinWindow".into()),
                ("state", "Active — Inactive flattens every material".into()),
            ]),
        ])
    }

    fn glass_page() -> AnyElement {
        let stripes = div()
            .absolute()
            .inset_0()
            .flex()
            .children((0..48).map(|index| {
                div().w(px(10.)).h_full().bg(if index % 2 == 0 {
                    rgb(0xf2f2f2)
                } else {
                    rgb(0x0a0a0a)
                })
            }));

        stack(vec![
            div()
                .relative()
                .w(px(480.))
                .h(px(180.))
                .overflow_hidden()
                .child(stripes)
                .child(
                    div()
                        .absolute()
                        .top(px(30.))
                        .left(px(40.))
                        .w(px(160.))
                        .h(px(120.))
                        .flex()
                        .child(glass("g1").corner_radius(px(28.))),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(30.))
                        .left(px(260.))
                        .w(px(160.))
                        .h(px(120.))
                        .flex()
                        .child(glass("g2").corner_radius(px(28.)).tint(rgb(0x0a84ff))),
                )
                .into_any_element(),
            props(&[
                ("glass(id)", "frosts the GPUI content beneath".into()),
                (
                    "frosting_desktop()",
                    "Below layer, needs a transparent hole".into(),
                ),
                ("corner_radius / tint / clear", "28pt, accent tint".into()),
                (
                    "glass_container(id)",
                    "batches passes — 400 panels 18 to 120 fps".into(),
                ),
            ]),
        ])
    }

    fn tokens_page() -> AnyElement {
        let colors = [
            ("Label", SystemColor::Label),
            ("SecondaryLabel", SystemColor::SecondaryLabel),
            ("Separator", SystemColor::Separator),
            ("ControlAccent", SystemColor::ControlAccent),
            (
                "SelectedContentBackground",
                SystemColor::SelectedContentBackground,
            ),
            ("Link", SystemColor::Link),
        ];

        stack(vec![
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .children(colors.map(|(name, token)| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .w(px(120.))
                                .h(px(48.))
                                .rounded_md()
                                .bg(system_color(token)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(system_color(SystemColor::SecondaryLabel))
                                .child(name),
                        )
                }))
                .into_any_element(),
            props(&[
                ("system_color(SystemColor)", "49 semantic colours".into()),
                ("accent()", "controlAccentColor".into()),
                (
                    "SystemTheme global",
                    "re-resolves on appearance change".into(),
                ),
                (
                    "sf_symbol(name, size, weight)",
                    "rasterised for GPUI".into(),
                ),
            ]),
        ])
    }
}

fn stack(children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_6()
        .children(children)
        .into_any_element()
}

fn sized(width: f32, child: AnyElement) -> AnyElement {
    div().w(px(width)).child(child).into_any_element()
}

fn demo(label: &'static str, child: AnyElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_xs()
                .text_color(system_color(SystemColor::TertiaryLabel))
                .child(label.to_uppercase()),
        )
        .child(child)
        .into_any_element()
}

fn props(rows: &[(&str, String)]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .text_xs()
                .pb_2()
                .text_color(system_color(SystemColor::TertiaryLabel))
                .child("PROPERTIES"),
        )
        .children(rows.iter().map(|(name, value)| {
            div()
                .flex()
                .gap_4()
                .py_1p5()
                .border_b_1()
                .border_color(system_color(SystemColor::Separator))
                .child(
                    div()
                        .w(px(240.))
                        .font_family("SF Mono")
                        .text_sm()
                        .text_color(system_color(SystemColor::Label))
                        .child(SharedString::from(name.to_string())),
                )
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(system_color(SystemColor::SecondaryLabel))
                        .child(SharedString::from(value.clone())),
                )
        }))
        .into_any_element()
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.sidebar.is_none() {
            self.install_sidebar(window, cx);
        }

        let metrics = cx
            .try_global::<SidebarMetrics>()
            .copied()
            .unwrap_or_default();
        let page = self.page();
        let body = self.body(cx);

        native_root(
            div().relative().size_full().child(
                div()
                    .absolute()
                    .left(metrics.left)
                    .top(metrics.top)
                    .right_0()
                    .bottom_0()
                    .child(
                        div()
                            .size_full()
                            .flex()
                            .flex_col()
                            .gap_5()
                            .px(px(32.))
                            .py(px(24.))
                            .id("content")
                            .overflow_y_scroll()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_3xl()
                                            .text_color(system_color(SystemColor::Label))
                                            .child(page.title),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(system_color(SystemColor::SecondaryLabel))
                                            .child(page.blurb),
                                    )
                                    .child(
                                        div().pt_1().child(
                                            div()
                                                .px_2()
                                                .py_0p5()
                                                .rounded_md()
                                                .text_xs()
                                                .bg(system_color(
                                                    SystemColor::SelectedContentBackground,
                                                ))
                                                .text_color(gpui::white())
                                                .child(SharedString::from(format!(
                                                    "drawn by macOS · {}",
                                                    page.class
                                                ))),
                                        ),
                                    ),
                            )
                            .child(body),
                    ),
            ),
        )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        tokens::init(cx);
        install_menus(cx);
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(1180.), px(140.)),
                    size: size(px(1060.), px(720.)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|_| Demo {
                    sidebar: None,
                    selected: "switch".into(),
                    switch: true,
                    slider: 0.4,
                    ticked: 0.5,
                    vertical: 0.6,
                    segment: 1,
                    check: CheckState::Mixed,
                    radio: 1,
                    clicks: 0,
                    choice: 1,
                    text: String::new(),
                    secure: String::new(),
                    search: String::new(),
                    progress: 0.62,
                    rating: 3.,
                    stepper: 8.,
                    color: rgb(0x0a84ff).into(),
                    date: 1_780_000_000.,
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
