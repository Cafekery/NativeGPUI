//! Component browser: native macOS sidebar, GPUI content, one page per module in `pages/`.
//! `NG_PAGE=<id>` opens straight onto a page.

mod pages;
mod ui;

use gpui::{
    AnyElement, App, Bounds, Context, Render, SharedString, Window, WindowBounds, WindowHandle,
    WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::sidebar::{NativeSidebar, SidebarOptions, sidebar_group, sidebar_item};
use native_gpui::tokens::{SystemColor, system_color};
use native_gpui::{install_menus, native_root, tokens, window_options};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct Page {
    pub id: &'static str,
    pub group: &'static str,
    pub title: &'static str,
    pub symbol: &'static str,
    pub class: &'static str,
    pub blurb: &'static str,
}

pub const PAGES: &[Page] = &[
    Page {
        id: "switch",
        group: "Controls",
        title: "Switch",
        symbol: "switch.2",
        class: "NSSwitch",
        blurb: "The macOS toggle, with the 26 glass knob and its press capsule.",
    },
    Page {
        id: "slider",
        group: "Controls",
        title: "Slider",
        symbol: "slider.horizontal.3",
        class: "NSSlider",
        blurb: "Continuous, tick-snapping and vertical. Tracks while dragging.",
    },
    Page {
        id: "segmented",
        group: "Controls",
        title: "Segmented",
        symbol: "square.split.2x1",
        class: "NSSegmentedControl",
        blurb: "Select-one or momentary. Restyled in macOS 26.",
    },
    Page {
        id: "buttons",
        group: "Controls",
        title: "Buttons",
        symbol: "capsule",
        class: "NSButton",
        blurb: "Push, default, bezel styles, checkbox with a mixed state, radios grouped in Rust.",
    },
    Page {
        id: "popup",
        group: "Controls",
        title: "Popup & Menu",
        symbol: "chevron.up.chevron.down",
        class: "NSPopUpButton / NSMenu",
        blurb: "Real menus, so they get the current glass panel and metrics.",
    },
    Page {
        id: "text",
        group: "Controls",
        title: "Text Fields",
        symbol: "character.cursor.ibeam",
        class: "NSTextField",
        blurb: "Real IME, spell check, Writing Tools, and the system focus ring.",
    },
    Page {
        id: "label",
        group: "Controls",
        title: "Label",
        symbol: "textformat",
        class: "NSTextField (label)",
        blurb: "Static text AppKit draws, so it can be ordered above a material that blurs GPUI.",
    },
    Page {
        id: "indicators",
        group: "Status",
        title: "Indicators",
        symbol: "progress.indicator",
        class: "NSProgressIndicator / NSLevelIndicator",
        blurb: "Determinate bars, the spinner, and level styles.",
    },
    Page {
        id: "small",
        group: "Status",
        title: "Steppers & Pickers",
        symbol: "calendar",
        class: "NSStepper / NSDatePicker / NSColorWell",
        blurb: "The small controls, each reading its value back through the host.",
    },
    Page {
        id: "material",
        group: "Surfaces",
        title: "Materials",
        symbol: "square.stack.3d.down.right",
        class: "NSVisualEffectView",
        blurb: "The full catalogue, over a checkerboard so a collapse is visible.",
    },
    Page {
        id: "glass",
        group: "Surfaces",
        title: "Liquid Glass",
        symbol: "drop",
        class: "NSGlassEffectView",
        blurb: "macOS 26 glass, and the container that batches many panels.",
    },
    Page {
        id: "glass_variants",
        group: "Surfaces",
        title: "Glass Variants",
        symbol: "drop.halffull",
        class: "NSGlassEffectView (private)",
        blurb: "The 13 undocumented render classes, their subvariants and interaction states.",
    },
    Page {
        id: "custom_glass",
        group: "Surfaces",
        title: "Custom Glass",
        symbol: "slider.horizontal.below.square.filled.and.square",
        class: "CABackdropLayer + CASDFLayer",
        blurb: "Glass built from its parts, with every filter input on a dial.",
    },
    Page {
        id: "surfaces",
        group: "Surfaces",
        title: "Popover & Sheet",
        symbol: "rectangle.portrait.topthird.inset.filled",
        class: "NSPopover / NSAlert / NSSharingServicePicker",
        blurb: "Transient surfaces, anchored to a GPUI element.",
    },
    Page {
        id: "tokens",
        group: "System",
        title: "System Tokens",
        symbol: "paintpalette",
        class: "NSColor / NSFont",
        blurb: "Semantic colours, the system font, and SF Symbols, so GPUI-drawn parts match.",
    },
    Page {
        id: "menus",
        group: "System",
        title: "Menu Bar",
        symbol: "menubar.rectangle",
        class: "NSMenu / NSMenuItem",
        blurb: "The standard bar, read back out of NSApp. Where ⌘W, ⌘Q, ⌘M and ⌘H live.",
    },
    Page {
        id: "background",
        group: "Window",
        title: "Backgrounds",
        symbol: "rectangle.inset.filled",
        class: "NSColor.windowBackgroundColor",
        blurb: "One background colour per window, and what a second one looks like.",
    },
    Page {
        id: "sidebar",
        group: "Window",
        title: "Sidebar",
        symbol: "sidebar.left",
        class: "NSSplitViewItem.sidebar",
        blurb: "Pops out its own window: a sidebar has to own the window's content view.",
    },
    Page {
        id: "chrome",
        group: "Window",
        title: "Window Chrome",
        symbol: "macwindow",
        class: "NSWindow / NSToolbar",
        blurb: "Pops out its own window: corner radius, toolbar, traffic lights, level.",
    },
];

pub fn page_of(id: &str) -> &'static Page {
    PAGES.iter().find(|p| p.id == id).unwrap_or(&PAGES[0])
}

