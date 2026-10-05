use std::cell::RefCell;

use gpui::{
    App, Bounds, Context, Window, WindowBounds, WindowHandle, WindowOptions, div, point,
    prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{native_label, native_root, titlebar_band, window_options};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString, NSWindowTitleVisibility};

use crate::chrome::toolbar::{titlebar_corners, titled_height};
use crate::frame::ns_window;
use crate::menus::ShowClipboard;
use crate::ops::clipboard::read_pasteboard;

const WIDTH: f32 = 488.;
const HEIGHT: f32 = 333.;
const MIN_W: f32 = 284.;
const MIN_H: f32 = 112.;
const STATUS_BAND: f32 = 20.;
const STATUS_BOTTOM: f32 = 3.;
const STATUS_INSET: f32 = 2.;

thread_local! {
    static OPEN: RefCell<Option<WindowHandle<Clipboard>>> = const { RefCell::new(None) };
}

pub(crate) fn show(_: &ShowClipboard, cx: &mut App) {
    let raised = OPEN.with_borrow(|open| *open).is_some_and(|open| {
        open.update(cx, |_, window, _| window.activate_window())
            .is_ok()
    });
    if raised {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(120.), px(120.)),
                size: size(px(WIDTH), titled_height(HEIGHT, mtm)),
            })),
            window_min_size: Some(size(px(MIN_W), px(MIN_H))),
            ..window_options()
        },
        |window, cx| {
            window.set_window_title("Clipboard");
            titlebar_corners(window);
            if let Some(ns) = ns_window(window) {
                ns.setTitleVisibility(NSWindowTitleVisibility::Visible);
            }
            cx.new(|cx| {
                cx.observe_window_activation(window, |_, _, cx| cx.notify())
                    .detach();
                Clipboard
            })
        },
    );
    if let Ok(opened) = opened {
        OPEN.with_borrow_mut(|open| *open = Some(opened));
    }
}

struct Clipboard;

impl Render for Clipboard {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let files = read_pasteboard();
        let (kind, shown) = if !files.is_empty() {
            let names: Vec<_> = files
                .iter()
                .map(|path| {
                    path.file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                })
                .collect();
            ("file names", names.join("\n"))
        } else if let Some(text) =
            NSPasteboard::generalPasteboard().stringForType(unsafe { NSPasteboardTypeString })
        {
            ("text", text.to_string())
        } else {
            ("none", String::new())
        };
        native_root(
            div()
                .size_full()
                .bg(system_color(SystemColor::WindowBackground))
                .pt(titlebar_band(window))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.))
                        .overflow_hidden()
                        .p(px(12.))
                        .text_color(system_color(SystemColor::Label))
                        .child(shown),
                )
                .child(
                    div().relative().flex_none().h(px(STATUS_BAND)).child(
                        div()
                            .absolute()
                            .left(px(STATUS_INSET))
                            .right(px(STATUS_INSET))
                            .bottom(px(STATUS_BOTTOM))
                            .child(
                                native_label(
                                    "ng.finder.clipboard.status",
                                    &format!("Clipboard contents: {kind}"),
                                )
                                .font_size(tokens::small_system_font_size()),
                            ),
                    ),
                ),
        )
    }
}
