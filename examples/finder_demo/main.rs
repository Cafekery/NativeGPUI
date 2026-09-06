//! A Finder front end: real macOS chrome over the real file system, `NG_QUIT=<frames>` to exit.

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
mod prefs;
mod preview;
mod quicklook;
mod read;
mod scope;
mod search;
mod select;
mod sidebar;
mod tags;
mod thumbs;
mod toolbar;
mod ui;
mod view;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use finder_fs::index::Index;
use finder_fs::queue::{self, Journal, Queue};
use finder_fs::volumes;
use finder_fs::watch::Watcher;
use gpui::{
    App, Bounds, FocusHandle, KeyBinding, Pixels, RenderImage, ScrollHandle, SharedString,
    UniformListScrollHandle, WindowBounds, WindowHandle, WindowOptions, point, prelude::*, px,
    size,
};
use native_gpui::sidebar::NativeSidebar;
use native_gpui::tokens::{self, FileType, SystemTheme};
use native_gpui::{install_menus, remember_frame, window_options};
use objc2::rc::Retained;
use objc2_app_kit::NSWindow;

use crate::art::Anchor;
use crate::commands::{Back, CopyNames, Forward, SelectAll, ZoomIn, ZoomOut};
use crate::data::{Column, Folders, Line, Row, intern_path};
use crate::header::{ColumnSpec, HeaderDrag, NAME_MIN_W, default_columns};
use crate::icon::Grid;
use crate::list::{ICON_SIZE, ICON_SMALL};
use crate::listing::{Keys, Listings};
use crate::preview::{INSET, PREVIEW_W, PreviewWindow};
use crate::view::LIST_VIEW;

thread_local! {
    static START: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}
const SAVE_EVERY: Duration = Duration::from_secs(180);
pub(crate) const LIST_CONTEXT: &str = "FinderList";

