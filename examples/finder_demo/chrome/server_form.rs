use std::cell::Cell;

use gpui::{
    AnyWindowHandle, App, Bounds, Context, Entity, FontWeight, Render, Window, WindowBounds,
    WindowOptions, div, point, prelude::*, px, relative, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{
    CheckState, LabelAlign, MenuEntry, NativeTextField, begin_sheet, native_button,
    native_checkbox, native_label, native_popup, native_root, native_text_field, window_options,
};

use super::connect::Connect;
use super::toolbar::system_corners;
use crate::form::text_height;
use crate::frame::ns_window;
use crate::servers::{Protocol, Saved};

const WIDTH: f32 = 480.;
const EDGE: f32 = 20.;
const GAP: f32 = 10.;
const LABEL_W: f32 = 100.;
const LABEL_GAP: f32 = 8.;
const FIELD_W: f32 = WIDTH - 2. * EDGE - LABEL_W - LABEL_GAP;
const FIELD_H: f32 = 22.;
const PORT_W: f32 = 64.;
const CHOOSE_W: f32 = 84.;
const BUTTON_W: f32 = 84.;
const BUTTON_H: f32 = 32.;
const OTHER: usize = 3;

thread_local! {
    static OPEN: Cell<bool> = const { Cell::new(false) };
}

pub(crate) struct ServerForm {
    connect: Entity<Connect>,
    at: Option<usize>,
    saved: Saved,
    address: String,
    fresh: bool,
}

pub(crate) fn open(
    connect: Entity<Connect>,
    host: AnyWindowHandle,
    at: Option<usize>,
    saved: Saved,
    cx: &mut App,
) {
    if OPEN.replace(true) {
        return;
    }
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(WIDTH), px(height())),
            })),
            show: false,
            is_minimizable: false,
            is_resizable: false,
            ..window_options()
        },
        |window, cx| {
            system_corners(window);
            cx.new(|_| ServerForm {
                connect,
                at,
                address: saved.address(),
                saved,
                fresh: true,
            })
        },
    );
    match opened {
        Ok(opened) => begin_sheet(host, opened.into(), cx),
        Err(_) => OPEN.set(false),
    }
}

fn popup_index(protocol: Protocol) -> usize {
    let at = Protocol::ALL
        .iter()
        .position(|&p| p == protocol)
        .unwrap_or(0);
    at + usize::from(at >= OTHER)
}

fn popup_protocol(at: usize) -> Protocol {
    Protocol::ALL[at - usize::from(at > OTHER)]
}

fn folder_hint(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Smb | Protocol::Afp => "Name of the share, e.g. Media",
        Protocol::Sftp => "Leave empty to start in your home folder",
        Protocol::WebDav => "Path on the server, if any",
        Protocol::Nfs => "Exported path, e.g. /volume1/media",
        Protocol::Ftp => "Leave empty for the top folder",
    }
}

fn small(text: &str) -> f32 {
    text_height(
        text,
        f32::from(tokens::small_system_font_size()),
        FontWeight::NORMAL,
        FIELD_W,
    )
}

fn hint_height() -> f32 {
    Protocol::ALL
        .iter()
        .map(|protocol| small(protocol.hint()))
        .fold(0., f32::max)
}

fn height() -> f32 {
    let title = text_height(
        "Edit Server",
        f32::from(tokens::system_font_size()),
        FontWeight::BOLD,
        FIELD_W,
    );
    2. * EDGE + 9. * GAP + 4. + title + 8. * FIELD_H + hint_height() + BUTTON_H
}

fn row(label: &str, field: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .h(px(FIELD_H))
        .gap(px(LABEL_GAP))
        .child(
            div().w(px(LABEL_W)).child(
                native_label(format!("ng.finder.server.label.{label}"), label)
                    .font_size(tokens::system_font_size())
                    .align(LabelAlign::Right),
            ),
        )
        .child(div().flex_1().h(px(FIELD_H)).child(field))
}

