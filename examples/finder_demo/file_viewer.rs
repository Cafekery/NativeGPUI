use std::cell::{Cell, OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use block2::StackBlock;
use finder_fs::kind::is_package_name;
use gpui::{App, AsyncApp};
use objc2_app_kit::{NSApplicationDidFinishLaunchingNotification, NSRunningApplication};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSString, NSURL};

use crate::data::intern_path;
use crate::navigate::{browser_windows, folder_at_path, reveal};
use crate::{home, open_finder_window};

thread_local! {
    static APP: OnceCell<AsyncApp> = const { OnceCell::new() };
    static ASKED: Cell<bool> = const { Cell::new(false) };
    static WAITING: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// `Application::on_open_urls`: items other apps open with Found'er, or reveal while it is their file viewer.
pub(crate) fn arrived(urls: Vec<String>) {
    let paths: Vec<PathBuf> = urls.iter().filter_map(|url| file_path(url)).collect();
    if paths.is_empty() {
        return;
    }
    ASKED.set(true);
    WAITING.with_borrow_mut(|waiting| waiting.extend(paths));
    APP.with(|app| {
        if let Some(app) = app.get() {
            take_waiting(app);
        }
    });
}

/// The first window opens once launching settles, unless windows were restored or a request opened its own.
pub(crate) fn follow(restored: bool, cx: &mut App) {
    let app = cx.to_async();
    APP.with(|slot| slot.set(app.clone()).ok());
    take_waiting(&app);
    let settle = move || {
        if restored || ASKED.get() {
            return;
        }
        app.spawn(async move |cx| {
            cx.update(|cx| open_finder_window(intern_path(&home()), 0, cx));
        })
        .detach();
    };
    if NSRunningApplication::currentApplication().isFinishedLaunching() {
        settle();
        return;
    }
    // gpui runs the launch callback before AppKit takes the launch Apple Event, and that event's
    // open request reaches `arrived` ahead of this notification.
    let block = StackBlock::new(move |_: NonNull<NSNotification>| settle()).copy();
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSApplicationDidFinishLaunchingNotification),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(observer);
}

fn file_path(url: &str) -> Option<PathBuf> {
    let url = NSURL::URLWithString(&NSString::from_str(url))?;
    url.isFileURL()
        .then(|| url.path())
        .flatten()
        .map(|path| PathBuf::from(path.to_string()))
}

fn take_waiting(app: &AsyncApp) {
    if WAITING.with_borrow(Vec::is_empty) {
        return;
    }
    app.spawn(async move |cx| {
        cx.update(|cx| open(WAITING.take(), cx));
    })
    .detach();
}

fn open(paths: Vec<PathBuf>, cx: &mut App) {
    let mut items = Vec::new();
    for path in paths {
        match folder_of(&path) {
            Some(folder) => show_folder(folder, cx),
            None => items.push(path),
        }
    }
    reveal(&items, cx);
}

/// NSWorkspace hands a file viewer the items it reveals as an open-documents event, so a folder opens.
fn folder_of(path: &Path) -> Option<&'static str> {
    let package = path
        .file_name()
        .is_some_and(|name| is_package_name(&name.to_string_lossy()));
    (!package)
        .then(|| folder_at_path(&path.to_string_lossy()))
        .flatten()
}

fn show_folder(folder: &'static str, cx: &mut App) {
    let windows = browser_windows(cx);
    let shown = windows
        .iter()
        .find(|window| window.read(cx).is_ok_and(|demo| demo.here() == folder));
    if let Some(window) = shown {
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }
    match windows
        .first()
        .filter(|_| crate::prefs::opens_folders_in_tabs())
    {
        Some(front) => {
            front
                .update(cx, |demo, window, cx| demo.open_tab(folder, window, cx))
                .ok();
        }
        None => {
            open_finder_window(folder, windows.len(), cx);
        }
    }
}