pub(crate) struct FinderDemo {
    pub(crate) folders: Folders,
    pub(crate) sidebar: Option<NativeSidebar>,
    pub(crate) sidebar_groups: Vec<sidebar::Group>,
    pub(crate) sidebar_rows: Vec<Vec<&'static str>>,
    pub(crate) toolbar: bool,
    pub(crate) history: Vec<&'static str>,
    pub(crate) at: usize,
    pub(crate) view: usize,
    pub(crate) sort: Column,
    pub(crate) ascending: bool,
    pub(crate) list_columns: Vec<ColumnSpec>,
    pub(crate) name_w: f32,
    pub(crate) header_drag: Option<HeaderDrag>,
    pub(crate) query: String,
    /// Finder's scope bar: `scope::THIS_MAC` or `scope::THIS_FOLDER`.
    pub(crate) scope: usize,
    pub(crate) expanded: Keys,
    pub(crate) selected: Keys,
    pub(crate) cursor: Option<SharedString>,
    pub(crate) anchor: Option<SharedString>,
    pub(crate) renaming: Option<SharedString>,
    /// ⌘X's mark: these rows dim until a paste moves them or the mark clears.
    pub(crate) cut: Keys,
    cache: RefCell<Listings>,
    /// What the last selection summary was computed over, and the summary itself.
    pub(crate) stats: Option<preview::Stats>,
    stats_pending: Option<(u64, usize)>,
    /// What [`Self::follow`] last pointed the watcher and the pins at.
    followed: Option<(u64, &'static str, &'static str, usize)>,
    /// Keys, side in pixels, image.
    pub(crate) stack: RefCell<Option<(Vec<SharedString>, u32, Arc<RenderImage>)>>,
    typed: String,
    typed_at: Option<Instant>,
    /// Set once a typed prefix matches no row's start; the substring filter it fell back to.
    pub(crate) type_filter: Option<String>,
    /// Whether type-select found a match earlier in the run typing `type_filter` reached.
    type_filter_seen_match: bool,
    pub(crate) tags: HashMap<SharedString, Vec<usize>>,
    pub(crate) preview: bool,
    pub(crate) preview_w: f32,
    pub(crate) preview_window: Option<PreviewWindow>,
    pub(crate) preview_opening: bool,
    pub(crate) preview_expanded: bool,
    pub(crate) list_focused: bool,
    pub(crate) list_room: f32,
    /// The list's width beside the pane at the last layout; what it gains or loses goes to Name.
    pub(crate) list_span: f32,
    pub(crate) path_bar_shown: bool,
    pub(crate) status_bar_shown: bool,
    pub(crate) show_hidden: bool,
    pub(crate) grouped: bool,
    spring: Option<(&'static str, Instant)>,
    /// The folders this window has been through, newest first, for the Go menu.
    pub(crate) recents: Vec<(&'static str, String)>,
    /// The path typed into Finder's Go to Folder sheet, while it is open.
    pub(crate) go_sheet: Option<String>,
    /// The cursor row's icon, as last drawn: the Quick Look panel zooms out of it.
    pub(crate) ql_icon: Anchor,
    ql_shown: Option<(SharedString, Option<Bounds<Pixels>>)>,
    pub(crate) icon_size: f64,
    pub(crate) list_icon: f32,
    /// `(from, to, started)` while the list's icons grow or shrink between the two sizes.
    pub(crate) list_zoom: Option<(f32, f32, Instant)>,
    pub(crate) strip_icon: f32,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) hscroll: ScrollHandle,
    pub(crate) strip: ScrollHandle,
    pub(crate) column_scroll: ScrollHandle,
    pub(crate) column_depth: usize,
    /// What each browser column has been dragged to this session, keyed by the folder it lists.
    pub(crate) column_widths: HashMap<&'static str, f32>,
    /// One per browser column, so a column knows which of its rows are on screen.
    pub(crate) column_scrolls: Vec<ScrollHandle>,
    /// A column has painted rows whose bounds it does not know yet: draw once more.
    pub(crate) column_settling: RefCell<bool>,
    pub(crate) column_grab: Option<(&'static str, f32)>,
    pub(crate) icons: Grid,
    /// The pixels a point buys on this window's screen, for the thumbnails it asks for.
    pub(crate) scale: f32,
    pub(crate) window_min_w: f32,
    /// FSEvents for the folders on screen; a change re-reads what it names.
    watcher: Watcher,
    watching: HashSet<&'static str>,
    /// What the index found elsewhere for the search field.
    hits: Vec<Arc<Row>>,
    hits_rev: u64,
    /// The sidebar tag row this window is listing, `tag:` on its own for every tag at once.
    tag_listing: Option<&'static str>,
    /// The folder the path bar's Show Inspector named, while the pane shows it.
    pub(crate) inspecting: Option<Line>,
    /// A row an operation made, to scroll to as soon as its folder is read again.
    reveal_next: Option<SharedString>,
    /// The window an error sheet hangs on.
    host: Option<Retained<NSWindow>>,
    pub(crate) focus: FocusHandle,
    pub(crate) frames: u64,
    pub(crate) quit_at: Option<u64>,
    /// Every move/copy/trash/etc. runs through here, one at a time, off the main thread.
    pub(crate) queue: Queue,
    pub(crate) journal: Journal,
    /// The pending queue events a background thread is handing this window since the last frame.
    queue_events: Arc<Mutex<Vec<queue::Event>>>,
    /// The conflict sheet, while one of the queue's operations has a name collision to resolve.
    pub(crate) conflict: Option<conflicts::Sheet>,
    /// The queue's jobs, for the toolbar's progress popover.
    pub(crate) jobs: Vec<conflicts::JobRow>,
    pub(crate) jobs_open: bool,
}

impl FinderDemo {
    pub(crate) fn new(start: &'static str, cx: &mut Context<Self>) -> Self {
        let groups = sidebar::groups();
        let changes: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = changes.clone();
        read::spawn_folder_tick(changes, cx);
        let queue_events: Arc<Mutex<Vec<queue::Event>>> = Arc::new(Mutex::new(Vec::new()));
        let queue_sink = queue_events.clone();
        let queue = Queue::new(move |event| queue_sink.lock().unwrap().push(event));
        read::spawn_queue_tick(cx);
        let mut this = Self {
            folders: Folders::default(),
            sidebar: None,
            sidebar_rows: sidebar::stored_rows(&groups),
            sidebar_groups: groups,
            toolbar: false,
            history: vec![start],
            at: 0,
            view: prefs::view().unwrap_or(LIST_VIEW),
            sort: Column::Name,
            ascending: true,
            list_columns: default_columns(),
            name_w: NAME_MIN_W,
            header_drag: None,
            query: String::new(),
            scope: prefs::search_scope().unwrap_or(scope::THIS_FOLDER),
            expanded: Keys::default(),
            selected: Keys::default(),
            cursor: None,
            anchor: None,
            renaming: None,
            cut: Keys::default(),
            cache: RefCell::default(),
            stats: None,
            stats_pending: None,
            followed: None,
            stack: RefCell::new(None),
            typed: String::new(),
            typed_at: None,
            type_filter: None,
            type_filter_seen_match: false,
            tags: HashMap::new(),
            preview: true,
            preview_w: prefs::pane_width().unwrap_or(PREVIEW_W),
            preview_window: None,
            preview_opening: false,
            preview_expanded: false,
            list_focused: false,
            list_room: 0.,
            list_span: f32::NAN,
            path_bar_shown: true,
            status_bar_shown: false,
            show_hidden: false,
            grouped: false,
            spring: None,
            recents: Vec::new(),
            go_sheet: None,
            ql_icon: Anchor::default(),
            ql_shown: None,
            icon_size: prefs::icon_size().unwrap_or(64.) as f64,
            list_icon: prefs::list_icon().unwrap_or(ICON_SIZE),
            list_zoom: None,
            strip_icon: prefs::strip_icon().unwrap_or(48.),
            scroll: UniformListScrollHandle::new(),
            hscroll: ScrollHandle::new(),
            strip: ScrollHandle::new(),
            column_scroll: ScrollHandle::new(),
            column_depth: 0,
            column_widths: HashMap::new(),
            column_scrolls: Vec::new(),
            column_settling: RefCell::new(false),
            column_grab: None,
            icons: Grid::default(),
            scale: 2.,
            window_min_w: f32::NAN,
            watcher: Watcher::new(move |dirs| sink.lock().unwrap().extend(dirs)),
            watching: HashSet::new(),
            hits: Vec::new(),
            hits_rev: 0,
            tag_listing: None,
            inspecting: None,
            reveal_next: None,
            host: None,
            focus: cx.focus_handle(),
            frames: 0,
            quit_at: env::var("NG_QUIT").ok().and_then(|v| v.parse().ok()),
            queue,
            journal: Journal::new(),
            queue_events,
            conflict: None,
            jobs: Vec::new(),
            jobs_open: false,
        };
        this.load_column_widths();
        this
    }

    /// Rasterise the artwork every folder wears up front, so no frame pays for it.
    pub(crate) fn warm_icons(&self, cx: &mut Context<Self>) {
        let kinds = [
            FileType::Folder,
            FileType::Path("/"),
            FileType::Extension("app"),
        ];
        let sizes = [ICON_SMALL, ICON_SIZE, PREVIEW_W - 1. - 2. * INSET];
        cx.spawn(async move |_, cx| {
            for kind in kinds {
                cx.background_executor()
                    .timer(Duration::from_millis(4))
                    .await;
                for size in sizes {
                    tokens::file_icon(kind, px(size));
                }
            }
        })
        .detach();
    }
}

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
