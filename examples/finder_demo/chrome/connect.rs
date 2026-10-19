use std::cell::RefCell;
use std::ops::Range;

use gpui::{
    AnyElement, AnyWindowHandle, App, AsyncApp, Bounds, ClickEvent, Context, FontWeight, Render,
    WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, div, point, prelude::*, px,
    size, uniform_list,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{
    ButtonBezel, ControlSize, LabelAlign, MenuEntry, native_button, native_context_menu,
    native_label, native_popup, native_root, native_spinner, native_symbol, native_text_field,
    titlebar_band, window_options,
};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSModalResponse, NSWindowButton, NSWindowCollectionBehavior,
    NSWindowTitleVisibility,
};
use objc2_foundation::NSString;

use super::strs;
use super::toolbar::{system_corners, titled_height};
use crate::app::FinderDemo;
use crate::data::intern_path;
use crate::frame::ns_window;
use crate::prefs;
use crate::servers::{self, Protocol, Saved};
use crate::ui::click_kind;

const WIDTH: f32 = 520.;
const HEIGHT: f32 = 430.;
const EDGE: f32 = 20.;
const GAP: f32 = 8.;
const RADIUS: f32 = 10.;
const HEADING: f32 = 15.;
const RECENTS_W: f32 = 52.;
const INSET: f32 = 6.;
const ROW_H: f32 = 48.;
const ROW_R: f32 = 6.;
const ROW_PAD: f32 = 10.;
const ROW_GAP: f32 = 10.;
const BADGE: f32 = 28.;
const BADGE_R: f32 = 7.;
const ICON: f32 = 14.;
const SQUARE: f32 = 24.;
const ACTIONS_W: f32 = 28.;
const TEXT_X: f32 = ROW_PAD + BADGE + ROW_GAP;
const TEXT_RIGHT: f32 = ROW_PAD + ACTIONS_W + ROW_GAP;
const ADD_W: f32 = 96.;
const BROWSE_W: f32 = 76.;
const CONNECT_W: f32 = 84.;
const RECENTS: usize = 10;
const EINVAL: i32 = 22;
const PARAM_ERR: i32 = -50;
const HELP_ANCHOR: &str = "mchlp3015";
const EMPTY: &str = "Type an address above, or add a server to keep it here.";

thread_local! {
    static OPEN: RefCell<Option<WindowHandle<Connect>>> = const { RefCell::new(None) };
}

pub(crate) struct Connect {
    demo: WeakEntity<FinderDemo>,
    host: AnyWindowHandle,
    address: String,
    saved: Vec<Saved>,
    recents: Vec<String>,
    picked: Option<usize>,
    connecting: bool,
    fresh: bool,
}

pub(crate) fn show(demo: WeakEntity<FinderDemo>, host: AnyWindowHandle, cx: &mut App) {
    let raised = open().is_some_and(|open| {
        open.update(cx, |connect, window, cx| {
            (connect.demo, connect.host) = (demo.clone(), host);
            window.activate_window();
            cx.notify();
        })
        .is_ok()
    });
    if raised {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let least = size(px(WIDTH), titled_height(HEIGHT, mtm));
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: least,
            })),
            show: false,
            is_minimizable: true,
            window_min_size: Some(least),
            ..window_options()
        },
        |window, cx| {
            window.set_window_title("Connect to Server");
            system_corners(window);
            cx.new(|_| Connect {
                demo,
                host,
                address: prefs::connect_last().unwrap_or_default(),
                saved: servers::saved(),
                recents: prefs::recent_servers(),
                picked: None,
                connecting: false,
                fresh: true,
            })
        },
    );
    let Ok(opened) = opened else {
        return;
    };
    OPEN.set(Some(opened));
    let Ok(Some(ns)) = opened.update(cx, |_, window, _| ns_window(window)) else {
        return;
    };
    // AppKit re-enters gpui as it moves and orders in a window, so not inside this update.
    cx.spawn(async move |_| {
        let name = NSString::from_str(prefs::CONNECT_WINDOW);
        if !ns.setFrameUsingName(&name) {
            ns.center();
        }
        ns.setFrameAutosaveName(&name);
        ns.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        ns.setCollectionBehavior(
            ns.collectionBehavior() | NSWindowCollectionBehavior::FullScreenNone,
        );
        if let Some(zoom) = ns.standardWindowButton(NSWindowButton::ZoomButton) {
            zoom.setEnabled(false);
        }
        ns.makeKeyAndOrderFront(None);
    })
    .detach();
}

pub(crate) fn open() -> Option<WindowHandle<Connect>> {
    OPEN.with_borrow(|open| *open)
}

pub(crate) fn set_connecting(connecting: bool, cx: &mut AsyncApp) {
    if let Some(open) = open() {
        open.update(cx, |connect, _, cx| {
            connect.connecting = connecting;
            cx.notify();
        })
        .ok();
    }
}

