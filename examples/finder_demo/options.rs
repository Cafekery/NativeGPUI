use std::cell::Cell;

use gpui::{
    App, Bounds, Context, Div, Entity, FocusHandle, IntoElement, ParentElement, Pixels, Render,
    SharedString, Size, Styled, WeakEntity, Window, WindowBounds, WindowHandle, WindowKind,
    WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{
    CheckState, MenuEntry, NativePopup, native_checkbox, native_popup, native_root, native_slider,
    window_options,
};
use objc2::msg_send;
use objc2_app_kit::{NSWindowStyleMask, NSWindowTitleVisibility};

use crate::app::FinderDemo;
use crate::data::{Column, SORT_KEYS, SORT_TITLES};
use crate::frame::ns_window;
use crate::menus::ShowViewOptions;
use crate::view::{ICON_VIEW, LIST_VIEW};
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

/// Finder's Show Columns, in its order; `None` rows are iCloud Drive's alone.
const SHOW_COLUMNS: [(&str, Option<Column>); 12] = [
    ("iCloud Status", None),
    ("Last Modified By", None),
    ("Shared By", None),
    ("Date Modified", Some(Column::Modified)),
    ("Date Created", Some(Column::Created)),
    ("Date Last Opened", Some(Column::Opened)),
    ("Date Added", Some(Column::Added)),
    ("Size", Some(Column::Size)),
    ("Kind", Some(Column::Kind)),
    ("Version", Some(Column::Version)),
    ("Comments", Some(Column::Comments)),
    ("Tags", Some(Column::Tags)),
];

const TITLEBAR_H: f32 = 34.;
const WIDTH: f32 = 268.;
const HEIGHT: f32 = 170.;
const LIST_HEIGHT: f32 = 341.;

fn panel_size(view: usize) -> Size<Pixels> {
    match view {
        LIST_VIEW => size(px(WIDTH), px(LIST_HEIGHT)),
        _ => size(px(WIDTH), px(HEIGHT)),
    }
}

pub(crate) const CONTEXT: &str = "ViewOptions";

pub(crate) struct ViewOptions {
    demo: WeakEntity<FinderDemo>,
    focus: FocusHandle,
}

impl ViewOptions {
    fn label(text: &str) -> Div {
        div()
            .text_size(tokens::small_system_font_size())
            .text_color(system_color(SystemColor::Label))
            .child(SharedString::from(text.to_string()))
    }

    fn row(label: &str, control: impl IntoElement) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(Self::label(label).w(px(74.)))
            .child(control)
    }

    fn show_columns(demo: &Entity<FinderDemo>, shown: &[Column]) -> impl IntoElement {
        let boxes = SHOW_COLUMNS
            .iter()
            .enumerate()
            .map(|(at, &(title, column))| {
                let demo = demo.clone();
                native_checkbox(
                    ("ng.finder.options.column", at),
                    column.is_some_and(|column| shown.contains(&column)),
                )
                .title(title)
                .enabled(column.is_some())
                .on_change(move |state: &CheckState, _, cx| {
                    let Some(column) = column else {
                        return;
                    };
                    demo.update(cx, |this, cx| {
                        let shown = this.list_columns.iter().any(|spec| spec.column == column);
                        if shown != (*state == CheckState::On) {
                            this.toggle_column(column, cx);
                            cx.notify();
                        }
                    });
                })
            });
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(Self::label("Show Columns:"))
            .children(boxes)
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
        let columns = (state.view == LIST_VIEW).then(|| {
            let shown: Vec<Column> = state.list_columns.iter().map(|spec| spec.column).collect();
            Self::show_columns(&demo, &shown)
        });

        native_root(
            div()
                .key_context(CONTEXT)
                .track_focus(&self.focus)
                .on_action(cx.listener(|_, _: &ShowViewOptions, window, _| window.remove_window()))
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
                })
                .children(columns),
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
    let view = demo.read(cx).view;
    let demo = demo.downgrade();
    let opened = cx.open_window(
        WindowOptions {
            kind: WindowKind::Floating,
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(320.), px(220.)),
                size: panel_size(view),
            })),
            is_resizable: false,
            is_minimizable: false,
            ..window_options()
        },
        |window, cx| {
            if let Some(panel) = ns_window(window) {
                unsafe {
                    let _: () = msg_send![&*panel, setFloatingPanel: true];
                }
                panel.setStyleMask(
                    panel.styleMask()
                        | NSWindowStyleMask::UtilityWindow
                        | NSWindowStyleMask::NonactivatingPanel,
                );
                panel.setTitleVisibility(NSWindowTitleVisibility::Visible);
            }
            cx.new(|cx| {
                cx.observe_in(&watched, window, |_, demo, window, cx| {
                    let size = panel_size(demo.read(cx).view);
                    if window.viewport_size() != size {
                        window.resize(size);
                    }
                    cx.notify();
                })
                .detach();
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                ViewOptions { demo, focus }
            })
        },
    );
    OPEN.set(opened.ok());
}
