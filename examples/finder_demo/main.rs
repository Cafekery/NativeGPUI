//! A Finder front end: real macOS chrome over the real file system, `NG_QUIT=<frames>` to exit.

mod app;
mod art;
mod clipboard;
mod column;
mod commands;
mod conflicts;
mod data;
mod dragdrop;
mod frame;
mod gallery;
mod header;
mod icon;
mod info;
mod input;
mod list;
mod listing;
mod menus;
mod navigate;
mod ops;
mod options;
mod overlay;
mod palette;
mod path_bar;
mod prefs;
mod preview;
mod quicklook;
mod read;
mod scope;
mod search;
mod select;
mod sidebar;
mod status_bar;
mod tags;
mod thumbs;
mod toolbar;
mod ui;
mod view;

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
use crate::commands::{Back, CopyNames, Forward, SelectAll, ZoomIn, ZoomOut};
use crate::data::intern_path;

const SAVE_EVERY: Duration = Duration::from_secs(180);
fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        tokens::init(cx);
        install_menus(cx);
        conflicts::install_undo_redo(cx);
        commands::install(cx);
        // After `install_menus`, never before: the more specific binding has to be the later one.
        cx.bind_keys([
            KeyBinding::new("cmd-a", SelectAll, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-c", CopyNames, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-x", commands::Cut, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-v", commands::Paste, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-alt-v", commands::MoveHere, Some(LIST_CONTEXT)),
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
        data::refresh_volumes();
        open_finder_window(intern_path(&home()), 0, cx);
        crawl(cx);
        cx.activate(true);
    });
}

/// The user's home folder, where the demo opens.
fn home() -> PathBuf {
    volumes::standard(volumes::Standard::Home).unwrap_or_else(|| PathBuf::from("/"))
}

/// Where the demo keeps what it writes for itself: the index, and the sidebar the user arranged.
pub(crate) fn store() -> PathBuf {
    let store = home().join("Library/Application Support/FinderDemo");
    std::fs::create_dir_all(&store).ok();
    store
}

/// The whole-Mac index, opened once for the app and shared by every window.
pub(crate) fn index() -> &'static Arc<Index> {
    static INDEX: OnceLock<Arc<Index>> = OnceLock::new();
    INDEX.get_or_init(|| Index::open(&store().join("index.bin")))
}

/// The index crawls from launch and saves as it goes, so the next launch starts warm.
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
            .spawn(async move { index().crawl(roots, |_| {}) })
            .detach();
    }
    // The prompt-gated folders are left out of the walk. Once it is over, Desktop, Documents and
    // Downloads join if a visit has listed them: that grant is durable. The media homes never
    // do — a snapshot listing is no grant, and their media-library service asks again per launch.
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
    cx.on_app_quit(|_| async { index().save() }).detach();
}

/// A tag listing's title: Finder names the window after the tag, or after all of them.
fn tag_title(id: &str) -> String {
    match id.strip_prefix("tag:").unwrap_or_default() {
        "" => "All Tags".to_string(),
        name => name.to_string(),
    }
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
            native_gpui::window_tabs(window, commands::NewTab, cx);
            cx.new(|cx| {
                cx.observe_global::<SystemTheme>(|_, cx| cx.notify())
                    .detach();
                FinderDemo::new(start, cx)
            })
        },
    )
    .expect("the Finder window opens")
}