/// Opens a second window with its own host, for the pages that configure the window itself.
pub fn popout<V: 'static + Render>(
    cx: &mut App,
    origin: (f32, f32),
    extent: (f32, f32),
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V,
) -> Option<WindowHandle<V>> {
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(origin.0), px(origin.1)),
                size: size(px(extent.0), px(extent.1)),
            })),
            ..window_options()
        },
        |window, cx| cx.new(|cx| build(window, cx)),
    )
    .ok()
}

pub struct Demo {
    sidebar: Option<NativeSidebar>,
    selected: SharedString,
    frames: VecDeque<Instant>,
    pub switch: pages::switch::State,
    pub slider: pages::slider::State,
    pub segmented: pages::segmented::State,
    pub buttons: pages::buttons::State,
    pub popup: pages::popup::State,
    pub text: pages::text::State,
    pub label: pages::label::State,
    pub indicators: pages::indicators::State,
    pub small: pages::small::State,
    pub material: pages::material::State,
    pub glass: pages::glass::State,
    pub glass_variants: pages::glass_variants::State,
    pub custom_glass: pages::custom_glass::State,
    pub surfaces: pages::surfaces::State,
    pub tokens: pages::tokens::State,
    pub menus: pages::menus::State,
    pub sidebar_page: pages::sidebar_page::State,
    pub chrome: pages::chrome::State,
    pub background: pages::background::State,
}

impl Demo {
    fn new() -> Self {
        Self {
            sidebar: None,
            frames: VecDeque::new(),
            selected: std::env::var("NG_PAGE")
                .ok()
                .map(|id| page_of(&id).id)
                .unwrap_or("switch")
                .into(),
            switch: Default::default(),
            slider: Default::default(),
            segmented: Default::default(),
            buttons: Default::default(),
            popup: Default::default(),
            text: Default::default(),
            label: Default::default(),
            indicators: Default::default(),
            small: Default::default(),
            material: Default::default(),
            glass: Default::default(),
            glass_variants: Default::default(),
            custom_glass: Default::default(),
            surfaces: Default::default(),
            tokens: Default::default(),
            menus: Default::default(),
            sidebar_page: Default::default(),
            chrome: Default::default(),
            background: Default::default(),
        }
    }

