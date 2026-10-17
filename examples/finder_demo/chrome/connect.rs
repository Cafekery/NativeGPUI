use std::cell::RefCell;
use std::ops::Range;

use gpui::{
    AnyElement, AnyWindowHandle, App, AsyncApp, Bounds, ClickEvent, Context, FontWeight, Render,
    WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, div, point, prelude::*, px,
    size, uniform_list,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{
    ButtonBezel, MenuEntry, native_button, native_combo_box, native_context_menu, native_label,
    native_popup, native_root, native_rule, native_spinner, native_symbol, window_options,
};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSModalResponse, NSWindowButton, NSWindowCollectionBehavior,
    NSWindowTitleVisibility,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use super::strs;
use crate::app::FinderDemo;
use crate::data::intern_path;
use crate::frame::ns_window;
use crate::prefs;
use crate::servers::{self, Protocol, Saved};
use crate::ui::click_kind;

const DEFAULT_W: f32 = 460.;
const DEFAULT_H: f32 = 380.;
const MIN_W: f32 = 400.;
const MIN_H: f32 = 300.;
const EDGE: f32 = 19.;
const FIELD_Y: f32 = 51.;
const LABEL_Y: f32 = 87.;
const LIST_Y: f32 = 110.;
const LIST_BOTTOM: f32 = 52.;
const ROW_H: f32 = 40.;
const ICON: f32 = 18.;
const ICON_X: f32 = 12.;
const TEXT_X: f32 = 42.;
const BAR_BOTTOM: f32 = 19.;
const BAR_H: f32 = 26.;
const SQUARE: f32 = 24.;
const HELP_X: f32 = 142.;
const SPINNER_RIGHT: f32 = 178.;
const BROWSE_RIGHT: f32 = 107.;
const BROWSE_W: f32 = 70.;
const CONNECT_W: f32 = 77.;
const RECENTS: usize = 10;
const EINVAL: i32 = 22;
const PARAM_ERR: i32 = -50;
const HELP_ANCHOR: &str = "mchlp3015";
const EMPTY: &str = "No saved servers yet. Click + to add one, or type an address above.";

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
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(DEFAULT_W), px(DEFAULT_H)),
            })),
            show: false,
            is_minimizable: true,
            window_min_size: Some(size(px(MIN_W), px(MIN_H))),
            ..window_options()
        },
        |window, cx| {
            window.set_window_title("Connect to Server");
            super::toolbar::system_corners(window);
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
            match prefs::finder_connect_frame() {
                Some([x, y, w, h]) => {
                    ns.setFrame_display(NSRect::new(NSPoint::new(x, y), NSSize::new(w, h)), false)
                }
                None => ns.center(),
            }
        }
        ns.setFrameAutosaveName(&name);
        ns.setTitleVisibility(NSWindowTitleVisibility::Visible);
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
        let row = div()
            .id(("ng.finder.connect.row", at))
            .relative()
            .h(px(ROW_H))
            .when(picked, |row| {
                row.child(
                    div().absolute().inset_0().child(
                        native_rule(("ng.finder.connect.pick", at))
                            .color(SystemColor::SelectedContentBackground),
                    ),
                )
            })
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(ICON_X))
                    .w(px(ICON))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(native_symbol(
                        ("ng.finder.connect.icon", at),
                        saved.protocol.symbol(),
                        ICON,
                        FontWeight::MEDIUM,
                    )),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(TEXT_X))
                    .right(px(EDGE))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .child(
                        native_label(("ng.finder.connect.name", at), saved.label())
                            .font_size(tokens::system_font_size())
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
                connect.picked = Some(at);
                connect.address = connect.saved[at].address();
                match pick {
                    0 => connect.connect(cx),
                    1 => connect.edit(Some(at), window, cx),
                    _ => connect.remove(at, cx),
                }
            }))
            .into_any_element()
    }
}

