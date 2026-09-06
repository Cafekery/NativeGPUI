//! A Finder front end: real macOS chrome over the real file system, `NG_QUIT=<frames>` to exit.

mod art;
mod clipboard;
mod column;
mod commands;
mod conflicts;
mod data;
mod gallery;
mod header;
mod icon;
mod info;
mod list;
mod listing;
mod menus;
mod options;
mod overlay;
mod palette;
mod prefs;
mod preview;
mod read;
mod scope;
mod sidebar;
mod tags;
mod thumbs;
mod toolbar;
mod ui;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use finder_fs::index::Index;
use finder_fs::ops::{Conflict, Op};
use finder_fs::queue::{self, Journal, Queue};
use finder_fs::volumes;
use finder_fs::watch::Watcher;
use gpui::{
    AnyElement, App, Bounds, DragMoveEvent, FocusHandle, KeyBinding, KeyDownEvent, Keystroke,
    Modifiers, MouseButton, Pixels, RenderImage, ScrollHandle, ScrollStrategy, SharedString,
    UniformListScrollHandle, Window, WindowBounds, WindowHandle, WindowOptions, div, point,
    prelude::*, px, relative, size,
};
use native_gpui::sidebar::{NativeSidebar, SidebarMetrics};
use native_gpui::tokens::{self, FileType, SystemTheme};
use native_gpui::{
    QuickLookKey, install_menus, native_root, quick_look, quick_look_close, quick_look_update,
    quick_look_visible, remember_frame, system_corner_radius, window_options,
};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardWriting, NSView, NSWindow};
use objc2_foundation::{NSArray, NSSize, NSString, NSURL};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::art::Anchor;
use crate::commands::{Back, CopyNames, Forward, SelectAll, ZoomIn, ZoomOut};
use crate::data::{Column, Folder, Folders, Line, Row, intern_path, split_key};
use crate::header::{
    ColumnSpec, HEADER_H, HeaderDrag, NAME_MIN_W, PANE_INSET, band_glass, default_columns,
    pane_surface,
};
use crate::icon::{Grid, cell};
use crate::list::{ICON_SIZE, ICON_SMALL, RowDrag, row_h};
use crate::listing::{Keys, Listings, next_rev};
use crate::palette::Palette;
use crate::preview::{INSET, LIST_MIN_W, PREVIEW_W, PreviewWindow};
use crate::toolbar::PATH_H;
use crate::ui::axis_locked;

pub(crate) const ICON_VIEW: usize = 0;
pub(crate) const LIST_VIEW: usize = 1;
pub(crate) const COLUMN_VIEW: usize = 2;
pub(crate) const GALLERY_VIEW: usize = 3;

const LIST_ZOOM_SECS: f32 = 0.2;
/// Rows a search or a tag listing shows at most.
const HITS: usize = 200;
thread_local! {
    static START: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}
