use std::cell::RefCell;

use gpui::{
    AnyWindowHandle, App, Bounds, Context, FontWeight, Render, WeakEntity, Window, WindowBounds,
    WindowHandle, WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{
    LabelAlign, NativeTextField, native_button, native_label, native_root, native_text_field,
    titlebar_band, window_options,
};
use objc2::MainThreadMarker;
use objc2_app_kit::NSWindowTitleVisibility;

use super::toolbar::{system_corners, titled_height};
use crate::app::FinderDemo;
use crate::form::text_height;

const WIDTH: f32 = 470.;
const EDGE: f32 = 20.;
const GAP: f32 = 10.;
const TEXT_W: f32 = WIDTH - 2. * EDGE;
const LABEL_W: f32 = 84.;
const FIELD_H: f32 = 22.;
const PORT_W: f32 = 64.;
const BUTTON_W: f32 = 84.;
const BUTTON_H: f32 = 32.;
const DEFAULT_PORT: &str = "22";
const TITLE: &str = "Browse a computer over SSH";
const INTRO: &str = "Anything you can reach with ssh, Found\u{2019}er can browse like a folder. Files you preview or open are copied to a local cache first, so browsing stays fast.";
const HINT: &str = "Next time you can type that address straight into Go \u{25B8} Connect to Server (\u{2318}K). You\u{2019}ll be asked for the password on the first connection; tick Remember to keep it in your keychain.";

thread_local! {
    static OPEN: RefCell<Option<WindowHandle<SftpForm>>> = const { RefCell::new(None) };
}

pub(crate) struct SftpForm {
    demo: WeakEntity<FinderDemo>,
    host: AnyWindowHandle,
    server: String,
    user: String,
    port: String,
    folder: String,
    fresh: bool,
}

pub(crate) fn show(demo: WeakEntity<FinderDemo>, host: AnyWindowHandle, cx: &mut App) {
    let raised = OPEN.with_borrow(|open| *open).is_some_and(|open| {
        open.update(cx, |form, window, _| {
            (form.demo, form.host) = (demo.clone(), host);
            window.activate_window();
        })
        .is_ok()
    });
    if raised {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let mut bounds = Bounds::centered(None, size(px(WIDTH), titled_height(height(), mtm)), cx);
    bounds.origin = point(bounds.origin.x.floor(), bounds.origin.y.floor());
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            is_resizable: false,
            ..window_options()
        },
        |window, cx| {
            window.set_window_title("Connect to SFTP Server");
            system_corners(window);
            if let Some(ns) = crate::frame::ns_window(window) {
                ns.setTitleVisibility(NSWindowTitleVisibility::Visible);
            }
            cx.new(|_| SftpForm {
                demo,
                host,
                server: String::new(),
                user: String::new(),
                port: DEFAULT_PORT.into(),
                folder: String::new(),
                fresh: true,
            })
        },
    );
    if let Ok(opened) = opened {
        OPEN.set(Some(opened));
    }
}

impl SftpForm {
    fn address(&self) -> String {
        let mut address = String::from("sftp://");
        let user = self.user.trim();
        if !user.is_empty() {
            address.push_str(user);
            address.push('@');
        }
        address.push_str(self.server.trim());
        let port = self.port.trim();
        if !port.is_empty() && port != DEFAULT_PORT {
            address.push(':');
            address.push_str(port);
        }
        let folder = self.folder.trim();
        if !folder.is_empty() {
            if !folder.starts_with('/') {
                address.push('/');
            }
            address.push_str(folder);
        }
        address
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.server.trim().is_empty() {
            return;
        }
        super::connect::connect_to(self.address(), self.demo.clone(), self.host, cx);
        self.close(window);
    }

    fn close(&self, window: &mut Window) {
        OPEN.set(None);
        window.remove_window();
    }

    fn field(
        &self,
        id: &'static str,
        text: &str,
        placeholder: &'static str,
        set: fn(&mut SftpForm, String),
        cx: &mut Context<Self>,
    ) -> NativeTextField {
        native_text_field(id, text)
            .stretch()
            .placeholder(placeholder)
            .on_change(cx.listener(move |form, text: &String, _, cx| {
                set(form, text.clone());
                cx.notify();
            }))
            .on_submit(cx.listener(|form, _, window, cx| form.connect(window, cx)))
            .on_cancel(cx.listener(|form, _, window, _| form.close(window)))
    }
}