    /// Frames rendered in the last second and the longest gap between two of them.
    fn frame_stats(&mut self) -> String {
        let now = Instant::now();
        while self
            .frames
            .front()
            .is_some_and(|t| now - *t > Duration::from_secs(1))
        {
            self.frames.pop_front();
        }
        self.frames.push_back(now);
        let worst = self
            .frames
            .iter()
            .zip(self.frames.iter().skip(1))
            .map(|(a, b)| *b - *a)
            .max()
            .unwrap_or_default();
        format!(
            "{} fps · worst {:.1} ms",
            self.frames.len(),
            worst.as_secs_f64() * 1e3
        )
    }

    fn install_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = NativeSidebar::install(window, cx, SidebarOptions::default());

        let mut groups: Vec<&'static str> = Vec::new();
        for page in PAGES {
            if !groups.contains(&page.group) {
                groups.push(page.group);
            }
        }
        let items: Vec<_> = groups
            .iter()
            .map(|group| {
                sidebar_group(*group, *group).children(
                    PAGES
                        .iter()
                        .filter(|page| page.group == *group)
                        .map(|page| sidebar_item(page.id, page.title).symbol(page.symbol)),
                )
            })
            .collect();

        // Before `update`: a selection made while building the rows is dropped if nothing listens.
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
        sidebar.update(&items);
        sidebar.set_selected(Some(&self.selected));
        self.sidebar = Some(sidebar);
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.selected.as_ref() {
            "slider" => pages::slider::render(self, window, cx),
            "segmented" => pages::segmented::render(self, window, cx),
            "buttons" => pages::buttons::render(self, window, cx),
            "popup" => pages::popup::render(self, window, cx),
            "text" => pages::text::render(self, window, cx),
            "label" => pages::label::render(self, window, cx),
            "indicators" => pages::indicators::render(self, window, cx),
            "small" => pages::small::render(self, window, cx),
            "material" => pages::material::render(self, window, cx),
            "glass" => pages::glass::render(self, window, cx),
            "glass_variants" => pages::glass_variants::render(self, window, cx),
            "custom_glass" => pages::custom_glass::render(self, window, cx),
            "surfaces" => pages::surfaces::render(self, window, cx),
            "tokens" => pages::tokens::render(self, window, cx),
            "menus" => pages::menus::render(self, window, cx),
            "sidebar" => pages::sidebar_page::render(self, window, cx),
            "chrome" => pages::chrome::render(self, window, cx),
            "background" => pages::background::render(self, window, cx),
            _ => pages::switch::render(self, window, cx),
        }
    }
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.sidebar.is_none() {
            self.install_sidebar(window, cx);
        }
        // The handle carries the metrics synchronously; the global is set a few frames later.
        let metrics = self
            .sidebar
            .as_ref()
            .map(NativeSidebar::metrics)
            .unwrap_or_default();
        let page = page_of(&self.selected);
        let stats = self.frame_stats();
        let body = self.body(window, cx);

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
                            .id("content")
                            .size_full()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap_6()
                            .px(px(38.))
                            .py(px(24.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_3xl()
                                            .text_color(system_color(SystemColor::Label))
                                            .child(page.title),
                                    )
                                    .child(
                                        div()
                                            .text_color(system_color(SystemColor::SecondaryLabel))
                                            .child(page.blurb),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .flex()
                                            .gap_2()
                                            .text_xs()
                                            .child(
                                                div()
                                                    .px_2p5()
                                                    .py_1()
                                                    .rounded_md()
                                                    .bg(system_color(SystemColor::ControlAccent))
                                                    .text_color(system_color(
                                                        SystemColor::AlternateSelectedControlText,
                                                    ))
                                                    .child(SharedString::from(format!(
                                                        "drawn by macOS · {}",
                                                        page.class
                                                    ))),
                                            )
                                            .child(
                                                div()
                                                    .px_2p5()
                                                    .py_1()
                                                    .rounded_md()
                                                    .bg(system_color(
                                                        SystemColor::QuaternarySystemFill,
                                                    ))
                                                    .text_color(system_color(SystemColor::Label))
                                                    .child(SharedString::from(stats)),
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
    gpui_platform::application().run(|cx: &mut App| {
        tokens::init(cx);
        install_menus(cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(1180.), px(140.)),
                    size: size(px(1100.), px(760.)),
                })),
                ..window_options()
            },
            |_, cx| cx.new(|_| Demo::new()),
        )
        .expect("the storybook window opens");
        cx.activate(true);
    });
}
