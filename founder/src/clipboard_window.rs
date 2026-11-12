use std::cell::Cell;

use gpui::{
    App, Bounds, Context, SharedString, Window, WindowBounds, WindowHandle, WindowOptions, div,
    point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{native_label, native_root, titlebar_band, window_options};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString, NSWindowTitleVisibility};

use crate::chrome::toolbar::{system_corners, titled_height};
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
const SHOWN_CHARS: usize = 20_000;
const SHOWN_FILES: usize = 500;

thread_local! {
    static OPEN: Cell<Option<WindowHandle<Clipboard>>> = const { Cell::new(None) };
}

pub(crate) fn show(_: &ShowClipboard, cx: &mut App) {
    let raised = OPEN.get().is_some_and(|open| {
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
            system_corners(window);
            if let Some(ns) = ns_window(window) {
                ns.setTitleVisibility(NSWindowTitleVisibility::Visible);
            }
            cx.new(|cx| {
                cx.observe_window_activation(window, |_, _, cx| cx.notify())
                    .detach();
                Clipboard::default()
            })
        },
    );
    OPEN.set(opened.ok());
}

#[derive(Default)]
struct Clipboard {
    seen: Option<isize>,
    kind: &'static str,
    shown: SharedString,
}

fn capped(text: &str) -> String {
    match text.char_indices().nth(SHOWN_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

impl Clipboard {
    fn refresh(&mut self) {
        let board = NSPasteboard::generalPasteboard();
        let count = board.changeCount();
        if self.seen == Some(count) {
            return;
        }
        self.seen = Some(count);
        let files = read_pasteboard();
        (self.kind, self.shown) = if !files.is_empty() {
            let names: Vec<_> = files
                .iter()
                .take(SHOWN_FILES)
                .map(|path| {
                    path.file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                })
                .collect();
            ("file names", capped(&names.join("\n")).into())
        } else if let Some(text) = board.stringForType(unsafe { NSPasteboardTypeString }) {
            ("text", capped(&text.to_string()).into())
        } else {
            ("none", SharedString::default())
        };
    }
}

impl Render for Clipboard {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.refresh();
        let (kind, shown) = (self.kind, self.shown.clone());
        native_root(
            div()
                .size_full()
                .bg(system_color(SystemColor::WindowBackground))
                .pt(titlebar_band(window))
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("ng.finder.clipboard.text")
                        .flex_1()
                        .min_h(px(0.))
                        .overflow_y_scroll()
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
