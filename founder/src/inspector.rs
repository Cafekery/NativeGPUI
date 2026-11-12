use std::cell::Cell;

use gpui::{
    AnyView, App, Bounds, Context, EntityId, Render, WeakEntity, Window, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::window_options;
use objc2::msg_send;
use objc2_app_kit::{NSWindowStyleMask, NSWindowTitleVisibility};

use crate::app::FinderDemo;
use crate::data::{Line, path_line};
use crate::frame::ns_window;
use crate::info::{Combined, HEIGHT, Info, Remote, WIDTH, where_of};

thread_local! {
    static OPEN: Cell<Option<WindowHandle<Inspector>>> = const { Cell::new(None) };
    static FOLLOWED: Cell<Option<(EntityId, u64, u64, &'static str)>> = const { Cell::new(None) };
}

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

pub(crate) fn toggle(demo: &FinderDemo, cx: &mut Context<FinderDemo>) {
    match OPEN.take() {
        Some(open) => cx.defer(move |cx| {
            open.update(cx, |_, window, _| window.remove_window()).ok();
        }),
        None => show_for(subject(demo), demo, cx),
    }
}

pub(crate) fn show_item(id: &'static str, demo: &FinderDemo, cx: &mut Context<FinderDemo>) {
    show_for(path_line(id).into_iter().collect(), demo, cx);
}

pub(crate) fn follow(demo: &FinderDemo, window: &Window, cx: &mut Context<FinderDemo>) {
    if OPEN.get().is_none() || !window.is_window_active() {
        return;
    }
    if FOLLOWED.get() != Some(followed(demo, cx)) {
        show_for(subject(demo), demo, cx);
    }
}

fn show_for(lines: Vec<Line>, demo: &FinderDemo, cx: &mut Context<FinderDemo>) {
    FOLLOWED.set(Some(followed(demo, cx)));
    let demo = cx.entity().downgrade();
    cx.defer(move |cx| show(lines, demo, cx));
}

fn followed(demo: &FinderDemo, cx: &Context<FinderDemo>) -> (EntityId, u64, u64, &'static str) {
    (
        cx.entity_id(),
        demo.selected.rev(),
        demo.folders.changed(),
        demo.here(),
    )
}

fn subject(demo: &FinderDemo) -> Vec<Line> {
    let chosen = demo.chosen_lines();
    if chosen.is_empty() {
        return path_line(demo.here()).into_iter().collect();
    }
    chosen
}

fn show(lines: Vec<Line>, demo: WeakEntity<FinderDemo>, cx: &mut App) {
    let Some(first) = lines.first() else {
        return;
    };
    let where_ = where_of(&first.key).unwrap_or_default().into();
    let (title, shown): (String, AnyView) = if crate::remote::is_remote(&first.key) {
        let remote = Remote::new(lines, where_);
        (remote.title(), cx.new(|_| remote).into())
    } else {
        match <[Line; 1]>::try_from(lines) {
            Ok([line]) => (
                format!("{} Info", line.row.display),
                cx.new(|_| Info::new(line, where_, demo).in_panel()).into(),
            ),
            Err(lines) => (
                "Multiple Item Info".to_owned(),
                cx.new(|_| Combined::new(lines, where_)).into(),
            ),
        }
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
            // Never key, so gpui would hold its section folds to its inactive-window frame rate.
            inactive_frame_interval: None,
            ..window_options()
        },
        |window, cx| {
            window.set_window_title(&title);
            if let Some(panel) = ns_window(window) {
                unsafe {
                    let _: () = msg_send![&*panel, setBecomesKeyOnlyIfNeeded: true];
                    let _: () = msg_send![&*panel, setFloatingPanel: true];
                }
                panel.setStyleMask(panel.styleMask() | NSWindowStyleMask::UtilityWindow);
                panel.setTitleVisibility(NSWindowTitleVisibility::Visible);
                crate::chrome::toolbar::system_corners_of(&panel);
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
