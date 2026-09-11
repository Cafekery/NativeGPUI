use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use finder_fs::queue::{self, Journal, Queue};
use finder_fs::watch::Watcher;
use gpui::{
    Bounds, Context, FocusHandle, Pixels, RenderImage, ScrollHandle, SharedString,
    UniformListScrollHandle,
};
use native_gpui::sidebar::NativeSidebar;
use objc2::rc::Retained;
use objc2_app_kit::NSWindow;

use crate::art::Anchor;
use crate::chrome::scope_bar;
use crate::chrome::sheet::{ConnectSheet, GoSheet};
use crate::chrome::sidebar;
use crate::data::{Column, Folders, Line, Row};
use crate::dragdrop::Spring;
use crate::listing::{Keys, Listings};
use crate::ops::{conflicts, jobs};
use crate::preview::{self, INSET, PREVIEW_W, PreviewWindow};
use crate::view::LIST_VIEW;
use crate::views::header::{ColumnSpec, HeaderDrag, NAME_MIN_W, default_columns};
use crate::views::icon::Grid;
use crate::views::list::{ICON_SIZE, ICON_SMALL};
use crate::{prefs, read};

pub(crate) const LIST_CONTEXT: &str = "FinderList";

pub(crate) type Stack = (Vec<SharedString>, u32, Arc<RenderImage>);