pub(crate) fn close(cx: &mut AsyncApp) {
    if let Some(open) = OPEN.with_borrow_mut(Option::take) {
        open.update(cx, |_, window, _| window.remove_window()).ok();
    }
}

pub(crate) fn complain(message: &str, detail: &str, cx: &mut AsyncApp) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(message));
    alert.setInformativeText(&NSString::from_str(detail));
    let sheet = open().and_then(|open| open.update(cx, |_, window, _| ns_window(window)).ok());
    match sheet.flatten() {
        Some(window) => alert.beginSheetModalForWindow_completionHandler(&window, None),
        None => {
            alert.runModal();
        }
    }
}

pub(crate) fn remember(address: &str) {
    prefs::set_connect_last(address);
    let mut recents = prefs::recent_servers();
    recents.retain(|seen| seen != address);
    recents.insert(0, address.to_string());
    recents.truncate(RECENTS);
    prefs::set_recent_servers(&strs(&recents));
}

impl FinderDemo {
    pub(crate) fn connect(&mut self, address: String, window: &mut Window, cx: &mut Context<Self>) {
        if address.is_empty() {
            return;
        }
        if let Some(mut sftp) = crate::remote::address(&address) {
            sftp.identity = crate::servers::identity_for(&address);
            self.connect_sftp(sftp, address, window, cx);
            return;
        }
        // Finder reads a bare host as an SMB server.
        let address = match address.contains("://") {
            true => address,
            false => format!("smb://{address}"),
        };
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            set_connecting(true, cx);
            let wanted = address.clone();
            let mounted = cx
                .background_executor()
                .spawn(async move { finder_fs::volumes::mount(&wanted) })
                .await;
            set_connecting(false, cx);
            match mounted {
                Ok(point) => {
                    remember(&address);
                    close(cx);
                    cx.update_window(handle, |_, window, cx| {
                        this.update(cx, |this, cx| this.go(intern_path(&point), window, cx))
                    })
                    .ok();
                }
                // NetFS refuses a malformed address without a word; NetAuth reports every other failure.
                Err(code @ (EINVAL | PARAM_ERR)) => complain(
                    &format!(
                        "Unable to connect to the server at the address you specified ({address})."
                    ),
                    &format!(
                        "Try again later, or try connecting to a different server (error code {code})."
                    ),
                    cx,
                ),
                Err(_) => {}
            }
        })
        .detach();
    }
}

impl Connect {
    fn connect(&mut self, cx: &mut Context<Self>) {
        let address = self.address.trim().to_string();
        let (demo, host) = (self.demo.clone(), self.host);
        host.update(cx, |_, window, cx| {
            demo.update(cx, |demo, cx| demo.connect(address, window, cx))
        })
        .ok();
    }

    pub(crate) fn store(&mut self, at: Option<usize>, saved: Saved, cx: &mut Context<Self>) {
        self.address = saved.address();
        match at {
            Some(at) if at < self.saved.len() => self.saved[at] = saved,
            _ => self.saved.push(saved),
        }
        self.picked = Some(at.unwrap_or(self.saved.len() - 1));
        servers::store(&self.saved);
        cx.notify();
    }

    fn edit(&mut self, at: Option<usize>, window: &Window, cx: &mut Context<Self>) {
        let saved = match at {
            Some(at) => self.saved[at].clone(),
            None => Saved::parse(&self.address).unwrap_or_else(|| Saved::new(Protocol::Smb)),
        };
        super::server_form::open(cx.entity(), window.window_handle(), at, saved, cx);
    }

    fn remove(&mut self, at: usize, cx: &mut Context<Self>) {
        self.saved.remove(at);
        self.picked = None;
        servers::store(&self.saved);
        cx.notify();
    }

