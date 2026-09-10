use std::cell::RefCell;

use gpui::{
    App, Bounds, Context, Window, WindowBounds, WindowHandle, WindowOptions, div, point,
    prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::window_options;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};

use crate::menus::ShowClipboard;
use crate::ops::clipboard::read_pasteboard;

const WIDTH: f32 = 480.;
const HEIGHT: f32 = 320.;

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
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(120.), px(120.)),
                size: size(px(WIDTH), px(HEIGHT)),
            })),
            ..window_options()
        },
        |window, cx| {
            window.set_window_title("Clipboard");
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
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let files = read_pasteboard();
        let text = NSPasteboard::generalPasteboard()
            .stringForType(unsafe { NSPasteboardTypeString })
            .map(|text| text.to_string());
        let (kind, shown) = match (files.is_empty(), text) {
            (false, _) => {
                let names: Vec<String> = files
                    .iter()
                    .map(|path| {
                        path.file_name()
                            .unwrap_or(path.as_os_str())
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect();
                ("file names", names.join("\n"))
            }
            (true, Some(text)) => ("text", text),
            (true, None) => ("none", String::new()),
        };
        div()
            .size_full()
            .overflow_hidden()
            .bg(system_color(SystemColor::WindowBackground))
            .pt(px(34.))
            .px(px(12.))
            .pb(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_size(tokens::small_system_font_size())
                    .text_color(system_color(SystemColor::SecondaryLabel))
                    .child(format!("Clipboard contents: {kind}")),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_color(system_color(SystemColor::Label))
                    .child(shown),
            )
    }
}
