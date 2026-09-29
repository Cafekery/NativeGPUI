use std::cell::RefCell;
use std::ops::Range;

use gpui::{
    AnyElement, AnyWindowHandle, App, AsyncApp, Bounds, ClickEvent, Context, FontWeight, Render,
    WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, div, point, prelude::*, px,
    size, uniform_list,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{
    ButtonBezel, MenuEntry, native_button, native_combo_box, native_label, native_popup,
    native_root, native_rule, native_spinner, native_symbol, window_options,
};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSHelpManager, NSModalResponse, NSWindowButton,
    NSWindowCollectionBehavior, NSWindowTitleVisibility,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::app::FinderDemo;
use crate::data::intern_path;
use crate::frame::ns_window;
use crate::prefs;
use crate::ui::click_kind;

const DEFAULT_W: f32 = 414.;
const DEFAULT_H: f32 = 227.;
const MIN_W: f32 = 355.;
const MIN_H: f32 = 234.;
const EDGE: f32 = 19.;
const FIELD_Y: f32 = 51.;
const LABEL_Y: f32 = 87.;
const LIST_Y: f32 = 110.;
const LIST_BOTTOM: f32 = 52.;
const ROW_H: f32 = 19.;
const ICON_X: f32 = 16.;
const TEXT_X: f32 = 36.;
const BAR_BOTTOM: f32 = 19.;
const BAR_H: f32 = 26.;
const SQUARE: f32 = 24.;
const HELP_X: f32 = 118.;
const SPINNER_RIGHT: f32 = 178.;
const BROWSE_RIGHT: f32 = 107.;
const BROWSE_W: f32 = 70.;
const CONNECT_W: f32 = 77.;
const RECENTS: usize = 10;
const EINVAL: i32 = 22;
const PARAM_ERR: i32 = -50;
const HELP_ANCHOR: &str = "mchlp3015";

thread_local! {
    static OPEN: RefCell<Option<WindowHandle<Connect>>> = const { RefCell::new(None) };
}

pub(crate) struct Connect {
    demo: WeakEntity<FinderDemo>,
    host: AnyWindowHandle,
    address: String,
    favorites: Vec<String>,
    recents: Vec<String>,
    picked: Option<usize>,
    connecting: bool,
    fresh: bool,
}

pub(crate) fn show(demo: WeakEntity<FinderDemo>, host: AnyWindowHandle, cx: &mut App) {
    let raised = open().is_some_and(|open| {
        open.update(cx, |connect, window, _| {
            (connect.demo, connect.host) = (demo.clone(), host);
            window.activate_window();
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
            cx.new(|_| Connect {
                demo,
                host,
                address: prefs::connect_last().unwrap_or_default(),
                favorites: prefs::favorite_servers(),
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
    OPEN.with_borrow_mut(|open| *open = Some(opened));
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

fn open() -> Option<WindowHandle<Connect>> {
    OPEN.with_borrow(|open| *open)
}

fn set_connecting(connecting: bool, cx: &mut AsyncApp) {
    if let Some(open) = open() {
        open.update(cx, |connect, _, cx| {
            connect.connecting = connecting;
            cx.notify();
        })
        .ok();
    }
}

fn close(cx: &mut AsyncApp) {
    if let Some(open) = OPEN.with_borrow_mut(Option::take) {
        open.update(cx, |_, window, _| window.remove_window()).ok();
    }
}

fn complain(address: &str, code: i32, cx: &mut AsyncApp) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&format!(
        "Unable to connect to the server at the address you specified ({address})."
    )));
    alert.setInformativeText(&NSString::from_str(&format!(
        "Try again later, or try connecting to a different server (error code {code})."
    )));
    let sheet = open().and_then(|open| open.update(cx, |_, window, _| ns_window(window)).ok());
    match sheet.flatten() {
        Some(window) => alert.beginSheetModalForWindow_completionHandler(&window, None),
        None => {
            alert.runModal();
        }
    }
}

fn remember(address: &str) {
    prefs::set_connect_last(address);
    let mut recents = prefs::recent_servers();
    recents.retain(|seen| seen != address);
    recents.insert(0, address.to_string());
    recents.truncate(RECENTS);
    prefs::set_recent_servers(&recents.iter().map(String::as_str).collect::<Vec<_>>());
}

fn open_help() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    NSHelpManager::sharedHelpManager(mtm).openHelpAnchor_inBook(
        &NSString::from_str(HELP_ANCHOR),
        Some(&NSString::from_str("com.apple.machelp")),
    );
}

impl FinderDemo {
    pub(crate) fn connect(&mut self, address: String, window: &mut Window, cx: &mut Context<Self>) {
        if address.is_empty() {
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
                Err(code @ (EINVAL | PARAM_ERR)) => complain(&address, code, cx),
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

    fn store_favorites(&mut self, cx: &mut Context<Self>) {
        prefs::set_favorite_servers(
            &self
                .favorites
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        cx.notify();
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        self.favorites.push(self.address.trim().to_string());
        self.picked = Some(self.favorites.len() - 1);
        self.store_favorites(cx);
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        let Some(at) = self.picked.take() else {
            return;
        };
        self.favorites.remove(at);
        self.store_favorites(cx);
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
        let address = self.favorites[at].clone();
        let picked = self.picked == Some(at);
        div()
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
                    .flex()
                    .items_center()
                    .child(native_symbol(
                        ("ng.finder.connect.icon", at),
                        "network",
                        13.,
                        FontWeight::NORMAL,
                    )),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(TEXT_X))
                    .right_0()
                    .flex()
                    .items_center()
                    .child(
                        native_label(("ng.finder.connect.url", at), &address)
                            .font_size(tokens::system_font_size())
                            .color(if picked {
                                SystemColor::AlternateSelectedControlText
                            } else {
                                SystemColor::Label
                            }),
                    ),
            )
            .on_click(cx.listener(move |connect, event: &ClickEvent, _, cx| {
                connect.picked = Some(at);
                connect.address.clone_from(&address);
                cx.notify();
                if click_kind(event).2 {
                    connect.connect(cx);
                }
            }))
            .into_any_element()
    }
}

impl Render for Connect {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let address = self.address.trim().to_string();
        let can_add = !address.is_empty() && !self.favorites.contains(&address);
        let mut field = native_combo_box("ng.finder.connect.address", &self.address)
            .stretch()
            .placeholder("Server Address")
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
        let rows = uniform_list(
            "ng.finder.connect.rows",
            self.favorites.len(),
            cx.processor(|connect, range: Range<usize>, _, cx| {
                range.map(|at| connect.row(at, cx)).collect::<Vec<_>>()
            }),
        )
        .size_full();
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
                        native_label("ng.finder.connect.heading", "Favorite Servers:")
                            .font_size(tokens::small_system_font_size()),
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
                        .child(rows),
                )
                .child(
                    bar(EDGE + 2.).child(
                        square("ng.finder.connect.add", "plus", can_add)
                            .on_click(cx.listener(|connect, _, _, cx| connect.add(cx))),
                    ),
                )
                .child(
                    bar(EDGE + 2. + SQUARE).child(
                        square("ng.finder.connect.remove", "minus", self.picked.is_some())
                            .on_click(cx.listener(|connect, _, _, cx| connect.remove(cx))),
                    ),
                )
                .child(bar(EDGE + 2. + 2. * SQUARE).h(px(SQUARE)).child(action))
                .child(
                    bar(HELP_X).child(
                        native_button("ng.finder.connect.help", "")
                            .bezel_style(ButtonBezel::HelpButton)
                            .on_click(|_, _, _| open_help()),
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
                                    crate::open_finder_window(crate::locations::NETWORK, 1, cx);
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