impl ServerForm {
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saved.host.trim().is_empty() {
            return;
        }
        let (at, saved) = (self.at, self.saved.clone());
        self.connect
            .update(cx, |connect, cx| connect.store(at, saved, cx));
        self.close(window, cx);
    }

    fn close(&self, window: &mut Window, cx: &mut Context<Self>) {
        OPEN.set(false);
        let (sheet, handle) = (ns_window(window), window.window_handle());
        cx.spawn(async move |_, cx| {
            if let Some(parent) = sheet.as_ref().and_then(|sheet| sheet.sheetParent()) {
                parent.endSheet(sheet.as_ref().unwrap());
            }
            handle
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        })
        .detach();
    }

    fn set_protocol(&mut self, protocol: Protocol) {
        if self.saved.port.trim() == self.saved.protocol.default_port().to_string() {
            self.saved.port.clear();
        }
        self.saved.protocol = protocol;
        self.address = self.saved.address();
    }

    fn typed_address(&mut self, text: &str) {
        self.address = text.to_owned();
        if let Some(mut parsed) = Saved::parse(text) {
            parsed.name.clone_from(&self.saved.name);
            parsed.identity.clone_from(&self.saved.identity);
            self.saved = parsed;
        }
    }

    fn field(
        &self,
        id: &'static str,
        text: &str,
        placeholder: &'static str,
        set: fn(&mut Saved, String),
        cx: &mut Context<Self>,
    ) -> NativeTextField {
        native_text_field(id, text)
            .stretch()
            .placeholder(placeholder)
            .on_change(cx.listener(move |form, text: &String, _, cx| {
                set(&mut form.saved, text.clone());
                form.address = form.saved.address();
                cx.notify();
            }))
            .on_submit(cx.listener(|form, _, window, cx| form.save(window, cx)))
            .on_cancel(cx.listener(|form, _, window, cx| form.close(window, cx)))
    }

    fn extra_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        match self.saved.protocol {
            Protocol::Sftp => row(
                "Identity file:",
                div()
                    .flex()
                    .gap(px(LABEL_GAP))
                    .child(div().flex_1().h(px(FIELD_H)).child(self.field(
                        "ng.finder.server.identity",
                        &self.saved.identity,
                        "Optional; ssh picks a key when empty",
                        |saved, text| saved.identity = text,
                        cx,
                    )))
                    .child(
                        native_button("ng.finder.server.choose", "Choose\u{2026}")
                            .width(px(CHOOSE_W))
                            .on_click(cx.listener(|_, _, _, cx| {
                                let ssh = crate::home().join(".ssh");
                                crate::form::choose_file(cx, Some(ssh), |form, path, cx| {
                                    form.saved.identity = path.to_string_lossy().into_owned();
                                    cx.notify();
                                });
                            })),
                    ),
            )
            .into_any_element(),
            Protocol::WebDav => div()
                .flex_shrink_0()
                .h(px(FIELD_H))
                .pl(px(LABEL_W + LABEL_GAP))
                .child(
                    native_checkbox("ng.finder.server.https", self.saved.https)
                        .title("Secure connection (HTTPS)")
                        .on_change(cx.listener(|form, state: &CheckState, _, cx| {
                            form.saved.https = *state == CheckState::On;
                            form.address = form.saved.address();
                            cx.notify();
                        })),
                )
                .into_any_element(),
            _ => div().flex_shrink_0().h(px(FIELD_H)).into_any_element(),
        }
    }
}

