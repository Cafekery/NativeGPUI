use gpui::{
    App, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled,
    WeakEntity, Window, WindowBounds, WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{MenuEntry, native_popup, native_root, native_slider, window_options};

use crate::app::FinderDemo;
use crate::data::Column;
use crate::view::ICON_VIEW;

const SORTS: [(Column, &str); 4] = [
    (Column::Name, "Name"),
    (Column::Kind, "Kind"),
    (Column::Modified, "Date Modified"),
    (Column::Size, "Size"),
];

pub(crate) struct ViewOptions {
    demo: WeakEntity<FinderDemo>,
}

impl ViewOptions {
    fn row(label: &str, control: impl IntoElement) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .w(px(74.))
                    .text_size(tokens::small_system_font_size())
                    .text_color(system_color(SystemColor::Label))
                    .child(SharedString::from(label.to_string())),
            )
            .child(control)
    }
}

impl Render for ViewOptions {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(demo) = self.demo.upgrade() else {
            return div().into_any_element();
        };
        let state = demo.read(cx);
        let title = state
            .folder()
            .map(|folder| folder.title)
            .unwrap_or("Finder");
        let sort = SORTS.iter().position(|(column, _)| *column == state.sort);
        let icon_size = state.icon_size;
        let icons = state.view == ICON_VIEW;

        native_root(
            div()
                .size_full()
                .p(px(14.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .bg(system_color(SystemColor::WindowBackground))
                .child(
                    div()
                        .text_size(tokens::small_system_font_size())
                        .text_color(system_color(SystemColor::Label))
                        .child(SharedString::from(title.to_string())),
                )
                .child(Self::row(
                    "Sort by:",
                    native_popup("ng.finder.options.sort", sort.unwrap_or(0))
                        .entries(SORTS.iter().map(|(_, label)| MenuEntry::item(*label)))
                        .on_change({
                            let demo = demo.clone();
                            move |index: &usize, _, cx| {
                                let column = SORTS[*index].0;
                                demo.update(cx, |this, cx| {
                                    this.sort_by(column, cx);
                                    cx.notify();
                                });
                            }
                        }),
                ))
                .when(icons, |panel| {
                    panel.child(Self::row(
                        "Icon size:",
                        native_slider("ng.finder.options.size", icon_size)
                            .range(32., 128.)
                            .on_change({
                                let demo = demo.clone();
                                move |value: &f64, _, cx| {
                                    let size = (value / 8.).round() * 8.;
                                    demo.update(cx, |this, cx| {
                                        this.icon_size = size;
                                        crate::prefs::set_icon_size(size as f32);
                                        cx.notify();
                                    });
                                }
                            }),
                    ))
                }),
        )
        .into_any_element()
    }
}

pub(crate) fn open(demo: &Entity<FinderDemo>, cx: &mut App) {
    let demo = demo.downgrade();
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(320.), px(220.)),
                size: size(px(268.), px(150.)),
            })),
            titlebar: None,
            ..window_options()
        },
        |_, cx| cx.new(|_| ViewOptions { demo }),
    );
}