fn row(label: &'static str, field: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .h(px(FIELD_H))
        .gap(px(8.))
        .child(
            div().w(px(LABEL_W)).child(
                native_label(format!("ng.finder.sftp.label.{label}"), label)
                    .font_size(tokens::system_font_size())
                    .align(LabelAlign::Right),
            ),
        )
        .child(div().flex_1().h(px(FIELD_H)).child(field))
}

fn small(text: &str) -> f32 {
    text_height(
        text,
        f32::from(tokens::small_system_font_size()),
        FontWeight::NORMAL,
        TEXT_W,
    )
}

fn note(id: &'static str, text: &'static str) -> impl IntoElement {
    div().flex_shrink_0().h(px(small(text))).child(
        native_label(id, text)
            .font_size(tokens::small_system_font_size())
            .color(SystemColor::SecondaryLabel)
            .wraps(true),
    )
}

/// Content height: eleven stacked children, the two notes measured at the window's text width.
fn height() -> f32 {
    let title = text_height(
        TITLE,
        f32::from(tokens::system_font_size()),
        FontWeight::BOLD,
        TEXT_W,
    );
    let rows = 2. * 4. + 4. * FIELD_H + small("Address:") + BUTTON_H;
    2. * EDGE + 10. * GAP + title + small(INTRO) + rows + small(HINT)
}

impl Render for SftpForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = !self.server.trim().is_empty();
        let mut server = self.field(
            "ng.finder.sftp.server",
            &self.server,
            "example.com or 192.168.1.20",
            |form, text| form.server = text,
            cx,
        );
        if std::mem::take(&mut self.fresh) {
            server = server.editing(0..0);
        }
        let user = self.field(
            "ng.finder.sftp.user",
            &self.user,
            "Your login on the server",
            |form, text| form.user = text,
            cx,
        );
        let port = self.field(
            "ng.finder.sftp.port",
            &self.port,
            DEFAULT_PORT,
            |form, text| form.port = text,
            cx,
        );
        let folder = self.field(
            "ng.finder.sftp.folder",
            &self.folder,
            "Leave empty to start in your home folder",
            |form, text| form.folder = text,
            cx,
        );
        native_root(
            div()
                .size_full()
                .bg(system_color(SystemColor::WindowBackground))
                .flex()
                .flex_col()
                .gap(px(GAP))
                .p(px(EDGE))
                .pt(titlebar_band(window) + px(EDGE))
                .child(
                    div().flex_shrink_0().child(
                        native_label("ng.finder.sftp.title", TITLE)
                            .font_size(tokens::system_font_size())
                            .weight(FontWeight::BOLD),
                    ),
                )
                .child(note("ng.finder.sftp.intro", INTRO))
                .child(div().h(px(4.)))
                .child(row("Server:", server))
                .child(row("User Name:", user))
                .child(row(
                    "Port:",
                    div()
                        .flex()
                        .child(div().w(px(PORT_W)).h(px(FIELD_H)).child(port)),
                ))
                .child(row("Folder:", folder))
                .child(div().h(px(4.)))
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .gap(px(8.))
                        .child(
                            div().w(px(LABEL_W)).child(
                                native_label("ng.finder.sftp.address.label", "Address:")
                                    .font_size(tokens::small_system_font_size())
                                    .color(SystemColor::SecondaryLabel)
                                    .align(LabelAlign::Right),
                            ),
                        )
                        .child(
                            div().flex_1().child(
                                native_label("ng.finder.sftp.address", &self.address())
                                    .font_size(tokens::small_system_font_size())
                                    .color(SystemColor::SecondaryLabel)
                                    .truncates(true),
                            ),
                        ),
                )
                .child(note("ng.finder.sftp.hint", HINT))
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .h(px(BUTTON_H))
                        .items_end()
                        .justify_end()
                        .gap(px(12.))
                        .child(
                            native_button("ng.finder.sftp.cancel", "Cancel")
                                .width(px(BUTTON_W))
                                .on_click(cx.listener(|form, _, window, _| form.close(window))),
                        )
                        .child(
                            native_button("ng.finder.sftp.connect", "Connect")
                                .width(px(BUTTON_W))
                                .default_button(true)
                                .enabled(ready)
                                .on_click(
                                    cx.listener(|form, _, window, cx| form.connect(window, cx)),
                                ),
                        ),
                ),
        )
    }
}