impl Render for Connect {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let address = self.address.trim().to_string();
        let mut field = native_combo_box("ng.finder.connect.address", &self.address)
            .stretch()
            .placeholder("smb://server/share or sftp://user@host")
            .items(self.recents.clone())
            .on_change(cx.listener(|connect, text: &String, _, cx| {
                connect.address.clone_from(text);
                cx.notify();
            }))
            .on_submit(cx.listener(|connect, _, _, cx| connect.connect(cx)))
            .on_cancel(|_, window, _| window.remove_window());
        if std::mem::take(&mut self.fresh) {
            field = field.editing(0..self.address.chars().count());
        }
        let list: AnyElement = if self.saved.is_empty() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    native_label("ng.finder.connect.empty", EMPTY)
                        .font_size(tokens::system_font_size())
                        .color(SystemColor::SecondaryLabel),
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
        let bar = |left: f32| div().absolute().bottom(px(BAR_BOTTOM)).left(px(left));
        let square = |id: &'static str, symbol: &'static str, enabled: bool| {
            native_button(id, "")
                .symbol(symbol)
                .bezel_style(ButtonBezel::SmallSquare)
                .square(SQUARE)
                .enabled(enabled)
        };
        let clearing = |entry: MenuEntry| {
            if self.recents.is_empty() {
                entry.disabled()
            } else {
                entry
            }
        };
        let action = native_popup("ng.finder.connect.action", 0)
            .pull_down(true)
            .bezel_style(ButtonBezel::SmallSquare)
            .entries([
                MenuEntry::item("").symbol("ellipsis.circle"),
                clearing(MenuEntry::item("Clear Recent Servers\u{2026}")),
                clearing(MenuEntry::item("Clear Recent Servers").option_alternate()),
            ])
            .on_change(cx.listener(|connect, picked: &usize, window, cx| {
                connect.clear_recents(*picked == 1, window, cx)
            }));
        native_root(
            div()
                .relative()
                .size_full()
                .bg(system_color(SystemColor::WindowBackground))
                .child(
                    div()
                        .absolute()
                        .top(px(FIELD_Y))
                        .left(px(EDGE))
                        .right(px(EDGE))
                        .child(field),
                )
                .child(
                    div().absolute().top(px(LABEL_Y)).left(px(EDGE)).child(
                        native_label("ng.finder.connect.heading", "Saved Servers")
                            .font_size(tokens::small_system_font_size())
                            .weight(FontWeight::SEMIBOLD)
                            .color(SystemColor::SecondaryLabel),
                    ),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(LIST_Y))
                        .bottom(px(LIST_BOTTOM))
                        .left(px(EDGE + 1.))
                        .right(px(EDGE + 1.))
                        .border_1()
                        .border_color(system_color(SystemColor::Separator))
                        .bg(system_color(SystemColor::ControlBackground))
                        .child(list),
                )
                .child(bar(EDGE + 2.).child(
                    square("ng.finder.connect.add", "plus", true).on_click(
                        cx.listener(|connect, _, window, cx| connect.edit(None, window, cx)),
                    ),
                ))
                .child(bar(EDGE + 2. + SQUARE).child(
                    square("ng.finder.connect.remove", "minus", self.picked.is_some()).on_click(
                        cx.listener(|connect, _, _, cx| {
                            if let Some(at) = connect.picked {
                                connect.remove(at, cx);
                            }
                        }),
                    ),
                ))
                .child(bar(EDGE + 2. + 2. * SQUARE).child(
                    square("ng.finder.connect.edit", "pencil", self.picked.is_some()).on_click(
                        cx.listener(|connect, _, window, cx| {
                            connect.edit(connect.picked, window, cx)
                        }),
                    ),
                ))
                .child(bar(EDGE + 2. + 3. * SQUARE).h(px(SQUARE)).child(action))
                .child(
                    bar(HELP_X).child(
                        native_button("ng.finder.connect.help", "")
                            .bezel_style(ButtonBezel::HelpButton)
                            .on_click(|_, _, _| crate::menus::open_mac_help(HELP_ANCHOR)),
                    ),
                )
                .when(self.connecting, |root| {
                    root.child(
                        div()
                            .absolute()
                            .bottom(px(BAR_BOTTOM + (BAR_H - 16.) / 2.))
                            .right(px(SPINNER_RIGHT))
                            .child(native_spinner("ng.finder.connect.spinner")),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .bottom(px(BAR_BOTTOM))
                        .right(px(BROWSE_RIGHT))
                        .child(
                            native_button("ng.finder.connect.browse", "Browse")
                                .width(px(BROWSE_W))
                                .on_click(|_, _, cx| {
                                    crate::open_finder_window(crate::locations::NETWORK, cx);
                                }),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .bottom(px(BAR_BOTTOM))
                        .right(px(EDGE + 1.))
                        .child(
                            native_button("ng.finder.connect.go", "Connect")
                                .width(px(CONNECT_W))
                                .default_button(true)
                                .enabled(!address.is_empty() && !self.connecting)
                                .on_click(cx.listener(|connect, _, _, cx| connect.connect(cx))),
                        ),
                ),
        )
    }
}