const SAVE_EVERY: Duration = Duration::from_secs(180);
const ICON_STOPS: [f32; 9] = [16., 24., 32., 48., 64., 96., 128., 256., 512.];
const STRIP_STOPS: [f32; 3] = [32., 48., 64.];

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

    pub(crate) fn here(&self) -> &'static str {
        self.history[self.at]
    }

    pub(crate) fn folder(&self) -> Option<&Folder> {
        self.folders.get(self.here())
    }

    /// The folder an operation lands in: where the cursor points in the browser or a search hit,
    /// otherwise the folder on screen.
    pub(crate) fn target(&self) -> &'static str {
        let here = self.here();
        let Some((holder, name)) = self.cursor.as_ref().and_then(|key| split_key(key)) else {
            return here;
        };
        if self.results() {
            return holder;
        }
        if self.view != COLUMN_VIEW {
            return here;
        }
        if self.selected.len() > 1 {
            return holder;
        }
        self.folders
            .get(holder)
            .and_then(|folder| folder.rows.iter().find(|row| row.name == name))
            .and_then(|row| row.opens)
            .unwrap_or(holder)
    }

    pub(crate) fn searching(&self) -> bool {
        !self.query.trim().is_empty() && !self.hits.is_empty()
    }

    /// Rows found elsewhere on the disk rather than read out of this folder: the search field's
    /// hits, or a tag's files.
    pub(crate) fn results(&self) -> bool {
        self.tag_listing.is_some() || self.searching()
    }

    /// The folder `text` names, absolute or under `~`, if it is one.
    pub(crate) fn folder_at_path(&self, text: &str) -> Option<&'static str> {
        let text = text.trim();
        let full = match text.strip_prefix('~') {
            Some(rest) => format!("{}{rest}", home().to_string_lossy()),
            None => text.to_string(),
        };
        let trimmed = full.trim_end_matches('/');
        let path = Path::new(match trimmed.is_empty() {
            true => "/",
            false => trimmed,
        });
        path.is_dir().then(|| intern_path(path))
    }

    pub(crate) fn crumb_path(&self) -> String {
        self.target().to_string()
    }

    pub(crate) fn retitle(&self, window: &mut Window) {
        match self.tag_listing.map(tag_title) {
            Some(title) => window.set_window_title(&title),
            None => window.set_window_title(&self.crumb_path()),
        }
    }

    pub(crate) fn pick_view(&mut self, view: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.set_view(view);
        self.retitle(window);
        cx.notify();
    }

    /// The folder each breadcrumb stands for, the volume first.
    pub(crate) fn crumb_ids(&self) -> Vec<&'static str> {
        let deep = self
            .folders
            .get(self.target())
            .map_or(1, |folder| folder.crumbs.len());
        let mut chain = Vec::with_capacity(deep);
        let mut at = Some(Path::new(self.target()));
        while let Some(path) = at.filter(|_| chain.len() < deep) {
            chain.push(intern_path(path));
            at = path.parent();
        }
        chain.reverse();
        chain
    }

    /// Keep the Go menu's Recent Folders in step with where the window has been.
    fn remember_recent(&mut self, id: &'static str) {
        let title = crate::data::folder_of(id).title.to_string();
        self.recents.retain(|(seen, _)| *seen != id);
        self.recents.insert(0, (id, title));
        self.recents.truncate(10);
        native_gpui::set_menu("Go", commands::go_rows(&self.recents));
    }

    pub(crate) fn go(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if self.here() == id {
            return;
        }
        self.history.truncate(self.at + 1);
        self.history.push(id);
        self.at = self.history.len() - 1;
        self.remember_recent(id);
        self.arrive(window, cx);
    }

    /// Finder's Show in Enclosing Folder: the parent folder, with the crumb chosen in it.
    pub(crate) fn show_enclosing(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(parent) = Path::new(id).parent().map(intern_path) else {
            return;
        };
        self.go(parent, window, cx);
        let key = SharedString::from(id);
        self.selected = std::iter::once(key.clone()).collect();
        self.cursor = Some(key.clone());
        self.anchor = Some(key.clone());
        self.reveal_next = Some(key);
    }

    pub(crate) fn step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let next = if forward {
            self.at + 1
        } else {
            self.at.wrapping_sub(1)
        };
        if next >= self.history.len() {
            return;
        }
        self.at = next;
        self.arrive(window, cx);
    }

    pub(crate) fn arrive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected.clear();
        self.cursor = None;
        self.anchor = None;
        self.query.clear();
        self.hits.clear();
        self.tag_listing = None;
        self.inspecting = None;
        self.icons.forget();
        self.clear_type_filter();
        self.load_column_widths();
        self.read_folder(self.here(), cx);
        self.settle();
        self.retitle(window);
        self.sidebar_follow();
        cx.notify();
    }

    /// Finder's search field: this folder's rows filter as they always have, and the index adds
    /// what it knows about the rest of the subtree.
    pub(crate) fn search(&mut self, text: String, cx: &mut Context<Self>) {
        self.query = text;
        self.run_search(cx);
    }

    /// Re-runs the current query, for a fresh scope or a fresh folder.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        self.tag_listing = None;
        self.hits.clear();
        self.hits_rev = next_rev();
        let query = self.query.trim().to_string();
        cx.notify();
        if query.is_empty() {
            return;
        }
        let under = match self.scope {
            scope::THIS_MAC => None,
            _ => Some(PathBuf::from(self.here())),
        };
        let wanted = query.clone();
        cx.spawn(async move |this, cx| {
            let hits = cx
                .background_executor()
                .spawn(async move {
                    let index = index();
                    index
                        .search(&query, under.as_deref(), HITS)
                        .iter()
                        .map(|hit| data::hit_row(&hit.path, &hit.entry, index))
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                if this.query.trim() == wanted {
                    this.hits = hits;
                    this.hits_rev = next_rev();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Finder's tag rows: the files Spotlight says carry the tag, in a listing titled with it.
    pub(crate) fn show_tag(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected.clear();
        self.cursor = None;
        self.anchor = None;
        self.query.clear();
        self.hits.clear();
        self.hits_rev = next_rev();
        self.tag_listing = Some(id);
        self.retitle(window);
        cx.notify();
        let tag = id.strip_prefix("tag:").unwrap_or_default().to_string();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let rows = data::tagged_rows(&tag, index(), HITS);
                    let marks: Vec<(SharedString, Vec<usize>)> = rows
                        .iter()
                        .map(|row| (row.path.clone(), crate::tags::row_tags(&row.path)))
                        .filter(|(_, carried)| !carried.is_empty())
                        .collect();
                    (rows, marks)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.tag_listing == Some(id) {
                    let (rows, marks) = found;
                    this.hits = rows;
                    this.hits_rev = next_rev();
                    this.tags.extend(marks);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Finder's scope bar: which segment is picked, per folder for the session, everywhere after.
    pub(crate) fn set_scope(&mut self, scope: usize, cx: &mut Context<Self>) {
        if self.scope == scope {
            return;
        }
        self.scope = scope;
        prefs::set_search_scope(scope);
        self.run_search(cx);
    }

    /// Every file operation runs through the queue; a move or copy that would collide with an
    /// existing name shows Finder's conflict sheet first.
    pub(crate) fn run_op(&mut self, op: Op, after: After, cx: &mut Context<Self>) {
        match conflicts::split(op, after) {
            conflicts::Split::Clean(op) => self.submit_op(op, Conflict::KeepBoth, after, cx),
            conflicts::Split::Conflicted { clean, sheet } => {
                if let Some(clean) = clean {
                    self.submit_op(clean, Conflict::KeepBoth, After::Nothing, cx);
                }
                self.conflict = Some(sheet);
                cx.notify();
            }
        }
    }

    pub(crate) fn settled(
        &mut self,
        touched: Vec<PathBuf>,
        results: Vec<PathBuf>,
        after: After,
        cx: &mut Context<Self>,
    ) {
        let landed = results.iter().filter_map(|path| path.parent());
        for dir in touched.iter().map(PathBuf::as_path).chain(landed) {
            let id = intern_path(dir);
            self.read_folder(id, cx);
        }
        if after == After::Nothing {
            return;
        }
        let keys: Vec<SharedString> = results
            .iter()
            .map(|path| SharedString::from(path.to_string_lossy().into_owned()))
            .collect();
        self.selected = keys.iter().cloned().collect();
        self.cursor = keys.last().cloned();
        self.anchor = self.cursor.clone();
        self.reveal_next = self.cursor.clone();
        if after == After::Rename {
            self.renaming = self.cursor.clone();
        }
        cx.notify();
    }

    /// Finder's error sheet: what the file system said, on the window it happened in.
    pub(crate) fn complain(&self, text: &str) {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        let alert = objc2_app_kit::NSAlert::new(mtm);
        alert.setMessageText(&objc2_foundation::NSString::from_str(text));
        match &self.host {
            Some(window) => alert.beginSheetModalForWindow_completionHandler(window, None),
            None => {
                alert.runModal();
            }
        }
    }

    pub(crate) fn set_view(&mut self, view: usize) {
        self.view = view;
        prefs::set_view(view);
        self.settle();
    }

    fn settle(&mut self) {
        match self.view {
            GALLERY_VIEW if self.cursor_line().is_none() => {
                if let Some(first) = self.lines(false).first().cloned() {
                    self.pick(&first, false, false);
                }
            }
            COLUMN_VIEW if self.cursor.is_none() => self.column_seed(),
            _ => {}
        }
    }

    pub(crate) fn open(&mut self, line: &Line, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = line.row.opens {
            self.go(id, window, cx);
        }
    }

    pub(crate) fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.view == COLUMN_VIEW {
            self.column_up();
            self.column_arrived(window, cx);
            return;
        }
        let from = self.here();
        let Some(parent) = self.folder().and_then(|folder| folder.parent) else {
            return;
        };
        self.go(parent, window, cx);
        let listing = self.lines(self.view == LIST_VIEW);
        let left = listing
            .iter()
            .find(|line| line.row.opens == Some(from))
            .cloned();
        if let Some(line) = left {
            self.pick(&line, false, false);
            self.reveal(&line.key);
        }
    }

    /// Finder's spring-loading: a folder held under a drag opens on its own.
    pub(crate) fn hover_folder(
        &mut self,
        into: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        match self.spring {
            Some((key, since)) if key == into => {
                if now.duration_since(since) > Duration::from_millis(700) {
                    self.spring = None;
                    self.go(into, window, cx);
                }
            }
            _ => self.spring = Some((into, now)),
        }
    }

    /// A row drag that reaches the sidebar becomes an AppKit file drag: the sidebar is a hosted
    /// `NSOutlineView`, and nothing of GPUI's own drags is offered to it.
    pub(crate) fn drag_to_sidebar(
        &mut self,
        event: &DragMoveEvent<RowDrag>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.event.position.x >= self.metrics().left {
            return;
        }
        let paths: Vec<PathBuf> = self
            .drag_keys(&event.drag(cx).key)
            .iter()
            .map(|key| PathBuf::from(key.as_ref()))
            .collect();
        if native_gpui::sidebar::drag_files(window, &paths) {
            cx.stop_active_drag(window);
        }
    }

    /// The rows a drag carries: the whole selection when the grabbed row is in it.
    pub(crate) fn drag_keys(&self, grabbed: &SharedString) -> Vec<SharedString> {
        match self.selected.contains(grabbed) {
            true => self.selected.iter().cloned().collect(),
            false => vec![grabbed.clone()],
        }
    }

    /// Finder's drop: the dragged rows move into `into`, or copy there while Option is held.
    pub(crate) fn drop_into(
        &mut self,
        into: &'static str,
        keys: &[SharedString],
        copy: bool,
        cx: &mut Context<Self>,
    ) {
        let items: Vec<PathBuf> = keys
            .iter()
            .map(|key| PathBuf::from(key.as_ref()))
            .filter(|path| path.parent() != Some(Path::new(into)))
            .collect();
        if items.is_empty() {
            return;
        }
        let into = PathBuf::from(into);
        let op = match copy {
            true => Op::Copy { items, into },
            false => Op::Move { items, into },
        };
        self.run_op(op, After::Select, cx);
    }

    pub(crate) fn reveal(&mut self, key: &SharedString) {
        let lines = self.lines(self.view == LIST_VIEW);
        if let Some(index) = self.line_at(&lines, key) {
            self.scroll.scroll_to_item(
                self.slot_of(&lines, index) + self.list_inset().0,
                ScrollStrategy::Center,
            );
        }
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.view == COLUMN_VIEW {
            return;
        }
        let folders: Vec<&'static str> = self
            .lines(self.view == LIST_VIEW)
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .filter_map(|line| line.row.opens)
            .collect();
        let mut folders = folders.into_iter();
        if let Some(first) = folders.next() {
            self.go(first, window, cx);
        }
        for (extra, id) in folders.enumerate() {
            open_finder_window(id, extra + 1, cx);
        }
    }

    fn disclose_selected(&mut self, open: bool, deep: bool) {
        let lines = self.lines(true);
        let keys: Vec<SharedString> = if self.selected.is_empty() {
            self.cursor.iter().cloned().collect()
        } else {
            lines
                .iter()
                .filter(|line| self.selected.contains(&line.key))
                .map(|line| line.key.clone())
                .collect()
        };
        let folders: Vec<(SharedString, &'static str)> = lines
            .iter()
            .filter(|line| keys.contains(&line.key))
            .filter_map(|line| line.row.opens.map(|into| (line.key.clone(), into)))
            .collect();
        let expandable = folders
            .iter()
            .filter(|(key, _)| self.expanded.contains(key) != open)
            .count();
        if expandable > 0 {
            for (key, into) in folders {
                if deep {
                    self.disclose_tree(&key, into, open);
                } else {
                    self.disclose(&key, open);
                }
            }
            return;
        }
        if open || keys.len() != 1 {
            return;
        }
        let Some(index) = self.line_at(&lines, &keys[0]) else {
            return;
        };
        let depth = lines[index].depth;
        if depth == 0 {
            return;
        }
        if let Some(parent) = lines[..index].iter().rev().find(|line| line.depth < depth) {
            let parent = parent.clone();
            self.pick(&parent, false, false);
            self.reveal(&parent.key);
        }
    }

    pub(crate) fn disclose_tree(&mut self, key: &SharedString, into: &'static str, open: bool) {
        self.disclose(key, open);
        let Some(folder) = self.folders.get(into) else {
            return;
        };
        let children: Vec<(SharedString, &'static str)> = folder
            .rows
            .iter()
            .filter_map(|row| row.opens.map(|deeper| (row.path.clone(), deeper)))
            .collect();
        for (key, deeper) in children {
            self.disclose_tree(&key, deeper, open);
        }
    }

    fn tab(&mut self, backwards: bool) {
        let lines = self.lines(self.view == LIST_VIEW);
        if lines.is_empty() {
            return;
        }
        let mut by_name: Vec<usize> = (0..lines.len()).collect();
        by_name.sort_by(|&i, &j| finder_fs::sort::name_cmp(&lines[i].row.name, &lines[j].row.name));
        let at = self
            .cursor
            .as_ref()
            .and_then(|key| self.line_at(&lines, key))
            .and_then(|index| by_name.iter().position(|&i| i == index));
        let next = match (at, backwards) {
            (Some(at), false) => (at + 1) % by_name.len(),
            (Some(at), true) => (at + by_name.len() - 1) % by_name.len(),
            (None, false) => 0,
            (None, true) => by_name.len() - 1,
        };
        let line = lines[by_name[next]].clone();
        self.pick(&line, false, false);
        self.reveal(&line.key);
    }

    /// Finder's type-select: jump to the first row starting with what is typed. When nothing
    /// starts with it, fall back to filtering rather than Finder's beep-and-do-nothing.
    fn type_select(&mut self, typed: &str, cx: &mut Context<Self>) {
        let now = Instant::now();
        if self
            .typed_at
            .is_some_and(|at| now.duration_since(at) > Duration::from_secs(1))
        {
            self.typed.clear();
            self.type_filter = None;
            self.type_filter_seen_match = false;
        }
        self.typed.push_str(&typed.to_lowercase());
        self.typed_at = Some(now);
        let lines = self.focused_lines();
        let prefix = lines
            .iter()
            .find(|line| finder_fs::sort::starts_with(&line.row.name, &self.typed))
            .cloned();
        match prefix {
            Some(line) => {
                self.type_filter = None;
                self.type_filter_seen_match = true;
                self.pick_in(&lines, &line, false, false);
                self.reveal(&line.key);
            }
            None => {
                self.type_filter = Some(self.typed.clone());
                if self.type_filter_seen_match {
                    self.schedule_filter_clear(cx);
                }
            }
        }
    }

    fn clear_type_filter(&mut self) {
        self.typed.clear();
        self.typed_at = None;
        self.type_filter = None;
        self.type_filter_seen_match = false;
    }

    /// Finder's type-select timeout: once a match existed, an idle spell clears the fallback
    /// filter on its own; a run that never matched anything waits for Escape or navigation.
    fn schedule_filter_clear(&mut self, cx: &mut Context<Self>) {
        let since = self.typed_at;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update(cx, |this, cx| {
                if this.typed_at == since {
                    this.clear_type_filter();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The rename lands on disk; the row comes back with the folder's next read.
    pub(crate) fn rename(&mut self, key: &SharedString, name: &str, cx: &mut Context<Self>) {
        let Some((_, old)) = split_key(key) else {
            return;
        };
        let name = name.trim();
        if name.is_empty() || name == old {
            return;
        }
        self.tags.remove(key);
        self.run_op(
            Op::Rename {
                path: PathBuf::from(key.as_ref()),
                name: name.to_owned(),
            },
            After::Select,
            cx,
        );
    }

    pub(crate) fn pick(&mut self, line: &Line, extend: bool, toggle: bool) {
        let lines = self.lines(self.view == LIST_VIEW);
        self.pick_in(&lines, line, extend, toggle);
    }

    pub(crate) fn pick_in(&mut self, lines: &[Line], line: &Line, extend: bool, toggle: bool) {
        self.inspecting = None;
        if toggle {
            if !self.selected.remove(&line.key) {
                self.selected.insert(line.key.clone());
            }
            self.anchor = Some(line.key.clone());
        } else if extend {
            let anchor = self
                .anchor
                .as_ref()
                .or(self.cursor.as_ref())
                .and_then(|key| self.line_at(lines, key))
                .unwrap_or(0);
            if let Some(target) = self.line_at(lines, &line.key) {
                let (lo, hi) = (anchor.min(target), anchor.max(target));
                self.selected = lines[lo..=hi].iter().map(|line| line.key.clone()).collect();
            }
        } else {
            self.selected.clear();
            self.selected.insert(line.key.clone());
            self.anchor = Some(line.key.clone());
        }
        self.cursor = Some(line.key.clone());
    }

    pub(crate) fn disclose(&mut self, key: &SharedString, open: bool) {
        if open {
            self.expanded.insert(key.clone());
        } else {
            self.expanded.remove(key);
        }
    }

    pub(crate) fn on_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        if key == "escape" && quick_look_visible() {
            quick_look_close();
            return;
        }
        if key == "escape" && self.type_filter.is_some() {
            self.clear_type_filter();
            cx.notify();
            return;
        }
        if key == "escape" && !self.cut.is_empty() {
            self.clear_cut(cx);
            return;
        }
        if key == "space" && self.cursor_line().is_some() {
            self.toggle_quick_look(window, cx);
            return;
        }
        let lines = self.focused_lines();
        if lines.is_empty()
            || (self.view == COLUMN_VIEW && self.column_key(key, modifiers, window, cx))
        {
            return;
        }
        let current = self
            .cursor
            .as_ref()
            .and_then(|key| self.line_at(&lines, key));
        let last = lines.len() - 1;
        let tree = self.view == LIST_VIEW;
        match key {
            "up" if modifiers.platform => self.up(window, cx),
            "down" | "o" if modifiers.platform => self.open_selected(window, cx),
            "up" | "down" | "left" | "right" if self.view == ICON_VIEW => {
                let columns = self.columns(window);
                let next = match current {
                    Some(at) => match key {
                        "up" if at >= columns => at - columns,
                        "down" if at + columns <= last => at + columns,
                        "left" if at % columns > 0 => at - 1,
                        "right" if at % columns + 1 < columns && at < last => at + 1,
                        _ => at,
                    },
                    None if matches!(key, "down" | "right") => 0,
                    None => last,
                };
                let line = &lines[next];
                if modifiers.shift {
                    self.selected.insert(line.key.clone());
                    self.cursor = Some(line.key.clone());
                } else {
                    self.pick(line, false, false);
                }
                let visible = self.list_room - self.pane_w();
                self.icons
                    .reveal(&line.key, next, columns, self.icon_size as f32, visible);
            }
            "home" | "end" if self.view == ICON_VIEW => self.icons.scroll_to_end(key == "end"),
            "left" | "right" if self.view == GALLERY_VIEW => {
                let next = match current {
                    Some(at) if key == "left" => at.saturating_sub(1),
                    Some(at) => (at + 1).min(last),
                    None => 0,
                };
                self.pick(&lines[next], modifiers.shift, false);
                self.strip_reveal(next);
            }
            "up" | "down" if self.view == GALLERY_VIEW => {}
            "home" | "end" if self.view == GALLERY_VIEW => {
                let x = if key == "end" {
                    -self.strip.max_offset().x
                } else {
                    px(0.)
                };
                self.strip.set_offset(point(x, px(0.)));
            }
            "up" | "down" => {
                let next = match current {
                    Some(at) if key == "up" => at.saturating_sub(1),
                    Some(at) => (at + 1).min(last),
                    None if key == "down" => 0,
                    None => last,
                };
                self.pick(&lines[next], modifiers.shift, false);
                self.reveal(&lines[next].key);
            }
            "right" | "left" if tree => self.disclose_selected(key == "right", modifiers.alt),
            "enter" => self.renaming = current.map(|at| lines[at].key.clone()),
            "tab" if self.view != COLUMN_VIEW => self.tab(modifiers.shift),
            "home" | "end" | "pageup" | "pagedown" if tree => {
                let (pads, _) = self.list_inset();
                let page = (f32::from(window.viewport_size().height) / self.row_h()) as usize;
                let top = self.scroll.0.borrow().base_handle.logical_scroll_top().0;
                let index = match key {
                    "home" => 0,
                    "end" => pads + last,
                    "pageup" => top.saturating_sub(page),
                    _ => (top + page).min(pads + last),
                };
                self.scroll.scroll_to_item(index, ScrollStrategy::Top);
            }
            _ if !modifiers.platform && !modifiers.control && !modifiers.alt => {
                let Some(typed) = event
                    .keystroke
                    .key_char
                    .as_deref()
                    .filter(|typed| typed.chars().all(|c| !c.is_control()))
                else {
                    return;
                };
                self.type_select(typed, cx);
            }
            _ => return,
        }
        if self.view == COLUMN_VIEW {
            self.column_arrived(window, cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn back(&mut self, _: &Back, window: &mut Window, cx: &mut Context<Self>) {
        self.step(false, window, cx);
    }

    pub(crate) fn forward(&mut self, _: &Forward, window: &mut Window, cx: &mut Context<Self>) {
        self.step(true, window, cx);
    }

    pub(crate) fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self
            .focused_lines()
            .iter()
            .map(|line| line.key.clone())
            .collect();
        cx.notify();
    }

    pub(crate) fn copy_names(&mut self, _: &CopyNames, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
    }

    /// Finder's ⌘C: the names as text and the files as file URLs, so a paste in Finder lands them.
    /// A copy also clears any pending ⌘X mark, Finder's own convention.
    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        self.clear_cut(cx);
        let keys: Vec<SharedString> = self
            .focused_lines()
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .map(|line| line.key.clone())
            .collect();
        if keys.is_empty() {
            return;
        }
        let names: Vec<&str> = keys
            .iter()
            .filter_map(|key| split_key(key).map(|(_, name)| name))
            .collect();
        let text = NSString::from_str(&names.join("\n"));
        let mut items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
            vec![ProtocolObject::from_retained(text)];
        items.extend(keys.iter().map(|key| {
            ProtocolObject::from_retained(NSURL::fileURLWithPath(&NSString::from_str(key)))
        }));
        let board = NSPasteboard::generalPasteboard();
        board.clearContents();
        board.writeObjects(&NSArray::from_retained_slice(&items));
    }

    pub(crate) fn metrics(&self) -> SidebarMetrics {
        self.sidebar
            .as_ref()
            .map(NativeSidebar::metrics)
            .unwrap_or_default()
    }

    pub(crate) fn preview_shown(&self) -> bool {
        self.preview && self.view != COLUMN_VIEW
    }

    /// The list's icon size this frame, mid-flight while a zoom animates.
    pub(crate) fn list_icon_now(&self) -> f32 {
        match self.list_zoom {
            Some((from, to, started)) => {
                let t = (started.elapsed().as_secs_f32() / LIST_ZOOM_SECS).min(1.);
                from + (to - from) * gpui::ease_in_out(t)
            }
            None => self.list_icon,
        }
    }

    pub(crate) fn row_h(&self) -> f32 {
        row_h(self.list_icon_now())
    }

    /// ⌘= and ⌘-: Finder toggles the list's icons, steps the icon view through its slider stops
    /// and the gallery strip through three, and leaves column view alone.
    pub(crate) fn zoom(&mut self, larger: bool, cx: &mut Context<Self>) {
        fn step(stops: &[f32], from: f32, larger: bool) -> Option<f32> {
            if larger {
                stops.iter().copied().find(|stop| *stop > from + 0.5)
            } else {
                stops.iter().rev().copied().find(|stop| *stop < from - 0.5)
            }
        }
        match self.view {
            LIST_VIEW => {
                let to = if larger { ICON_SIZE } else { ICON_SMALL };
                if to != self.list_icon {
                    let from = self.list_icon_now();
                    self.list_icon = to;
                    self.list_zoom = Some((from, to, Instant::now()));
                    prefs::set_list_icon(to);
                }
            }
            ICON_VIEW => {
                if let Some(size) = step(&ICON_STOPS, self.icon_size as f32, larger) {
                    self.icon_size = size as f64;
                    prefs::set_icon_size(size);
                }
            }
            GALLERY_VIEW => {
                if let Some(size) = step(&STRIP_STOPS, self.strip_icon, larger) {
                    self.strip_icon = size;
                    prefs::set_strip_icon(size);
                }
            }
            _ => return,
        }
        cx.notify();
    }

    /// Space: the system Quick Look panel over the selection, zooming out of the cursor's icon.
    pub(crate) fn toggle_quick_look(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if quick_look_visible() {
            quick_look_close();
            return;
        }
        let Some((items, current)) = self.quick_look_items() else {
            return;
        };
        let source = self.ql_icon.get();
        self.ql_shown = self.cursor.clone().map(|key| (key, source));
        let entity = cx.entity().downgrade();
        let handle = gpui::Window::window_handle(window);
        let app = cx.to_async();
        quick_look(window, items, current, source, move |key| {
            let entity = entity.clone();
            let mut app = app.clone();
            let _ = app.update_window(handle, |_, window, cx| {
                if let Some(entity) = entity.upgrade() {
                    entity.update(cx, |this, cx| this.quick_look_key(key, window, cx));
                }
            });
        });
    }

    /// Finder previews the whole selection with the cursor's item showing, or just the cursor.
    fn quick_look_items(&self) -> Option<(Vec<PathBuf>, usize)> {
        let cursor = self.cursor.clone()?;
        let lines = self.lines(self.view == LIST_VIEW);
        let mut items: Vec<&Line> = lines
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .collect();
        if !items.iter().any(|line| line.key == cursor) {
            items = lines.iter().filter(|line| line.key == cursor).collect();
        }
        let current = items.iter().position(|line| line.key == cursor)?;
        let paths = items
            .iter()
            .map(|line| PathBuf::from(line.row.path.as_ref()))
            .collect();
        Some((paths, current))
    }

    /// The panel follows the selection while it is up.
    fn sync_quick_look(&mut self, window: &Window) {
        if !quick_look_visible() {
            self.ql_shown = None;
            return;
        }
        let Some(cursor) = self.cursor.clone() else {
            return;
        };
        let source = self.ql_icon.get();
        if self
            .ql_shown
            .as_ref()
            .is_some_and(|(key, at)| *key == cursor && *at == source)
        {
            return;
        }
        let Some((items, current)) = self.quick_look_items() else {
            return;
        };
        quick_look_update(window, items, current, source);
        self.ql_shown = Some((cursor, source));
    }

    /// The panel's arrow keys move the selection the way the same keys do in the window.
    fn quick_look_key(&mut self, key: QuickLookKey, window: &mut Window, cx: &mut Context<Self>) {
        let key = match key {
            QuickLookKey::Up => "up",
            QuickLookKey::Down => "down",
            QuickLookKey::Left => "left",
            QuickLookKey::Right => "right",
        };
        let event = KeyDownEvent {
            keystroke: Keystroke {
                key: key.into(),
                modifiers: Modifiers::default(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        };
        self.on_key(&event, window, cx);
    }

    fn sync_window_min(&mut self, window: &Window) {
        let width = f32::from(self.metrics().left)
            + LIST_MIN_W
            + if self.preview_shown() {
                self.preview_w
            } else {
                0.
            };
        if (width - self.window_min_w).abs() < 0.5 {
            return;
        }
        let Some(ns_window) = ns_window(window) else {
            return;
        };
        self.window_min_w = width;
        let height = ns_window.contentMinSize().height;
        ns_window.setContentMinSize(NSSize {
            width: f64::from(width),
            height,
        });
    }

    pub(crate) fn list_content_w(&self) -> f32 {
        self.name_w + self.list_columns.iter().map(|spec| spec.width).sum::<f32>() + PANE_INSET
    }

    fn list_column(
        &self,
        body: AnyElement,
        headers: Option<AnyElement>,
        path_bar: Option<AnyElement>,
        palette: &Palette,
        metrics: SidebarMetrics,
        corner: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The scope bar lives in the same hosted band as the toolbar and column header: it grows
        // the content's top inset the way `list_inset` grows the list's.
        let scope_h = if self.searching() { scope::SCOPE_H } else { 0. };
        let scope_row = (scope_h > 0.).then(|| self.scope_row(cx));
        let above = metrics.top;
        let top = above + px(scope_h);
        // The path bar takes height from the list, not from the pane, which runs to the corner.
        let foot = if path_bar.is_some() { PATH_H } else { 0. };
        let banded = headers.is_some();
        let opaque = matches!(self.view, ICON_VIEW | LIST_VIEW | COLUMN_VIEW);
        let mut surface = div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(foot))
            .top(if banded { px(0.) } else { top })
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(opaque, |surface| surface.bg(palette.surface))
            .when(!banded && self.view != ICON_VIEW, |column| {
                column.border_t_1().border_color(palette.separator)
            });
        // The list stops at the pane until its columns no longer fit, then runs on beneath it.
        let pane = self.pane_w();
        let band = (self.list_room - pane).max(self.list_content_w());
        let width = metrics.left + px(band + pane);
        if banded {
            surface = surface.child(
                axis_locked(
                    div()
                        .id("ng.finder.hscroll")
                        .size_full()
                        .overflow_x_scroll()
                        .track_scroll(&self.hscroll),
                )
                .child(
                    div()
                        .h_full()
                        .w(width)
                        .min_w(relative(1.))
                        .pl(metrics.left)
                        .pr(px(pane))
                        .flex()
                        .flex_col()
                        .child(body),
                ),
            );
        } else {
            surface = surface.child(body);
        }
        div()
            .flex_1()
            .relative()
            .overflow_hidden()
            .child(surface)
            // A Layer::Above material blurs only what is painted before it: rows, glass, header.
            .when(banded || scope_h > 0., |column| {
                let height = top + if banded { px(HEADER_H) } else { px(0.) };
                column.child(band_glass(px(pane), height))
            })
            .children(scope_row.map(|row| {
                div()
                    .absolute()
                    .top(above)
                    .left_0()
                    .right(px(pane))
                    .h(px(scope_h))
                    .pl(metrics.left)
                    .child(row)
            }))
            .when(pane > 0., |column| column.child(pane_surface(px(pane), corner)))
            .children(path_bar.map(|bar| {
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right(px(pane))
                    // Only the list view reaches here unshifted; the others are already inset.
                    .when(banded, |bar_row| bar_row.pl(metrics.left))
                    .child(bar)
            }))
            // A second scroller on the same handle, the pane's width shorter, so header and rows
            // clamp and scroll alike.
            .children(headers.map(|headers| {
                axis_locked(
                    div()
                        .id("ng.finder.hscroll.header")
                        .absolute()
                        .top(top)
                        .left_0()
                        .right(px(pane))
                        .h(px(HEADER_H))
                        .overflow_x_scroll()
                        .track_scroll(&self.hscroll),
                )
                .child(
                    div()
                        .h_full()
                        .w(width - px(pane))
                        .min_w(relative(1.))
                        .pl(metrics.left)
                        .child(headers),
                )
            }))
            .children(
                (self.view == LIST_VIEW)
                    .then(|| self.type_filter_pill(palette, pane, foot))
                    .flatten(),
            )
            .into_any_element()
    }

    pub(crate) fn pane_w(&self) -> f32 {
        if self.preview_shown() {
            self.preview_w
        } else {
            0.
        }
    }

    /// Tiles per row: the width beside the sidebar, pane included, since the grid scrolls under it.
    pub(crate) fn columns(&self, window: &Window) -> usize {
        let metrics = self.metrics();
        let avail = f32::from(window.viewport_size().width)
            - f32::from(metrics.left)
            - f32::from(metrics.right)
            - 24.;
        let tile = cell(self.icon_size as f32);
        ((avail / tile).floor() as usize).max(1)
    }
}

pub(crate) fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }?.window()
}

impl Render for FinderDemo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.sidebar.is_none() {
            self.install(window, cx);
        }
        if !self.toolbar {
            self.install_toolbar(window, cx);
        }
        if self.host.is_none() {
            self.host = ns_window(window);
        }
        let began = Instant::now();
        self.scale = window.scale_factor();
        self.frames += 1;
        if let Some(limit) = self.quit_at {
            window.request_animation_frame();
            if self.frames >= limit {
                std::process::exit(0);
            }
        }
        self.sync_window_min(window);
        self.sync_preview_window(window, cx);
        native_gpui::set_trailing_inset(window, self.pane_w());
        if let Some((_, _, started)) = self.list_zoom {
            if started.elapsed().as_secs_f32() >= LIST_ZOOM_SECS {
                self.list_zoom = None;
            } else {
                window.request_animation_frame();
            }
        }

        let metrics = self.metrics();
        let corner = system_corner_radius(window);
        let palette = Palette::new(self.list_focused);
        let family = tokens::text_font_family(cx).unwrap_or_else(tokens::system_font_family);

        let path_bar = self
            .path_bar_shown
            .then(|| self.path_bar(&palette, window, cx));
        let status_bar = self.status_bar_shown.then(|| self.status_bar(&palette, cx));
        let body = match self.view {
            ICON_VIEW => self.icon_body(&palette, window, cx),
            COLUMN_VIEW => self.column_body(&palette, cx),
            GALLERY_VIEW => self.gallery_body(&palette, window, cx),
            _ => self.list_body(&palette, cx),
        };
        // The rows only: a menu whose element also spans the path bar swallows the crumbs' own.
        let body = menus::attach(
            "ng.finder.background",
            menus::background(self),
            cx.entity().downgrade(),
            body,
        )
        .into_any_element();
        let headers = (self.view == LIST_VIEW).then(|| self.header_row(&palette, cx));

        let list_view = self.view == LIST_VIEW;
        let sheet = self
            .go_sheet
            .clone()
            .map(|text| self.go_sheet(&text, &palette, window, cx));
        let conflict_sheet = self
            .conflict
            .is_some()
            .then(|| self.conflict_sheet(&palette, cx));
        let jobs_popover = self.jobs_open.then(|| self.jobs_popover(&palette, cx));
        self.pump(cx);
        self.sync_quick_look(window);
        let root = native_root(
            div()
                .size_full()
                .relative()
                .font_family(family)
                .text_size(tokens::system_font_size())
                .flex()
                .flex_col()
                .pr(metrics.right)
                .pb(metrics.bottom)
                .child(
                    div()
                        .track_focus(&self.focus)
                        .key_context(LIST_CONTEXT)
                        .on_action(cx.listener(Self::select_all))
                        .on_action(cx.listener(Self::copy_names))
                        .on_action(cx.listener(Self::back))
                        .on_action(cx.listener(Self::forward))
                        .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(true, cx)))
                        .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(false, cx)))
                        .map(|element| self.menu_bar_actions(element, cx))
                        .map(|element| self.wire_undo_redo(element, cx))
                        .on_key_down(cx.listener(Self::on_key))
                        .on_drag_move(cx.listener(Self::drag_to_sidebar))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                // The native sidebar is an NSView; a click here takes AppKit first
                                // responder back to the GPUI view so bare keys reach on_key.
                                native_gpui::focus_gpui(window);
                                window.focus(&this.focus, cx);
                            }),
                        )
                        .size_full()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .overflow_hidden()
                                .when(!list_view, |row| row.pl(metrics.left))
                                .child(self.list_column(
                                    body, headers, path_bar, &palette, metrics, corner, cx,
                                )),
                        )
                        .children(
                            status_bar.map(|bar| div().flex_none().pl(metrics.left).child(bar)),
                        )
                        .children(sheet)
                        .children(conflict_sheet)
                        .children(jobs_popover),
                ),
        );
        trace_frame(self, began);
        root
    }
}

