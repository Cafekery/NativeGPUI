use std::cell::{Cell, OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use block2::StackBlock;
use finder_fs::kind::is_package_name;
use finder_fs::viewer;
use gpui::{App, AsyncApp};
use objc2::rc::Retained;
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplicationDidFinishLaunchingNotification, NSApplicationWillFinishLaunchingNotification,
    NSBeep, NSRunningApplication,
};
use objc2_foundation::{
    NSAppleEventDescriptor, NSAppleEventManager, NSArray, NSBundle, NSData, NSNotification,
    NSNotificationCenter, NSNotificationName, NSObject, NSString, NSURL, NSURLPathKey,
};

use crate::data::intern_path;
use crate::navigate::{browser_windows, folder_at_path, reveal, showing};
use crate::{home, open_finder_window};

const CORE_EVENTS: u32 = u32::from_be_bytes(*b"aevt");
const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const LIST: u32 = u32::from_be_bytes(*b"list");
const BOOKMARK: u32 = u32::from_be_bytes(*b"bmrk");
const FILE_URL: u32 = u32::from_be_bytes(*b"furl");

thread_local! {
    static APP: OnceCell<AsyncApp> = const { OnceCell::new() };
    static ASKED: Cell<bool> = const { Cell::new(false) };
    static WAITING: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGOpenDocuments"]
    struct Opener;

    impl Opener {
        #[unsafe(method(openDocuments:withReplyEvent:))]
        fn open_documents(&self, event: &NSAppleEventDescriptor, _: &NSAppleEventDescriptor) {
            opened(event);
        }
    }
);

/// AppKit's own open-documents handler resolves each bookmark on the main thread, stalling on a dead share.
pub(crate) fn take_open_documents() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let opener: Retained<Opener> = unsafe { msg_send![Opener::alloc(mtm), init] };
    let launching = unsafe { NSApplicationWillFinishLaunchingNotification };
    // AppKit installs its handlers whenever it finishes launching, before it posts this.
    observe(launching, move || {
        let manager = NSAppleEventManager::sharedAppleEventManager();
        let _: () = unsafe {
            msg_send![
                &*manager,
                setEventHandler: &*opener,
                andSelector: sel!(openDocuments:withReplyEvent:),
                forEventClass: CORE_EVENTS,
                andEventID: OPEN_DOCUMENTS
            ]
        };
    });
}

fn observe(name: &NSNotificationName, run: impl Fn() + Clone + 'static) {
    let block = StackBlock::new(move |_: NonNull<NSNotification>| run()).copy();
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(name),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(observer);
}

pub(crate) fn arrived(urls: Vec<String>) {
    take(urls.iter().filter_map(|url| file_path(url)).collect());
}

fn opened(event: &NSAppleEventDescriptor) {
    let direct: Option<Retained<NSAppleEventDescriptor>> =
        unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
    let Some(direct) = direct else {
        return;
    };
    let items = match kind_of(&direct) {
        LIST => (1..=direct.numberOfItems())
            .filter_map(|at| direct.descriptorAtIndex(at))
            .collect(),
        _ => vec![direct],
    };
    let mut paths = Vec::new();
    let mut unread = Vec::new();
    for item in items {
        let kind = kind_of(&item);
        let data = item.data();
        let path = match kind {
            BOOKMARK => bookmarked(&data),
            FILE_URL => named(&data.to_vec()),
            _ => None,
        };
        match path {
            Some(path) => paths.push(path),
            None => unread.push((kind, data.to_vec())),
        }
    }
    take(paths);
    if !unread.is_empty() {
        coerce_later(unread);
    }
}

fn kind_of(item: &NSAppleEventDescriptor) -> u32 {
    unsafe { msg_send![item, descriptorType] }
}

fn bookmarked(data: &NSData) -> Option<PathBuf> {
    let key = unsafe { NSURLPathKey };
    let values = NSURL::resourceValuesForKeys_fromBookmarkData(&NSArray::from_slice(&[key]), data)?;
    let path = values.objectForKey(key)?.downcast::<NSString>().ok()?;
    Some(PathBuf::from(path.to_string()))
}

