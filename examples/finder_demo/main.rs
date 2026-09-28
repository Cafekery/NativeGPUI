mod app;
mod art;
mod chrome;
mod clipboard_window;
mod custom;
mod data;
mod dock;
mod dragdrop;
mod elastic;
mod fit;
mod frame;
mod icons;
mod info;
mod input;
mod inspector;
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
mod prefs;
mod preview;
mod progress_window;
mod quick_actions;
mod quicklook;
mod read;
mod search;
mod select;
mod settings;
mod signal;
mod smart;
mod sound;
mod tag_editor;
mod tags;
mod thumbs;
mod trim;
mod ui;
mod view;
mod views;

use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use finder_fs::index::Index;
use finder_fs::volumes;
use gpui::{
    App, Bounds, KeyBinding, WindowBounds, WindowHandle, WindowOptions, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemTheme};
use native_gpui::{install_menus, remember_frame, window_options};

use crate::app::{FinderDemo, LIST_CONTEXT};
use crate::data::intern_path;
use crate::menus::{Back, Forward, ShowViewOptions, ZoomIn, ZoomOut};

/// Each save rewrites the whole snapshot, hundreds of MB; a launch replays events since the last.
const SAVE_EVERY: Duration = Duration::from_secs(3600);

const KINDS_WARM: usize = 400;
/// gpui gives shutdown handlers about 200 ms.
const QUIT_WAIT: Duration = Duration::from_millis(150);
pub(crate) const WINDOW_W: f32 = 1200.;

fn main() {
    settings::apply_names();
    app::EARLY_SIDEBAR.set(Some(app::ask_sidebar()));
    std::thread::spawn(|| finder_fs::words::say(""));
    native_gpui::set_wording(|text| match finder_fs::words::say(text) {
        std::borrow::Cow::Owned(said) => Some(said),
        std::borrow::Cow::Borrowed(_) => None,
    });
    keep::skip_appkit_restore();
    gpui_platform::application().run(|cx: &mut App| {
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
            // The panel is never gpui's active window, so the menu row's ⌘J would not reach it.
            KeyBinding::new("cmd-j", ShowViewOptions, Some(options::CONTEXT)),
        ]);
        navigate::arrive_on_keys(cx);
        // Helper windows such as the preview pane do not keep the app alive.
        cx.on_window_closed(|cx, _| {
            if !cx
                .windows()
                .iter()
                .any(|window| window.downcast::<FinderDemo>().is_some())
            {
                cx.quit();
            }
        })
        .detach();
        let warm = cx.background_executor().clone();
        warm.spawn(async {
            index().warm();
        })
        .detach();
        warm.spawn(async { icons::ask_app_management(icons::installed_apps()) })
            .detach();
        warm.spawn(async {
            tags::all();
            tags::tags();
        })
        .detach();
        warm.spawn(async {
            let table = store().join("kinds-2.tsv");
            finder_fs::kind::load_kinds(&table);
            for path in index().extensions(KINDS_WARM) {
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
        if !keep::reopen(cx) {
            open_finder_window(intern_path(&home()), 0, cx);
        }
        keep::follow_quit(cx);
        crawl(cx);
        thumbs::follow_thumbnails(cx);
        preview::follow_stacks(cx);
        preview::keep_a_key_window();
        search::follow_search_fields(cx);
        data::follow_volumes(cx);
        cx.activate(true);
    });
}

fn home() -> PathBuf {
    volumes::standard(volumes::Standard::Home).unwrap_or_else(|| PathBuf::from("/"))
}

pub(crate) fn store() -> PathBuf {
    let store = home().join("Library/Application Support/FinderDemo");
    std::fs::create_dir_all(&store).ok();
    store
}

pub(crate) fn index() -> &'static Arc<Index> {
    static INDEX: OnceLock<Arc<Index>> = OnceLock::new();
    INDEX.get_or_init(|| Index::open(&store().join("index.bin")))
}

fn crawl(cx: &mut App) {
    if env::var_os("NG_NO_CRAWL").is_none() {
        static FOLLOWING: OnceLock<finder_fs::watch::Watcher> = OnceLock::new();
        // Off the main thread: the first `index()` loads the snapshot, and each local volume is
        // asked for its attributes.
        cx.background_executor()
            .spawn(async move {
                let mut roots = vec![home(), PathBuf::from("/Applications"), PathBuf::from("/")];
                // A network share is sealed below a root already; as a root it would be walked over
                // the wire.
                roots.extend(
                    volumes::mounted()
                        .into_iter()
                        .filter(|volume| !volume.network)
                        .map(|volume| volume.path)
                        .filter(|path| path != Path::new("/")),
                );
                FOLLOWING.get_or_init(|| index().follow(roots.clone()));
                // TCC-gated folders join the crawl only once a visit has listed them, so no prompt is
                // raised.
                index().sync(roots, |progress| {
                    let index = index();
                    if !progress.done {
                        return;
                    }
                    let granted: Vec<PathBuf> = ["Desktop", "Documents", "Downloads"]
                        .iter()
                        .map(|name| home().join(name))
                        .filter(|dir| index.children(dir).is_some())
                        .collect();
                    if !granted.is_empty() {
                        index.crawl(granted, |_| {});
                    }
                })
            })
            .detach();
    }
    let ticker = cx.background_executor().clone();
    cx.background_executor()
        .spawn(async move {
            loop {
                ticker.timer(SAVE_EVERY).await;
                index().save();
            }
        })
        .detach();
    cx.on_app_quit(|_| async { index().settle(QUIT_WAIT) })
        .detach();
}

pub(crate) fn open_finder_window(
    start: &'static str,
    extra: usize,
    cx: &mut App,
) -> WindowHandle<FinderDemo> {
    let step = px(extra as f32 * 30.);
    let bounds = WindowBounds::Windowed(Bounds {
        origin: point(px(160.) + step, px(120.) + step),
        size: size(px(WINDOW_W), px(772.)),
    });
    finder_window(start, bounds, extra == 0, true, cx)
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
                remember_frame(window, "ng.finder.window");
            }
            native_gpui::window_tabs(window, menus::NewTab, cx);
            cx.new(|cx| {
                cx.observe_global::<SystemTheme>(|_, cx| cx.notify())
                    .detach();
                // The Edit menu is app-wide; it names the undo of whichever window is in front.
                cx.observe_window_activation(window, |_: &mut FinderDemo, window, cx| {
                    if window.is_window_active() {
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
