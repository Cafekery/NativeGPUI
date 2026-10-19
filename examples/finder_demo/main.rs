mod about;
mod airdrop;
mod app;
mod art;
mod chrome;
mod clipboard_window;
mod custom;
mod customize;
mod data;
mod dock;
mod dragdrop;
mod elastic;
mod file_viewer;
mod fit;
mod fold;
mod form;
mod frame;
mod fsearch;
mod icon_well;
mod icons;
mod info;
mod input;
mod inspector;
mod item_info;
mod keep;
mod listing;
mod locations;
mod menus;
mod meta;
mod navigate;
mod openwith;
mod ops;
mod options;
mod palette;
mod pictures;
mod prefs;
mod preview;
mod progress_window;
mod quick_actions;
mod quicklook;
mod read;
mod remote;
mod search;
mod search_field;
mod select;
mod servers;
mod settings;
mod signal;
mod smart;
mod sound;
mod tag_editor;
mod tags;
mod terms;
mod thumbs;
mod trim;
mod ui;
mod view;
mod views;

use std::path::PathBuf;

use finder_fs::volumes;
use gpui::{
    App, Bounds, KeyBinding, WindowBounds, WindowHandle, WindowOptions, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemTheme};
use native_gpui::{install_menus, remember_frame, save_frame, window_options};
use objc2_foundation::{NSProcessInfo, NSString};

use crate::app::{FinderDemo, LIST_CONTEXT};
use crate::menus::{Back, Forward, ZoomIn, ZoomOut};

const KINDS_WARM: usize = 400;
const RETIRED_INDEX: [&str; 3] = ["index.bin", "index.lock", "index.tmp"];
pub(crate) const WINDOW_W: f32 = 1200.;
const FRAME: &str = "ng.finder.window";

fn main() {
    if std::env::var_os(finder_sftp::askpass::ATTEMPT).is_some() {
        std::process::exit(remote::askpass(
            &std::env::args().nth(1).unwrap_or_default(),
        ));
    }
    prefs::share_folder_looks();
    settings::apply_names();
    app::EARLY_SIDEBAR.set(Some(app::ask_sidebar()));
    std::thread::spawn(|| finder_fs::words::say(""));
    native_gpui::set_wording(|text| match finder_fs::words::say(text) {
        std::borrow::Cow::Owned(said) => Some(said),
        std::borrow::Cow::Borrowed(_) => None,
    });
    keep::skip_appkit_restore();
    chrome::toolbar::concentric_corners();
    let application = gpui_platform::application();
    application.on_open_urls(file_viewer::arrived);
    file_viewer::take_open_documents();
    application.on_reopen(|cx| {
        if navigate::browser_windows(cx).is_empty() {
            menus::open_first_window(cx);
        }
    });
    application.run(|cx: &mut App| {
        tokens::init(cx);
        install_menus(cx);
        menus::install(cx);
        // After `install_menus`, never before: the more specific binding has to be the later one.
        cx.bind_keys([
            KeyBinding::new("cmd-[", Back, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-]", Forward, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-=", ZoomIn, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-shift-=", ZoomIn, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd--", ZoomOut, Some(LIST_CONTEXT)),
        ]);
        navigate::arrive_on_keys(cx);
        // AppKit ends a windowless app that allows automatic termination.
        NSProcessInfo::processInfo().disableAutomaticTermination(&NSString::from_str(
            "Found'er keeps running without windows",
        ));
        let warm = cx.background_executor().clone();
        warm.spawn(async { fsearch::start() }).detach();
        warm.spawn(async {
            for name in RETIRED_INDEX {
                std::fs::remove_file(store().join(name)).ok();
            }
        })
        .detach();
        warm.spawn(async { icons::ask_app_management(icons::installed_apps()) })
            .detach();
        warm.spawn(async { file_viewer::keep_guard() }).detach();
        warm.spawn(async {
            tags::all();
            tags::tags();
        })
        .detach();
        warm.spawn(async {
            let table = store().join("kinds-2.tsv");
            finder_fs::kind::load_kinds(&table);
            fsearch::wait_ready();
            for path in fsearch::extensions(KINDS_WARM) {
                finder_fs::kind::kind_of_path(&path);
            }
            finder_fs::kind::save_kinds(&table);
        })
        .detach();
        cx.spawn(async move |cx| {
            warm.spawn(async { data::refresh_volumes() }).await;
            cx.update(|cx| cx.refresh_windows());
        })
        .detach();
        let restored = keep::reopen(cx);
        file_viewer::follow(restored, cx);
        keep::follow_quit(cx);
        reread_once_indexed(cx);
        thumbs::follow_thumbnails(cx);
        pictures::follow_pictures(cx);
        preview::follow_stacks(cx);
        preview::keep_a_key_window();
        search::follow_search_fields(cx);
        data::follow_volumes(cx);
        data::follow_clock(cx);
        locations::follow_network(cx);
        cx.activate(true);
    });
}

fn home() -> PathBuf {
    volumes::standard(volumes::Standard::Home).unwrap_or_else(|| PathBuf::from("/"))
}

pub(crate) fn store() -> PathBuf {
    let store = home().join("Library/Application Support/Founder");
    std::fs::create_dir_all(&store).ok();
    store
}

fn reread_once_indexed(cx: &mut App) {
    let indexed = cx
        .background_executor()
        .spawn(async { fsearch::wait_ready() });
    cx.spawn(async move |cx| {
        if indexed.await {
            cx.update(|cx| settings::each_window(cx, |this, cx| this.reread_folders(cx)));
        }
    })
    .detach();
}

pub(crate) fn open_finder_window(start: &'static str, cx: &mut App) -> WindowHandle<FinderDemo> {
    let bounds = WindowBounds::Windowed(Bounds {
        origin: point(px(160.), px(120.)),
        size: size(px(WINDOW_W), px(772.)),
    });
    finder_window(start, bounds, true, true, cx)
}

/// Hidden until it joins its tab group, at the frame that group already has.
fn open_finder_tab(
    start: &'static str,
    bounds: WindowBounds,
    cx: &mut App,
) -> WindowHandle<FinderDemo> {
    finder_window(start, bounds, false, false, cx)
}

fn finder_window(
    start: &'static str,
    bounds: WindowBounds,
    remember: bool,
    show: bool,
    cx: &mut App,
) -> WindowHandle<FinderDemo> {
    cx.open_window(
        WindowOptions {
            window_bounds: Some(bounds),
            show,
            tabbing_identifier: Some("ng.finder".into()),
            ..window_options()
        },
        |window, cx| {
            if remember {
                remember_frame(window, FRAME);
            } else {
                save_frame(window, FRAME);
            }
            native_gpui::window_tabs(window, menus::NewTab, cx);
            data::ask_servers_once_drawn(window);
            cx.new(|cx| {
                cx.observe_global::<SystemTheme>(|_, cx| cx.notify())
                    .detach();
                // The app-wide Edit menu names the undo of whichever window is in front.
                cx.observe_window_activation(window, |this: &mut FinderDemo, window, cx| {
                    if window.is_window_active() {
                        this.reread_unwatched(cx);
                        cx.notify();
                        cx.background_executor()
                            .spawn(async { icons::ask_app_management(icons::installed_apps()) })
                            .detach();
                    }
                })
                .detach();
                preview::close_with_finder(cx);
                let mut demo = FinderDemo::new(start, cx);
                demo.icons.sized = remember;
                demo
            })
        },
    )
    .expect("the Finder window opens")
}
