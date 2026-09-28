use std::cell::Cell;

use gpui::{
    App, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled,
    WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, div, point, prelude::*, px,
    size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{
    MenuEntry, NativePopup, native_popup, native_root, native_slider, window_options,
};
use objc2_app_kit::NSWindowTitleVisibility;

use crate::app::FinderDemo;
use crate::data::{Column, SORT_KEYS, SORT_TITLES};
use crate::frame::ns_window;
use crate::view::ICON_VIEW;
use crate::views::header::column_title;
use crate::views::icon::Arrangement;

/// `None` is a separator.
const ARRANGEMENTS: [Option<Arrangement>; 12] = [
    Some(Arrangement::Free),
    None,
    Some(Arrangement::Grid),
    None,
    Some(Arrangement::By(Column::Name)),
    Some(Arrangement::By(Column::Kind)),
    Some(Arrangement::By(Column::Opened)),
    Some(Arrangement::By(Column::Added)),
    Some(Arrangement::By(Column::Modified)),
    Some(Arrangement::By(Column::Created)),
    Some(Arrangement::By(Column::Size)),
    Some(Arrangement::By(Column::Tags)),
];

const TITLEBAR_H: f32 = 34.;

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

impl ViewOptions {
    fn arrange_popup(demo: &Entity<FinderDemo>, arranged: Arrangement) -> NativePopup {
        let entries = ARRANGEMENTS.iter().map(|arrangement| match arrangement {
            None => MenuEntry::separator(),
            Some(Arrangement::Free) => MenuEntry::item("None"),
            Some(Arrangement::Grid) => MenuEntry::item("Snap to Grid"),
            Some(Arrangement::By(column)) => MenuEntry::item(column_title(*column)),
        });
        let at = ARRANGEMENTS
            .iter()
            .position(|arrangement| *arrangement == Some(arranged));
        native_popup("ng.finder.options.arrange", at.unwrap_or(0))
            .entries(entries)
            .on_change({
                let demo = demo.clone();
                move |index: &usize, _, cx| {
                    let Some(arrangement) = ARRANGEMENTS.get(*index).copied().flatten() else {
                        return;
                    };
                    demo.update(cx, |this, cx| this.arrange(arrangement, cx));
                }
            })
    }
}

impl Render for ViewOptions {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(demo) = self.demo.upgrade() else {
            return div().into_any_element();
        };
        let state = demo.read(cx);
        let title = state
            .folder()
            .map(|folder| folder.title)
            .unwrap_or("Finder");
        window.set_window_title(title);
        let sort = SORT_KEYS.iter().position(|column| *column == state.sort);
        let icon_size = state.icon_size;
        let icons = state.view == ICON_VIEW;
        let arranged = state.arrangement().filter(|_| icons);

        native_root(
            div()
                .size_full()
                .p(px(14.))
                .pt(px(TITLEBAR_H))
                .flex()
                .flex_col()
                .gap(px(10.))
                .bg(system_color(SystemColor::WindowBackground))
                .child(Self::row(
                    "Sort By:",
                    match arranged {
                        Some(arranged) => Self::arrange_popup(&demo, arranged),
                        None => native_popup("ng.finder.options.sort", sort.unwrap_or(0))
                            .entries(SORT_TITLES.iter().map(|label| MenuEntry::item(*label)))
                            .on_change({
                                let demo = demo.clone();
                                move |index: &usize, _, cx| {
                                    let column = SORT_KEYS[*index];
                                    demo.update(cx, |this, cx| {
                                        this.sort_by(column, cx);
                                        cx.notify();
                                    });
                                }
                            }),
                    },
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

thread_local! {
    static OPEN: Cell<Option<WindowHandle<ViewOptions>>> = const { Cell::new(None) };
}

/// Whether the View Options window is up.
pub(crate) fn shown(cx: &App) -> bool {
    OPEN.get().is_some_and(|open| {
        cx.windows()
            .iter()
            .any(|window| window.window_id() == open.window_id())
    })
}

/// Finder's ⌘J toggles its one View Options window.
pub(crate) fn open(demo: &Entity<FinderDemo>, cx: &mut App) {
    if shown(cx) {
        if let Some(open) = OPEN.take() {
            let _ = open.update(cx, |_, window, _| window.remove_window());
        }
        return;
    }
    let watched = demo.clone();
    let demo = demo.downgrade();
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(320.), px(220.)),
                size: size(px(268.), px(150.) + px(TITLEBAR_H - 14.)),
            })),
            is_resizable: false,
            is_minimizable: false,
            ..window_options()
        },
        |_, cx| {
            cx.new(|cx| {
                cx.observe(&watched, |_, _, cx| cx.notify()).detach();
                ViewOptions { demo }
            })
        },
    );
    if let Ok(opened) = opened {
        OPEN.set(Some(opened));
        let _ = opened.update(cx, |_, window, _| {
            if let Some(ns_window) = ns_window(window) {
                ns_window.setTitleVisibility(NSWindowTitleVisibility::Visible);
            }
        });
    }
}
