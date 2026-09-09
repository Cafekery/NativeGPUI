//! A Finder front end: real macOS chrome over the real file system, `NG_QUIT=<frames>` to exit.

mod app;
mod art;
mod chrome;
mod data;
mod dragdrop;
mod frame;
mod icons;
mod info;
mod input;
mod listing;
mod menus;
mod meta;
mod navigate;
mod ops;
mod options;
mod palette;
mod prefs;
mod preview;
mod quicklook;
mod read;
mod search;
mod select;
mod tags;
mod thumbs;
mod ui;
mod view;
mod views;

use std::collections::HashSet;
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
use crate::menus::{Back, CopyNames, Forward, SelectAll, ZoomIn, ZoomOut};

const SAVE_EVERY: Duration = Duration::from_secs(180);
/// What a quit can spend on a save already under way; gpui gives shutdown handlers about 200 ms.
const QUIT_WAIT: Duration = Duration::from_millis(150);

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        tokens::init(cx);
        install_menus(cx);
        ops::undo::install_undo_redo(cx);
        menus::install(cx);
        // After `install_menus`, never before: the more specific binding has to be the later one.
        cx.bind_keys([
            KeyBinding::new("cmd-a", SelectAll, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-c", CopyNames, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-x", menus::Cut, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-v", menus::Paste, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-alt-v", menus::MoveHere, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-[", Back, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-]", Forward, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-=", ZoomIn, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-shift-=", ZoomIn, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd--", ZoomOut, Some(LIST_CONTEXT)),
        ]);
        // The preview pane is a child window; the app ends with the Finder window, not the pane.
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
        // The snapshot is hundreds of megabytes, the tags are a query against Finder's key value
        // store and the volume scan asks every disk for its purgeable space: none of it belongs in
        // front of the window, so all three land while it opens.
        let warm = cx.background_executor().clone();
        warm.spawn(async {
            let _ = index();
        })
        .detach();
        warm.spawn(async {
            tags::all();
            tags::tags();
        })
        .detach();
        cx.spawn(async move |cx| {
            warm.spawn(async { data::refresh_volumes() }).await;
            cx.update(|cx| cx.refresh_windows());
        })
        .detach();
        open_finder_window(intern_path(&home()), 0, cx);
        crawl(cx);
        cx.activate(true);
    });
}

fn home() -> PathBuf {
    volumes::standard(volumes::Standard::Home).unwrap_or_else(|| PathBuf::from("/"))
}

/// Where the demo keeps what it writes for itself: the index, and the sidebar the user arranged.
pub(crate) fn store() -> PathBuf {
    let store = home().join("Library/Application Support/FinderDemo");
    std::fs::create_dir_all(&store).ok();
    store
}

pub(crate) fn index() -> &'static Arc<Index> {
    static INDEX: OnceLock<Arc<Index>> = OnceLock::new();
    INDEX.get_or_init(|| Index::open(&store().join("index.bin")))
}

/// The index catches up from the FSEvents log rather than walking again, and saves as it goes.
fn crawl(cx: &mut App) {
    let mut roots = vec![home(), PathBuf::from("/Applications"), PathBuf::from("/")];
    roots.extend(
        volumes::mounted()
            .into_iter()
            .map(|volume| volume.path)
            .filter(|path| path != Path::new("/")),
    );
    let executor = cx.background_executor().clone();
    // `NG_NO_CRAWL=1` keeps the index idle, to tell the crawl's disk walk apart from the view's.
    if env::var_os("NG_NO_CRAWL").is_none() {
        executor
            .spawn(async move { index().sync(roots, |_| {}) })
            .detach();
    }
    // The TCC-gated folders are left out of the walk; Desktop, Documents and Downloads join once
    // a visit has listed them. The media homes never do: their service asks again every launch.
    let launched = std::time::SystemTime::now();
    let follow = cx.background_executor().clone();
    cx.background_executor()
        .spawn(async move {
            let mut joined: HashSet<PathBuf> = HashSet::new();
            loop {
                follow.timer(Duration::from_secs(20)).await;
                let index = index();
                if !index.stats().crawled_at.is_some_and(|at| at > launched) {
                    continue;
                }
                let granted: Vec<PathBuf> = ["Desktop", "Documents", "Downloads"]
                    .iter()
                    .map(|name| home().join(name))
                    .filter(|dir| !joined.contains(dir) && index.children(dir).is_some())
                    .collect();
                if !granted.is_empty() {
                    joined.extend(granted.iter().cloned());
                    index.crawl(granted, |_| {});
                }
            }
        })
        .detach();
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

fn open_finder_window(start: &'static str, extra: usize, cx: &mut App) -> WindowHandle<FinderDemo> {
    let step = px(extra as f32 * 30.);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(160.) + step, px(120.) + step),
                size: size(px(1200.), px(772.)),
            })),
            tabbing_identifier: Some("ng.finder".into()),
            ..window_options()
        },
        |window, cx| {
            if extra == 0 {
                remember_frame(window, "ng.finder.window");
            }
            native_gpui::window_tabs(window, menus::NewTab, cx);
            cx.new(|cx| {
                cx.observe_global::<SystemTheme>(|_, cx| cx.notify())
                    .detach();
                FinderDemo::new(start, cx)
            })
        },
    )
    .expect("the Finder window opens")
}