pub(crate) struct FinderDemo {
    pub(crate) folders: Folders,
    pub(crate) sidebar: Option<NativeSidebar>,
    pub(crate) sidebar_groups: Vec<sidebar::Group>,
    pub(crate) sidebar_rows: Vec<Vec<&'static str>>,
    pub(crate) toolbar: bool,
    pub(crate) toolbar_for: (u64, Option<SharedString>),
    pub(crate) history: Vec<&'static str>,
    pub(crate) at: usize,
    pub(crate) view: usize,
    pub(crate) sort: Column,
    pub(crate) ascending: bool,
    pub(crate) list_columns: Vec<ColumnSpec>,
    pub(crate) name_w: f32,
    pub(crate) header_drag: Option<HeaderDrag>,
    pub(crate) header_press: Option<f32>,
    pub(crate) query: String,
    pub(crate) titled: String,
    /// The query the rows on screen are filtered by; `query` is what has been typed since.
    pub(crate) filtered: String,
    pub(crate) criteria: Vec<crate::chrome::criteria_bar::Criterion>,
    pub(crate) scope: usize,
    pub(crate) expanded: Keys,
    pub(crate) expanding: HashMap<&'static str, usize>,
    pub(crate) selected: Keys,
    pub(crate) cursor: Option<SharedString>,
    pub(crate) anchor: Option<SharedString>,
    pub(crate) renaming: Option<SharedString>,
    pub(crate) menu_row: Option<SharedString>,
    pub(crate) cut: Keys,
    pub(crate) cache: RefCell<Listings>,
    pub(crate) stats: Option<preview::Stats>,
    pub(crate) stats_pending: Option<(u64, usize)>,
    pub(crate) followed: Option<(u64, &'static str, &'static str, usize)>,
    pub(crate) stack: RefCell<Option<Stack>>,
    pub(crate) typed: String,
    pub(crate) typed_at: Option<Instant>,
    pub(crate) type_filter: Option<String>,
    pub(crate) type_filter_seen_match: bool,
    pub(crate) tags: HashMap<SharedString, Vec<usize>>,
    pub(crate) preview: bool,
    pub(crate) preview_w: f32,
    pub(crate) preview_window: Option<PreviewWindow>,
    pub(crate) preview_opening: bool,
    pub(crate) preview_expanded: bool,
    pub(crate) list_focused: bool,
    pub(crate) list_room: f32,
    pub(crate) pane_h: f32,
    pub(crate) list_span: f32,
    pub(crate) path_bar_shown: bool,
    pub(crate) status_bar_shown: bool,
    pub(crate) show_hidden: bool,
    pub(crate) grouped: bool,
    pub(crate) spring: Option<Spring>,
    pub(crate) recents: Vec<(&'static str, String)>,
    pub(crate) go_sheet: Option<GoSheet>,
    pub(crate) connect_sheet: Option<ConnectSheet>,
    pub(crate) save_sheet: Option<String>,
    pub(crate) ql_icon: Anchor,
    pub(crate) tag_editor: Option<crate::tag_editor::TagEditor>,
    pub(crate) ql_shown: Option<(SharedString, Option<Bounds<Pixels>>)>,
    pub(crate) icon_size: f64,
    pub(crate) list_icon: f32,
    pub(crate) list_zoom: Option<(f32, f32, Instant)>,
    pub(crate) strip_icon: f32,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) hscroll: ScrollHandle,
    pub(crate) strip: ScrollHandle,
    pub(crate) column_scroll: ScrollHandle,
    pub(crate) column_depth: usize,
    pub(crate) column_reveal: Option<(f32, std::time::Instant)>,
    pub(crate) column_widths: HashMap<&'static str, f32>,
    pub(crate) column_scrolls: Vec<UniformListScrollHandle>,
    pub(crate) column_grab: Option<(&'static str, f32)>,
    pub(crate) icons: Grid,
    pub(crate) scale: f32,
    pub(crate) window_min_w: f32,
    pub(crate) watcher: Watcher,
    pub(crate) watching: HashSet<&'static str>,
    pub(crate) hits: Vec<Arc<Row>>,
    pub(crate) hits_rev: u64,
    pub(crate) tag_listing: Option<&'static str>,
    pub(crate) inspecting: Option<Line>,
    pub(crate) reveal_next: Option<SharedString>,
    pub(crate) vanished: Option<&'static str>,
    pub(crate) host: Option<Retained<NSWindow>>,
    pub(crate) focus: FocusHandle,
    pub(crate) frames: u64,
    pub(crate) quit_at: Option<u64>,
    pub(crate) queue: Queue,
    pub(crate) journal: Journal,
    pub(crate) queue_events: Arc<Mutex<Vec<queue::Event>>>,
    pub(crate) conflict: Option<conflicts::Sheet>,
    pub(crate) jobs: Vec<jobs::JobRow>,
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
            titled: String::new(),
            criteria: vec![Default::default()],
            folders: Folders::default(),
            sidebar: None,
            sidebar_rows: sidebar::stored_rows(&groups),
            sidebar_groups: groups,
            toolbar: false,
            toolbar_for: (0, None),
            history: vec![start],
            at: 0,
            view: prefs::view().unwrap_or(LIST_VIEW),
            sort: Column::Name,
            ascending: true,
            list_columns: default_columns(),
            name_w: NAME_MIN_W,
            header_drag: None,
            header_press: None,
            query: String::new(),
            filtered: String::new(),
            scope: prefs::search_scope().unwrap_or(scope_bar::THIS_FOLDER),
            expanded: Keys::default(),
            expanding: HashMap::new(),
            selected: Keys::default(),
            cursor: None,
            anchor: None,
            renaming: None,
            menu_row: None,
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
            pane_h: 0.,
            list_span: f32::NAN,
            path_bar_shown: true,
            status_bar_shown: false,
            show_hidden: false,
            grouped: false,
            spring: None,
            recents: Vec::new(),
            go_sheet: None,
            connect_sheet: None,
            save_sheet: None,
            ql_icon: Anchor::default(),
            tag_editor: None,
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
            column_reveal: None,
            column_widths: HashMap::new(),
            column_scrolls: Vec::new(),
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
            vanished: None,
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

    pub(crate) fn warm_icons(&self) {
        crate::icons::draw_stand_ins(ICON_SMALL);
        for kind in crate::icons::STAND_INS {
            for size in [ICON_SIZE, PREVIEW_W - 1. - 2. * INSET] {
                crate::icons::icon(kind, size);
            }
        }
    }
}