impl Render for ServerForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let protocol = self.saved.protocol;
        let title = if self.at.is_some() {
            "Edit Server"
        } else {
            "New Server"
        };
        let mut name = self.field(
            "ng.finder.server.name",
            &self.saved.name,
            "Optional, e.g. Home NAS",
            |saved, text| saved.name = text,
            cx,
        );
        if std::mem::take(&mut self.fresh) {
            name = name.editing(0..self.saved.name.chars().count());
        }
        let mut entries: Vec<MenuEntry> = Protocol::ALL
            .iter()
            .map(|protocol| MenuEntry::item(protocol.title()))
            .collect();
        entries.insert(OTHER, MenuEntry::separator());
        let protocols = native_popup("ng.finder.server.protocol", popup_index(protocol))
            .entries(entries)
            .width(relative(1.))
            .on_change(cx.listener(|form, at: &usize, _, cx| {
                form.set_protocol(popup_protocol(*at));
                cx.notify();
            }));
        let server = self.field(
            "ng.finder.server.host",
            &self.saved.host,
            "example.com or 192.168.1.20",
            |saved, text| saved.host = text,
            cx,
        );
        let user = self
            .field(
                "ng.finder.server.user",
                &self.saved.user,
                "Your login on the server",
                |saved, text| saved.user = text,
                cx,
            )
            .enabled(protocol != Protocol::Nfs);
        let port = self.field(
            "ng.finder.server.port",
            &self.saved.port,
            match protocol {
                Protocol::Smb => "445",
                Protocol::Sftp => "22",
                Protocol::WebDav => "443",
                Protocol::Afp => "548",
                Protocol::Nfs => "2049",
                Protocol::Ftp => "21",
            },
            |saved, text| saved.port = text,
            cx,
        );
        let folder = self.field(
            "ng.finder.server.path",
            &self.saved.path,
            folder_hint(protocol),
            |saved, text| saved.path = text,
            cx,
        );
        let address = native_text_field("ng.finder.server.address", &self.address)
            .stretch()
            .placeholder("smb://user@server/share")
            .on_change(cx.listener(|form, text: &String, _, cx| {
                form.typed_address(text);
                cx.notify();
            }))
            .on_submit(cx.listener(|form, _, window, cx| form.save(window, cx)))
            .on_cancel(cx.listener(|form, _, window, cx| form.close(window, cx)));
        native_root(
            div()
                .size_full()
                .bg(system_color(SystemColor::WindowBackground))
                .flex()
                .flex_col()
                .gap(px(GAP))
                .p(px(EDGE))
                .child(
                    div().flex_shrink_0().child(
                        native_label("ng.finder.server.title", title)
                            .font_size(tokens::system_font_size())
                            .weight(FontWeight::BOLD),
                    ),
                )
                .child(row("Name:", name))
                .child(row("Connect using:", protocols))
                .child(
                    div()
                        .flex_shrink_0()
                        .mt(px(4. - GAP))
                        .pl(px(LABEL_W + LABEL_GAP))
                        .h(px(hint_height()))
                        .child(
                            native_label("ng.finder.server.hint", protocol.hint())
                                .font_size(tokens::small_system_font_size())
                                .color(SystemColor::SecondaryLabel)
                                .wraps(true),
                        ),
                )
                .child(row("Server:", server))
                .child(row("User Name:", user))
                .child(row(
                    "Port:",
                    div()
                        .flex()
                        .child(div().w(px(PORT_W)).h(px(FIELD_H)).child(port)),
                ))
                .child(row(
                    if protocol.shares() {
                        "Share:"
                    } else {
                        "Folder:"
                    },
                    folder,
                ))
                .child(self.extra_row(cx))
                .child(row("Address:", address))
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .h(px(BUTTON_H))
                        .items_end()
                        .justify_end()
                        .gap(px(12.))
                        .child(
                            native_button("ng.finder.server.cancel", "Cancel")
                                .width(px(BUTTON_W))
                                .on_click(
                                    cx.listener(|form, _, window, cx| form.close(window, cx)),
                                ),
                        )
                        .child(
                            native_button("ng.finder.server.save", "Save")
                                .width(px(BUTTON_W))
                                .default_button(true)
                                .enabled(!self.saved.host.trim().is_empty())
                                .on_click(cx.listener(|form, _, window, cx| form.save(window, cx))),
                        ),
                ),
        )
    }
}