    fn clear_recents(&mut self, ask: bool, window: &Window, cx: &mut Context<Self>) {
        let clear = |connect: &mut Connect, cx: &mut Context<Connect>| {
            prefs::set_recent_servers(&[]);
            connect.recents.clear();
            cx.notify();
        };
        let (Some(mtm), Some(ns)) = (MainThreadMarker::new(), ns_window(window)) else {
            return;
        };
        if !ask {
            clear(self, cx);
            return;
        }
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(
            "Are you sure you want to clear the list of recent servers?",
        ));
        alert.setInformativeText(&NSString::from_str("You can\u{2019}t undo this action."));
        alert.addButtonWithTitle(&NSString::from_str("Clear"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let this = cx.entity().downgrade();
        let async_cx = cx.to_async();
        let handler = block2::RcBlock::new(move |response: NSModalResponse| {
            if response == NSAlertFirstButtonReturn {
                this.update(&mut async_cx.clone(), |connect, cx| clear(connect, cx))
                    .ok();
            }
        });
        alert.beginSheetModalForWindow_completionHandler(&ns, Some(&handler));
    }

    fn act(&mut self, at: usize, pick: usize, window: &Window, cx: &mut Context<Self>) {
        self.picked = Some(at);
        self.address = self.saved[at].address();
        match pick {
            0 => self.connect(cx),
            1 => self.edit(Some(at), window, cx),
            _ => self.remove(at, cx),
        }
    }

    fn row(&self, at: usize, cx: &mut Context<Self>) -> AnyElement {
        let saved = &self.saved[at];
        let (address, picked) = (saved.address(), self.picked == Some(at));
        let color = |plain| {
            if picked {
                SystemColor::AlternateSelectedControlText
            } else {
                plain
            }
        };
        let actions = native_popup(("ng.finder.connect.actions", at), 0)
            .pull_down(true)
            .bordered(false)
            .entries([
                MenuEntry::item("").symbol("ellipsis.circle"),
                MenuEntry::item("Connect"),
                MenuEntry::item("Edit\u{2026}"),
                MenuEntry::separator(),
                MenuEntry::item("Remove"),
            ])
            .on_change(cx.listener(move |connect, pick: &usize, window, cx| {
                if *pick > 0 {
                    connect.act(at, pick - 1, window, cx);
                }
            }));
        let divided = at + 1 < self.saved.len() && !picked && self.picked != Some(at + 1);
        let row = div()
            .id(("ng.finder.connect.row", at))
            .relative()
            .h(px(ROW_H))
            .rounded(px(ROW_R))
            .when(picked, |row| {
                row.bg(system_color(SystemColor::SelectedContentBackground))
            })
            .when(!picked, |row| {
                row.hover(|row| row.bg(system_color(SystemColor::QuinarySystemFill)))
            })
            .child(
                div()
                    .absolute()
                    .left(px(ROW_PAD))
                    .top(px((ROW_H - BADGE) / 2.))
                    .size(px(BADGE))
                    .rounded(px(BADGE_R))
                    .bg(system_color(saved.protocol.tint()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        native_symbol(
                            ("ng.finder.connect.icon", at),
                            saved.protocol.symbol(),
                            ICON,
                            FontWeight::SEMIBOLD,
                        )
                        .ink(gpui::white()),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(TEXT_X))
                    .right(px(TEXT_RIGHT))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .child(
                        native_label(("ng.finder.connect.name", at), saved.label())
                            .font_size(tokens::system_font_size())
                            .weight(FontWeight::MEDIUM)
                            .color(color(SystemColor::Label))
                            .truncates(true),
                    )
                    .child(
                        native_label(("ng.finder.connect.url", at), &address)
                            .font_size(tokens::small_system_font_size())
                            .color(color(SystemColor::SecondaryLabel))
                            .truncates(true),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .right(px(ROW_PAD))
                    .top(px((ROW_H - SQUARE) / 2.))
                    .w(px(ACTIONS_W))
                    .h(px(SQUARE))
                    .child(actions),
            )
            .when(divided, |row| {
                row.child(
                    div()
                        .absolute()
                        .left(px(TEXT_X))
                        .right(px(ROW_PAD))
                        .bottom_0()
                        .h(px(1.))
                        .bg(system_color(SystemColor::Separator)),
                )
            })
            .on_click(cx.listener(move |connect, event: &ClickEvent, _, cx| {
                connect.picked = Some(at);
                connect.address.clone_from(&address);
                cx.notify();
                if click_kind(event).2 {
                    connect.connect(cx);
                }
            }));
        native_context_menu(("ng.finder.connect.menu", at), row)
            .entries([
                MenuEntry::item("Connect"),
                MenuEntry::item("Edit\u{2026}"),
                MenuEntry::separator(),
                MenuEntry::item("Remove"),
            ])
            .on_select(cx.listener(move |connect, pick: &usize, window, cx| {
                connect.act(at, *pick, window, cx)
            }))
            .into_any_element()
    }
}

impl Render for Connect {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let address = self.address.trim().to_string();
        let mut field = native_text_field("ng.finder.connect.address", &self.address)
            .stretch()
            .control_size(ControlSize::Large)
            .placeholder("smb://server/share or sftp://user@host")
            .on_change(cx.listener(|connect, text: &String, _, cx| {
                connect.address.clone_from(text);
                cx.notify();
            }))
            .on_submit(cx.listener(|connect, _, _, cx| connect.connect(cx)))
            .on_cancel(|_, window, _| window.remove_window());
        if std::mem::take(&mut self.fresh) {
            field = field.editing(0..self.address.chars().count());
        }
        let clearing = |entry: MenuEntry| {
            if self.recents.is_empty() {
                entry.disabled()
            } else {
                entry
            }
        };
        let recents = native_popup("ng.finder.connect.recents", 0)
            .pull_down(true)
            .control_size(ControlSize::Large)
            .width(px(RECENTS_W))
            .entries(
                std::iter::once(MenuEntry::item("").symbol("clock.arrow.circlepath"))
                    .chain(
                        self.recents
                            .iter()
                            .map(|recent| MenuEntry::item(recent.clone())),
                    )
                    .chain([
                        MenuEntry::separator(),
                        clearing(MenuEntry::item("Clear Recent Servers\u{2026}")),
                        clearing(MenuEntry::item("Clear Recent Servers").option_alternate()),
                    ]),
            )
            .on_change(cx.listener(|connect, pick: &usize, window, cx| {
                let count = connect.recents.len();
                match *pick {
                    0 => {}
                    pick if pick <= count => {
                        connect.address.clone_from(&connect.recents[pick - 1]);
                        cx.notify();
                    }
                    pick => connect.clear_recents(pick == count + 2, window, cx),
                }
            }));
        let heading = div().flex_none().child(
            native_label("ng.finder.connect.title", "Connect to Server")
                .font_size(px(HEADING))
                .weight(FontWeight::BOLD),
        );
        let bar = div()
            .flex_none()
            .mt(px(GAP))
            .flex()
            .items_center()
            .gap(px(GAP))
            .child(div().flex_1().child(field))
            .child(recents);
        let header = div()
            .flex_none()
            .mt(px(2. * GAP))
            .flex()
            .items_center()
            .child(
                native_label("ng.finder.connect.heading", "Saved Servers")
                    .font_size(tokens::small_system_font_size())
                    .weight(FontWeight::SEMIBOLD)
                    .color(SystemColor::SecondaryLabel),
            )
            .child(div().flex_1())
            .child(
                native_button("ng.finder.connect.add", "Add Server\u{2026}")
                    .bezel_style(ButtonBezel::AccessoryBarAction)
                    .control_size(ControlSize::Small)
                    .width(px(ADD_W))
                    .on_click(cx.listener(|connect, _, window, cx| connect.edit(None, window, cx))),
            );
        let list: AnyElement = if self.saved.is_empty() {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .child(
                    native_symbol(
                        "ng.finder.connect.none",
                        "server.rack",
                        28.,
                        FontWeight::LIGHT,
                    )
                    .tint(SystemColor::TertiaryLabel),
                )
                .child(
                    native_label("ng.finder.connect.empty", "No Saved Servers")
                        .font_size(tokens::system_font_size())
                        .weight(FontWeight::SEMIBOLD)
                        .color(SystemColor::SecondaryLabel)
                        .align(LabelAlign::Center),
                )
                .child(
                    native_label("ng.finder.connect.empty.hint", EMPTY)
                        .font_size(tokens::small_system_font_size())
                        .color(SystemColor::TertiaryLabel)
                        .align(LabelAlign::Center),
                )
                .into_any_element()
        } else {
            uniform_list(
                "ng.finder.connect.rows",
                self.saved.len(),
                cx.processor(|connect, range: Range<usize>, _, cx| {
                    range.map(|at| connect.row(at, cx)).collect::<Vec<_>>()
                }),
            )
            .size_full()
            .into_any_element()
        };
        let card = div()
            .flex_1()
            .min_h_0()
            .mt(px(GAP))
            .p(px(INSET))
            .rounded(px(RADIUS))
            .bg(system_color(SystemColor::QuaternarySystemFill))
            .child(list);
        let footer = div()
            .flex_none()
            .mt(px(2. * GAP))
            .flex()
            .items_center()
            .gap(px(ROW_GAP))
            .child(
                native_button("ng.finder.connect.help", "")
                    .bezel_style(ButtonBezel::HelpButton)
                    .on_click(|_, _, _| crate::menus::open_mac_help(HELP_ANCHOR)),
            )
            .child(div().flex_1())
            .when(self.connecting, |footer| {
                footer.child(native_spinner("ng.finder.connect.spinner"))
            })
            .child(
                native_button("ng.finder.connect.browse", "Browse")
                    .width(px(BROWSE_W))
                    .on_click(|_, _, cx| {
                        crate::open_finder_window(crate::locations::NETWORK, cx);
                    }),
            )
            .child(
                native_button("ng.finder.connect.go", "Connect")
                    .width(px(CONNECT_W))
                    .default_button(true)
                    .enabled(!address.is_empty() && !self.connecting)
                    .on_click(cx.listener(|connect, _, _, cx| connect.connect(cx))),
            );
        native_root(
            div()
                .size_full()
                .bg(system_color(SystemColor::WindowBackground))
                .pt(titlebar_band(window))
                .child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .pt(px(GAP))
                        .px(px(EDGE))
                        .pb(px(EDGE))
                        .child(heading)
                        .child(bar)
                        .child(header)
                        .child(card)
                        .child(footer),
                ),
        )
    }
}
