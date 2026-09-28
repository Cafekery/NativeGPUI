use std::cell::Cell;
use std::path::Path;

use gpui::{
    AnyView, App, Bounds, Context, EntityId, Render, Window, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::window_options;
use objc2::msg_send;
use objc2_app_kit::NSWindowStyleMask;

use crate::app::FinderDemo;
use crate::data::{Line, folder_of, intern_path, path_line};
use crate::frame::ns_window;
use crate::info::{Combined, HEIGHT, Info, WIDTH};

thread_local! {
    static OPEN: Cell<Option<WindowHandle<Inspector>>> = const { Cell::new(None) };
    /// The window and state the inspector last followed; it follows again once they change.
    static FOLLOWED: Cell<Option<(EntityId, u64, u64, &'static str)>> = const { Cell::new(None) };
}

/// Finder's inspector: an Info window that floats and shows whatever the front window selects.
pub(crate) struct Inspector {
    shown: AnyView,
}

impl Render for Inspector {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.shown.clone())
    }
}

pub(crate) fn is_open() -> bool {
    OPEN.get().is_some()
}

/// Show Inspector and Hide Inspector.
pub(crate) fn toggle(demo: &FinderDemo, cx: &mut Context<FinderDemo>) {
    match OPEN.take() {
        Some(open) => {
            cx.defer(move |cx| {
                open.update(cx, |_, window, _| window.remove_window()).ok();
            });
        }
        None => {
            let lines = subject(demo);
            FOLLOWED.set(Some(followed(demo, cx)));
            cx.defer(move |cx| show(lines, cx));
        }
    }
}

/// The inspector on one item, until the front window's selection changes.
pub(crate) fn show_item(id: &'static str, demo: &FinderDemo, cx: &mut Context<FinderDemo>) {
    let lines: Vec<Line> = path_line(id).into_iter().collect();
    FOLLOWED.set(Some(followed(demo, cx)));
    cx.defer(move |cx| show(lines, cx));
}

/// Asked as each Finder window draws: the key one hands the inspector its selection.
pub(crate) fn follow(demo: &FinderDemo, window: &Window, cx: &mut Context<FinderDemo>) {
    if OPEN.get().is_none() || !window.is_window_active() {
        return;
    }
    let now = followed(demo, cx);
    if FOLLOWED.get() == Some(now) {
        return;
    }
    FOLLOWED.set(Some(now));
    let lines = subject(demo);
    cx.defer(move |cx| show(lines, cx));
}

fn followed(demo: &FinderDemo, cx: &Context<FinderDemo>) -> (EntityId, u64, u64, &'static str) {
    (
        cx.entity_id(),
        demo.selected.rev(),
        demo.folders.changed(),
        demo.here(),
    )
}

/// With nothing selected, Finder's Info is the folder's own.
fn subject(demo: &FinderDemo) -> Vec<Line> {
    let chosen = demo.chosen_lines();
    match chosen.is_empty() {
        true => path_line(demo.here()).into_iter().collect(),
        false => chosen,
    }
}

fn show(lines: Vec<Line>, cx: &mut App) {
    let Some(first) = lines.first() else {
        return;
    };
    let title = match lines.len() {
        1 => format!("{} Info", first.row.display),
        _ => "Multiple Item Info".to_owned(),
    };
    let where_ = Path::new(first.key.as_ref())
        .parent()
        .map(|parent| folder_of(intern_path(parent)).crumbs.join(" \u{203a} "))
        .unwrap_or_default()
        .into();
    let shown: AnyView = match lines.len() {
        1 => {
            let line = first.clone();
            cx.new(|_| Info { line, where_ }).into()
        }
        _ => cx.new(|_| Combined { lines, where_ }).into(),
    };
    if let Some(open) = OPEN.get() {
        let retitled = open.update(cx, |inspector, window, cx| {
            window.set_window_title(&title);
            inspector.shown = shown.clone();
            cx.notify();
        });
        if retitled.is_ok() {
            return;
        }
    }
    let opened = cx.open_window(
        WindowOptions {
            kind: WindowKind::Floating,
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(10.), px(54.)),
                size: size(px(WIDTH), px(HEIGHT)),
            })),
            focus: false,
            is_minimizable: false,
            ..window_options()
        },
        |window, cx| {
            window.set_window_title(&title);
            if let Some(panel) = ns_window(window) {
                // Key stays with the Finder window, whose arrow keys the inspector follows.
                unsafe {
                    let _: () = msg_send![&*panel, setBecomesKeyOnlyIfNeeded: true];
                    let _: () = msg_send![&*panel, setFloatingPanel: true];
                }
                panel.setStyleMask(panel.styleMask() | NSWindowStyleMask::UtilityWindow);
            }
            let id = window.window_handle().window_id();
            cx.new(|cx| {
                cx.on_release(move |_, _| {
                    if OPEN.get().is_some_and(|open| open.window_id() == id) {
                        OPEN.set(None);
                    }
                })
                .detach();
                Inspector { shown }
            })
        },
    );
    OPEN.set(opened.ok());
}
