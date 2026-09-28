use std::cell::Cell;

use finder_fs::ds_store::IconOptions;
use gpui::{
    App, Bounds, Context, Div, Entity, FocusHandle, IntoElement, ParentElement, Pixels, Render,
    SharedString, Size, Styled, WeakEntity, Window, WindowBounds, WindowHandle, WindowKind,
    WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{
    CheckState, MenuEntry, NativePopup, native_checkbox, native_popup, native_radio, native_root,
    native_slider, window_options,
};
use objc2::msg_send;
use objc2_app_kit::{NSWindowStyleMask, NSWindowTitleVisibility};

use crate::app::FinderDemo;
use crate::data::{Column, SORT_KEYS, SORT_TITLES};
use crate::frame::ns_window;
use crate::menus::ShowViewOptions;
use crate::view::{ICON_VIEW, LIST_VIEW};
use crate::views::header::column_title;
use crate::views::icon::{Arrangement, SIZE_STEPS, size_slider, slider_size};

const TEXT_SIZES: [f32; 7] = [10., 11., 12., 13., 14., 15., 16.];

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
fn panel_size(view: usize) -> Size<Pixels> {
    let height = match view {
        LIST_VIEW => 341.,
        ICON_VIEW => 182. + TITLEBAR_H,
        _ => 170.,
    };
    size(px(WIDTH), px(height))
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
            .child(Self::label(label).w(px(86.)))
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
        let options = state.icon_options();
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
                    let change = |change: fn(&mut IconOptions, f64)| {
                        let demo = demo.clone();
                        move |value: &f64, _: &mut Window, cx: &mut App| {
                            let value = *value;
                            demo.update(cx, |this, cx| {
                                this.change_icon_options(|options| change(options, value), cx);
                            });
                        }
                    };
                    let label = |bottom: bool, title: &str| {
                        let demo = demo.clone();
                        native_radio(
                            ("ng.finder.options.label", usize::from(bottom)),
                            options.bottom == bottom,
                        )
                        .title(title)
                        .on_change(move |_: &bool, _, cx| {
                            demo.update(cx, |this, cx| {
                                this.change_icon_options(
                                    |options| options.label_on_bottom = Some(bottom),
                                    cx,
                                );
                            });
                        })
                    };
                    let size = SharedString::from(format!("{0} × {0}", options.size));
                    panel
                        .child(Self::row(
                            "Icon size:",
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(tokens::small_system_font_size())
                                        .text_color(system_color(SystemColor::SecondaryLabel))
                                        .child(size),
                                )
                                .child(
                                    native_slider(
                                        "ng.finder.options.size",
                                        size_slider(options.size),
                                    )
                                    .range(0., SIZE_STEPS)
                                    .on_change(change(
                                        |options, step| {
                                            options.icon_size = Some(slider_size(step));
                                        },
                                    )),
                                ),
                        ))
                        .child(Self::row(
                            "Grid spacing:",
                            native_slider("ng.finder.options.spacing", f64::from(options.spacing))
                                .range(1., 100.)
                                .on_change(change(|options, spacing| {
                                    options.grid_spacing = Some(spacing.round() as f32);
                                })),
                        ))
                        .child(Self::row(
                            "Text size:",
                            native_popup(
                                "ng.finder.options.text",
                                TEXT_SIZES
                                    .iter()
                                    .position(|size| *size == options.text)
                                    .unwrap_or(2),
                            )
                            .entries(
                                TEXT_SIZES
                                    .iter()
                                    .map(|size| MenuEntry::item(size.to_string())),
                            )
                            .on_change({
                                let demo = demo.clone();
                                move |index: &usize, _, cx| {
                                    let Some(size) = TEXT_SIZES.get(*index).copied() else {
                                        return;
                                    };
                                    demo.update(cx, |this, cx| {
                                        this.change_icon_options(
                                            |options| options.text_size = Some(size),
                                            cx,
                                        );
                                    });
                                }
                            }),
                        ))
                        .child(Self::row(
                            "Label position:",
                            div()
                                .flex()
                                .gap(px(8.))
                                .child(label(true, "Bottom"))
                                .child(label(false, "Right")),
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
