//! Finder's two overlays: the Go to Folder sheet and Quick Look.

use gpui::{AnyElement, Context, FontWeight, WeakEntity, Window, div, prelude::*, px};
use native_gpui::tokens;
use native_gpui::{Material, material, system_corner_radius};

use crate::FinderDemo;
use crate::palette::Palette;
use crate::ui::{focus_gpui, key_to_parent, native_text_field};

const SHEET_W: f32 = 520.;

impl FinderDemo {
    /// Finder's ⇧⌘G sheet: a path field that walks the window when it resolves.
    pub(crate) fn go_sheet(
        &self,
        text: &str,
        palette: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this: WeakEntity<Self> = cx.entity().downgrade();
        let leave = this.clone();
        let field = native_text_field("ng.finder.go", text)
            .stretch()
            .editing(0..text.chars().count())
            .on_submit(move |typed, window, cx| {
                let typed = typed.to_string();
                this.update(cx, |demo: &mut FinderDemo, cx| {
                    if let Some(id) = demo.folder_at_path(&typed) {
                        demo.go(id, window, cx);
                    }
                    demo.go_sheet = None;
                    focus_gpui(window);
                    key_to_parent(window);
                    cx.notify();
                })
                .ok();
            })
            .on_cancel(move |_, window, cx| {
                leave
                    .update(cx, |demo: &mut FinderDemo, cx| {
                        demo.go_sheet = None;
                        focus_gpui(window);
                        key_to_parent(window);
                        cx.notify();
                    })
                    .ok();
            });

        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w(px(SHEET_W))
                    .rounded_b(system_corner_radius(window))
                    .overflow_hidden()
                    .child(
                        material("ng.finder.go.glass", Material::Popover)
                            .absolute()
                            .inset_0(),
                    )
                    .child(
                        div()
                            .relative()
                            .px(px(20.))
                            .py(px(18.))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .font_family(tokens::system_font_family())
                            .text_color(palette.label)
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Go to the folder:"),
                            )
                            .child(div().h(px(24.)).child(field)),
                    ),
            )
            .into_any_element()
    }
}
