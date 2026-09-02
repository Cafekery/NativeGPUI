//! Real `NSPopUpButton`s and `NSMenu`s driving GPUI state: pop-up, pull-down and context menus.

use gpui::{
    App, Application, Bounds, Context, Hsla, Render, Window, WindowBounds, div, point, prelude::*,
    px, rgb, size,
};
use native_gpui::{
    MenuEntry, MenuOrigin, native_context_menu, native_popup, native_root, window_options,
};

fn accent_entries() -> Vec<MenuEntry> {
    vec![
        MenuEntry::item("Automatic").symbol("paintpalette"),
        MenuEntry::item("Graphite").symbol("circle.fill"),
        MenuEntry::item("Vivid Blue").symbol("drop.fill"),
        MenuEntry::separator(),
        MenuEntry::item("Custom…")
            .symbol("slider.horizontal.3")
            .disabled(),
    ]
}

fn accent_color(index: usize) -> Hsla {
    match index {
        0 => rgb(0xffd60a).into(),
        1 => rgb(0x8e8e93).into(),
        2 => rgb(0x0a84ff).into(),
        _ => rgb(0xff453a).into(),
    }
}

// AppKit lifts entry 0 onto a pull-down's button face and hides it from the list.
fn document_entries() -> Vec<MenuEntry> {
    vec![
        MenuEntry::item("Document"),
        MenuEntry::item("New Sketch").symbol("doc.badge.plus"),
        MenuEntry::item("Duplicate").symbol("plus.square.on.square"),
        MenuEntry::separator(),
        MenuEntry::item("Move to Trash").symbol("trash"),
    ]
}

fn region_entries() -> Vec<MenuEntry> {
    vec![
        MenuEntry::item("Tag Red").symbol("circle.fill"),
        MenuEntry::item("Tag Green").symbol("circle.fill"),
        MenuEntry::item("Tag Purple").symbol("circle.fill"),
        MenuEntry::separator(),
        MenuEntry::item("Locked").symbol("lock").disabled(),
    ]
}

fn tag_color(index: Option<usize>) -> Hsla {
    match index {
        Some(0) => rgb(0xff453a).into(),
        Some(1) => rgb(0x30d158).into(),
        Some(2) => rgb(0xbf5af2).into(),
        _ => rgb(0x2c2c2e).into(),
    }
}

fn title(entries: &[MenuEntry], index: usize) -> String {
    match entries.get(index) {
        Some(MenuEntry::Item { title, .. }) => title.clone(),
        _ => "—".into(),
    }
}

fn region(label: &'static str, tag: Option<usize>) -> gpui::Div {
    div()
        .w(px(240.))
        .h(px(110.))
        .rounded_lg()
        .bg(tag_color(tag))
        .flex()
        .items_center()
        .justify_center()
        .child(format!("{label} — right-click"))
}

struct Menus {
    accent: usize,
    document: Option<usize>,
    region: Option<usize>,
    pointer_region: Option<usize>,
}

impl Render for Menus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let document_title = self.document.map_or_else(
            || "—".to_string(),
            |index| title(&document_entries(), index),
        );

        native_root(
            div()
                .size_full()
                .bg(rgb(0x1c1c1e))
                .text_color(rgb(0xf2f2f7))
                .flex()
                .flex_col()
                .p_6()
                .gap_5()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            native_popup("accent", self.accent)
                                .entries(accent_entries())
                                .on_change(cx.listener(|this, index: &usize, _, cx| {
                                    this.accent = *index;
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .size(px(40.))
                                .rounded_md()
                                .bg(accent_color(self.accent)),
                        )
                        .child(title(&accent_entries(), self.accent)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            native_popup("document", self.document.unwrap_or(0))
                                .pull_down(true)
                                .entries(document_entries())
                                .on_change(cx.listener(|this, index: &usize, _, cx| {
                                    this.document = Some(*index);
                                    cx.notify();
                                })),
                        )
                        .child(format!("action: {document_title}")),
                )
                .child(
                    native_popup("locked", 0)
                        .items(["Locked pop-up"])
                        .enabled(false),
                )
                .child(
                    native_context_menu("element-origin", region("ELEMENT", self.region))
                        .origin(MenuOrigin::ElementTopLeft)
                        .entries(region_entries())
                        .on_select(cx.listener(|this, index: &usize, _, cx| {
                            this.region = Some(*index);
                            cx.notify();
                        })),
                )
                .child(
                    native_context_menu("pointer", region("POINTER", self.pointer_region))
                        .entries(region_entries())
                        .on_select(cx.listener(|this, index: &usize, _, cx| {
                            this.pointer_region = Some(*index);
                            cx.notify();
                        })),
                ),
        )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(1400.), px(120.)),
                    size: size(px(520.), px(560.)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|_| Menus {
                    accent: 0,
                    document: None,
                    region: None,
                    pointer_region: None,
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