/// `NG_FRAME_TRACE=1`: how long this window spent building each frame, and over how much.
fn trace_frame(demo: &FinderDemo, began: Instant) {
    static ON: OnceLock<bool> = OnceLock::new();
    if !ON.get_or_init(|| env::var("NG_FRAME_TRACE").is_ok()) {
        return;
    }
    eprintln!(
        "frame {} build {:.2}ms rows {} selected {} folders {}",
        demo.frames,
        began.elapsed().as_secs_f64() * 1000.,
        demo.folder().map_or(0, |folder| folder.rows.len()),
        demo.selected.len(),
        demo.folders.len(),
    );
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

/// What a finished operation leaves behind it.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum After {
    Nothing,
    Select,
    Rename,
}

/// The folders an operation changes, which are read again once it lands.
pub(crate) fn touched(op: &Op) -> Vec<PathBuf> {
    let parents = |items: &[PathBuf]| {
        items
            .iter()
            .filter_map(|item| item.parent().map(PathBuf::from))
            .collect::<Vec<_>>()
    };
    match op {
        Op::Move { items, into } | Op::Copy { items, into } => {
            let mut dirs = parents(items);
            dirs.push(into.clone());
            dirs
        }
        Op::Trash(items) | Op::DeleteImmediately(items) | Op::Duplicate(items) => parents(items),
        Op::Rename { path, .. } => parents(std::slice::from_ref(path)),
        Op::NewFolder { into, .. } | Op::NewFile { into, .. } => vec![into.clone()],
    }
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
