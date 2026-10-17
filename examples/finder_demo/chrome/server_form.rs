use std::cell::Cell;

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, Div, Entity, FontWeight, Render,
    SharedString, Window, WindowBounds, WindowOptions, div, point, prelude::*, px, relative, size,
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

const WIDTH: f32 = 520.;
const EDGE: f32 = 20.;
const GAP: f32 = 12.;
const SECTION_GAP: f32 = 16.;
const HEADER_GAP: f32 = 6.;
const CARD_PAD: f32 = 12.;
const CARD_RADIUS: f32 = 10.;
const LABEL_W: f32 = 100.;
const LABEL_GAP: f32 = 8.;
const FIELD_W: f32 = WIDTH - 2. * (EDGE + CARD_PAD) - LABEL_W - LABEL_GAP;
const FIELD_H: f32 = 22.;
const HINT_TOP: f32 = 4.;
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
    height: f32,
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
    let height = height(saved.protocol);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(WIDTH), px(height)),
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
                height,
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

fn has_option(protocol: Protocol) -> bool {
    matches!(protocol, Protocol::Sftp | Protocol::WebDav)
}

fn hint_height() -> f32 {
    let small = f32::from(tokens::small_system_font_size());
    Protocol::ALL
        .iter()
        .map(|protocol| text_height(protocol.hint(), small, FontWeight::NORMAL, FIELD_W))
        .fold(0., f32::max)
}

fn height(protocol: Protocol) -> f32 {
    let (regular, small) = (
        f32::from(tokens::system_font_size()),
        f32::from(tokens::small_system_font_size()),
    );
    let title = text_height("Edit Server", regular, FontWeight::BOLD, FIELD_W);
    let card =
        text_height("Login", small, FontWeight::SEMIBOLD, FIELD_W) + HEADER_GAP + 2. * CARD_PAD;
    let general = 2. * FIELD_H + GAP + HINT_TOP + hint_height();
    let server = 3. * FIELD_H + 2. * GAP;
    let login = FIELD_H
        + if has_option(protocol) {
            FIELD_H + GAP
        } else {
            0.
        };
    2. * EDGE + title + 5. * SECTION_GAP + 4. * card + general + server + login + FIELD_H + BUTTON_H
}

fn row(label: &str, field: impl IntoElement) -> Div {
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

fn section(name: &'static str, rows: impl IntoIterator<Item = AnyElement>) -> Div {
    div()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(HEADER_GAP))
        .child(
            div().pl(px(4.)).child(
                native_label(format!("ng.finder.server.section.{name}"), name)
                    .font_size(tokens::small_system_font_size())
                    .weight(FontWeight::SEMIBOLD)
                    .color(SystemColor::SecondaryLabel),
            ),
        )
        .child(
            div()
                .rounded(px(CARD_RADIUS))
                .bg(system_color(SystemColor::QuaternarySystemFill))
                .p(px(CARD_PAD))
                .flex()
                .flex_col()
                .gap(px(GAP))
                .children(rows),
        )
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

    fn fit(&mut self, window: &Window, cx: &mut Context<Self>) {
        let wanted = height(self.saved.protocol);
        if self.height == wanted {
            return;
        }
        self.height = wanted;
        let Some(ns) = ns_window(window) else {
            return;
        };
        // AppKit re-enters gpui as it resizes the sheet, so not inside this render.
        cx.spawn(async move |_, _| {
            let mut frame = ns.frame();
            frame.origin.y += frame.size.height - f64::from(wanted);
            frame.size.height = f64::from(wanted);
            ns.setFrame_display_animate(frame, true, true);
        })
        .detach();
    }

    fn field(
        &self,
        id: &'static str,
        text: &str,
        placeholder: impl Into<SharedString>,
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

    fn option_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match self.saved.protocol {
            Protocol::Sftp => Some(
                row(
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
            ),
            Protocol::WebDav => Some(
                div()
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
            ),
            _ => None,
        }
    }
}

impl Render for ServerForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.fit(window, cx);
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
        let connect_using = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(HINT_TOP))
            .child(row("Connect using:", protocols))
            .child(
                div()
                    .flex_shrink_0()
                    .pl(px(LABEL_W + LABEL_GAP))
                    .h(px(hint_height()))
                    .child(
                        native_label("ng.finder.server.hint", protocol.hint())
                            .font_size(tokens::small_system_font_size())
                            .color(SystemColor::SecondaryLabel)
                            .wraps(true),
                    ),
            );
        let server = self.field(
            "ng.finder.server.host",
            &self.saved.host,
            "example.com or 192.168.1.20",
            |saved, text| saved.host = text,
            cx,
        );
        let port = self.field(
            "ng.finder.server.port",
            &self.saved.port,
            protocol.default_port().to_string(),
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
        let user = self
            .field(
                "ng.finder.server.user",
                &self.saved.user,
                "Your login on the server",
                |saved, text| saved.user = text,
                cx,
            )
            .enabled(protocol != Protocol::Nfs);
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
                .gap(px(SECTION_GAP))
                .p(px(EDGE))
                .child(
                    div().flex_shrink_0().child(
                        native_label("ng.finder.server.title", title)
                            .font_size(tokens::system_font_size())
                            .weight(FontWeight::BOLD),
                    ),
                )
                .child(section(
                    "General",
                    [
                        row("Name:", name).into_any_element(),
                        connect_using.into_any_element(),
                    ],
                ))
                .child(section(
                    "Server",
                    [
                        row("Server:", server).into_any_element(),
                        row(
                            "Port:",
                            div()
                                .flex()
                                .child(div().w(px(PORT_W)).h(px(FIELD_H)).child(port)),
                        )
                        .into_any_element(),
                        row(
                            if protocol.shares() {
                                "Share:"
                            } else {
                                "Folder:"
                            },
                            folder,
                        )
                        .into_any_element(),
                    ],
                ))
                .child(section(
                    "Login",
                    [row("User Name:", user).into_any_element()]
                        .into_iter()
                        .chain(self.option_row(cx)),
                ))
                .child(section(
                    "Address",
                    [div().h(px(FIELD_H)).child(address).into_any_element()],
                ))
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