/// A file URL item holds its URL's text, or by its documentation the bare path.
fn named(bytes: &[u8]) -> Option<PathBuf> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.starts_with('/') {
        Some(PathBuf::from(text))
    } else {
        file_path(text)
    }
}

/// An alias or any other kind of item goes through the system's coercion, which can wait on its volume.
fn coerce_later(unread: Vec<(u32, Vec<u8>)>) {
    APP.with(|app| {
        let Some(app) = app.get() else {
            return;
        };
        let coercing = app.background_executor().spawn(async move {
            unread
                .iter()
                .filter_map(|(kind, bytes)| coerced(*kind, bytes))
                .collect::<Vec<_>>()
        });
        app.spawn(async move |_| take(coercing.await)).detach();
    });
}

fn coerced(kind: u32, bytes: &[u8]) -> Option<PathBuf> {
    let data = NSData::with_bytes(bytes);
    let item: Option<Retained<NSAppleEventDescriptor>> = unsafe {
        msg_send![NSAppleEventDescriptor::class(), descriptorWithDescriptorType: kind, data: &*data]
    };
    let url: Option<Retained<NSAppleEventDescriptor>> =
        unsafe { msg_send![&*item?, coerceToDescriptorType: FILE_URL] };
    named(&url?.data().to_vec())
}

fn take(paths: Vec<PathBuf>) {
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

pub(crate) fn follow(restored: bool, cx: &mut App) {
    let app = cx.to_async();
    APP.with(|slot| slot.set(app.clone()).ok());
    take_waiting(&app);
    let settle = move || {
        if restored || ASKED.get() {
            return;
        }
        app.spawn(async move |cx| {
            cx.update(|cx| open_finder_window(intern_path(&home()), cx));
        })
        .detach();
    };
    if NSRunningApplication::currentApplication().isFinishedLaunching() {
        settle();
        return;
    }
    let launched = unsafe { NSApplicationDidFinishLaunchingNotification };
    // gpui runs the launch callback before AppKit takes the launch Apple Event, whose open request arrives first.
    observe(launched, settle);
}

fn file_path(url: &str) -> Option<PathBuf> {
    let url = NSURL::URLWithString(&NSString::from_str(url))?;
    if !url.isFileURL() {
        return None;
    }
    Some(PathBuf::from(url.path()?.to_string()))
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
    if path
        .file_name()
        .is_some_and(|name| is_package_name(&name.to_string_lossy()))
    {
        return None;
    }
    folder_at_path(&path.to_string_lossy())
}

fn show_folder(folder: &'static str, cx: &mut App) {
    let windows = browser_windows(cx);
    if let Some(window) = showing(folder, &windows, cx) {
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }
    if let Some(front) = windows
        .first()
        .filter(|_| crate::prefs::opens_folders_in_tabs())
    {
        front
            .update(cx, |demo, window, cx| demo.open_tab(folder, window, cx))
            .ok();
    } else {
        open_finder_window(folder, cx);
    }
}

pub(crate) fn is_default() -> bool {
    own_id().is_some_and(|id| viewer::current() == Some(id))
}

pub(crate) fn can_be_default() -> bool {
    own_id().is_some() && !viewer::is_translocated(&own_bundle())
}

/// LaunchServices refuses `public.folder` to any handler but Finder, so only `NSFileViewer` is ours to set.
pub(crate) fn toggle_default(cx: &mut App) {
    let Some(id) = own_id() else {
        return;
    };
    let (bundle, turn_on) = (own_bundle(), !is_default());
    let toggled = cx.background_executor().spawn(async move {
        if turn_on {
            viewer::on(&id, &bundle)
        } else {
            viewer::off(&id)
        }
    });
    cx.spawn(async move |_| {
        if toggled.await.is_err() {
            NSBeep();
        }
    })
    .detach();
}

pub(crate) fn keep_guard() {
    let bundle = own_bundle();
    if !viewer::is_translocated(&bundle) {
        viewer::register(&bundle);
    }
    if let Some(id) = own_id() {
        _ = viewer::follow(&id, &bundle);
    }
}

fn own_id() -> Option<String> {
    Some(NSBundle::mainBundle().bundleIdentifier()?.to_string())
}

fn own_bundle() -> PathBuf {
    PathBuf::from(NSBundle::mainBundle().bundlePath().to_string())
}
