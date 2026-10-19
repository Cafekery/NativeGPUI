use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use finder_fs::queue::{self, Journal, Queue};
use finder_fs::watch::Watcher;
use gpui::{
    Bounds, Context, Entity, FocusHandle, Pixels, RenderImage, ScrollHandle, SharedString, Size,
    UniformListScrollHandle, WindowHandle,
};
use native_gpui::sidebar::NativeSidebar;
use objc2::rc::Retained;
use objc2_app_kit::NSWindow;
use parking_lot::Mutex;

use crate::art::{Anchor, Previews};
use crate::chrome::sheet::GoPanel;
use crate::chrome::sidebar;
use crate::data::{Column, Folders, Order, Row};
use crate::dragdrop::Spring;
use crate::elastic::Bands;
use crate::listing::{Keys, Listings};
use crate::ops::{conflicts, jobs};
use crate::preview::{self, INSET, PREVIEW_W, PreviewWindow};
use crate::terms::Search;
use crate::view::{COLUMN_VIEW, GALLERY_VIEW, ICON_VIEW, LIST_VIEW};
use crate::views::column::{ColumnFit, ColumnLook};
use crate::views::gallery::GalleryLook;
use crate::views::header::{ColumnSpec, HeaderBand, HeaderDrag, columns_for};
use crate::views::icon::Grid;
use crate::views::list::{Faces, ICON_SIZE, ICON_SMALL, ListLook};
use crate::{prefs, read};

pub(crate) const LIST_CONTEXT: &str = "FinderList";

const SIDEBAR_POLL: Duration = Duration::from_millis(100);

thread_local! {
    pub(crate) static EARLY_SIDEBAR: RefCell<Option<Receiver<Vec<sidebar::Group>>>> =
        const { RefCell::new(None) };
}

/// A share that stopped answering stalls whoever asks the sidebar's rows, so no window waits.
pub(crate) fn ask_sidebar() -> Receiver<Vec<sidebar::Group>> {
    let (sent, answer) = std::sync::mpsc::channel();
    std::thread::spawn(move || sent.send(sidebar::groups()).ok());
    answer
}

pub(crate) struct FinderDemo {
    pub(crate) folders: Folders,
    pub(crate) sidebar: Option<NativeSidebar>,
    pub(crate) sidebar_groups: Vec<sidebar::Group>,
    pub(crate) sidebar_rows: Vec<Vec<&'static str>>,
    pub(crate) closed_sections: Vec<&'static str>,
    pub(crate) sidebar_hidden: bool,
    pub(crate) toolbar_for: Option<crate::chrome::toolbar::ToolbarState>,
    pub(crate) services_for: u64,
    pub(crate) history: Vec<&'static str>,
    pub(crate) at: usize,
    pub(crate) view: usize,
    /// The held rows' order: the shown view's, or the pending one's.
    pub(crate) sort: Column,
    pub(crate) ascending: bool,
    pub(crate) banded: Option<Column>,
    pub(crate) sorting: Sorting,
    pub(crate) column_sort: Column,
    pub(crate) pending_view: Option<usize>,
    pub(crate) list_columns: Vec<ColumnSpec>,
    pub(crate) name_w: f32,
    pub(crate) header_drag: Option<HeaderDrag>,
    pub(crate) header: Option<Entity<HeaderBand>>,
    pub(crate) header_press: Option<f32>,
    pub(crate) query: Search,
    /// Spotlight's query for `query`, as Finder's field writes it.
    pub(crate) raw_query: Option<String>,
    pub(crate) search_editing: bool,
    pub(crate) titled: String,
    /// The query the rows on screen are filtered by; `query` is what has been typed since.
    pub(crate) filtered: Search,
    pub(crate) criteria: Vec<crate::chrome::criteria_bar::Criterion>,
    pub(crate) scope: usize,
    pub(crate) expanded: Keys,
    pub(crate) expanding: HashMap<&'static str, usize>,
    pub(crate) selected: Keys,
    pub(crate) cursor: Option<SharedString>,
    pub(crate) anchor: Option<SharedString>,
    pub(crate) renaming: Option<SharedString>,
    pub(crate) menu_row: Option<SharedString>,
    pub(crate) cache: RefCell<Listings>,
    pub(crate) stats: Option<preview::Stats>,
    pub(crate) stats_pending: Option<(u64, usize)>,
    pub(crate) stats_stale: Arc<AtomicBool>,
    pub(crate) followed: Option<(u64, &'static str, &'static str, usize)>,
    pub(crate) stack: RefCell<Option<Arc<RenderImage>>>,
    pub(crate) typed: String,
    pub(crate) typed_at: Option<Instant>,
    pub(crate) type_filter: Option<String>,
    pub(crate) type_filter_seen_match: bool,
    pub(crate) preview: bool,
    pub(crate) preview_w: f32,
    /// Whether `preview_w` is gallery view's width.
    pub(crate) preview_gallery: bool,
    pub(crate) preview_window: Option<PreviewWindow>,
    pub(crate) preview_opening: bool,
    pub(crate) preview_expanded: bool,
    pub(crate) list_focused: bool,
    pub(crate) list_room: f32,
    pub(crate) pane_h: f32,
    pub(crate) list_span: f32,
    pub(crate) path_bar_shown: bool,
    pub(crate) crumb_drop: Option<usize>,
    pub(crate) status_bar_shown: bool,
    pub(crate) browse_view: usize,
    pub(crate) spring: Option<Spring>,
    pub(crate) pressed_at: gpui::Point<Pixels>,
    pub(crate) recents: Vec<(&'static str, String)>,
    pub(crate) go_sheet: Option<WindowHandle<GoPanel>>,
    pub(crate) save_sheet: Option<String>,
    pub(crate) ql_icon: Anchor,
    pub(crate) faces: Rc<RefCell<Faces>>,
    pub(crate) tag_editor: Option<crate::tag_editor::TagEditor>,
    pub(crate) customizer: Option<crate::customize::Customizer>,
    pub(crate) ql_shown: Option<(SharedString, Option<Bounds<Pixels>>)>,
    pub(crate) ql_fetch: Option<u64>,
    pub(crate) list_look: ListLook,
    pub(crate) list_zoom: Option<(f32, f32, Instant)>,
    pub(crate) gallery_look: GalleryLook,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) hscroll: ScrollHandle,
    pub(crate) strip: ScrollHandle,
    pub(crate) column_scroll: ScrollHandle,
    pub(crate) column_depth: usize,
    pub(crate) column_reveal: Option<(f32, Instant)>,
    /// The selection the browser last laid its columns out for.
    pub(crate) column_cursor: Option<SharedString>,
    pub(crate) columns_from_computer: bool,
    pub(crate) column_widths: HashMap<&'static str, Option<f32>>,
    pub(crate) column_scrolls: Vec<UniformListScrollHandle>,
    pub(crate) column_grab: Option<(&'static str, f32)>,
    pub(crate) column_look: ColumnLook,
    pub(crate) column_fits: HashMap<&'static str, ColumnFit>,
    pub(crate) icons: Grid,
    pub(crate) bands: Bands,
    pub(crate) previews: Previews,
    pub(crate) scale: f32,
    pub(crate) viewport: Size<Pixels>,
    pub(crate) window_min_w: f32,
    pub(crate) watcher: Watcher,
    pub(crate) watching: HashSet<&'static str>,
    pub(crate) hits: Vec<Arc<Row>>,
    pub(crate) hits_rev: u64,
    pub(crate) search_gen: Arc<AtomicU64>,
    pub(crate) tag_listing: Option<&'static str>,
    pub(crate) reveal_next: Option<SharedString>,
    pub(crate) vanished: Option<&'static str>,
    pub(crate) arriving: Option<crate::navigate::Arriving>,
    pub(crate) host: Option<Retained<NSWindow>>,
    pub(crate) focus: FocusHandle,
    pub(crate) frames: u64,
    pub(crate) quit_at: Option<u64>,
    pub(crate) queue: Queue,
    pub(crate) journal: Journal,
    pub(crate) queue_events: Arc<Mutex<Vec<queue::Event>>>,
    pub(crate) conflict: Option<conflicts::Sheet>,
    pub(crate) jobs: Vec<jobs::JobRow>,
    pub(crate) remote_jobs: crate::remote::Jobs,
    pub(crate) weighing: HashMap<&'static str, gpui::Task<()>>,
}

impl FinderDemo {
    pub(crate) fn new(start: &'static str, cx: &mut Context<Self>) -> Self {
        let answer = EARLY_SIDEBAR.take().unwrap_or_else(ask_sidebar);
        let groups = match answer.try_recv() {
            Ok(groups) => {
                sidebar::remember(&groups);
                groups
            }
            Err(_) => {
                cx.spawn(async move |this, cx| {
                    let groups = loop {
                        cx.background_executor().timer(SIDEBAR_POLL).await;
                        match answer.try_recv() {
                            Ok(groups) => break groups,
                            Err(TryRecvError::Empty) => continue,
                            Err(TryRecvError::Disconnected) => return,
                        }
                    };
                    this.update(cx, |this, cx| {
                        this.sidebar_took(groups);
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
                sidebar::last_groups()
            }
        };
        let changes: Arc<Mutex<(Vec<PathBuf>, Vec<PathBuf>)>> = Arc::default();
        let sink = changes.clone();
        read::follow_events(changes, cx);
        let queue_events: Arc<Mutex<Vec<queue::Event>>> = Arc::default();
        let queue_sink = queue_events.clone();
        let queue = Queue::new(move |event| {
            queue_sink.lock().push(event);
            read::EVENTS.ring();
        });
        let kept = prefs::opens_in(start);
        let browse_view = match kept {
            Some((view, true)) => view,
            _ => prefs::view().unwrap_or(LIST_VIEW),
        };
        let view = kept.map_or(browse_view, |(view, _)| view);
        let finder = prefs::finder();
        let mut this = Self {
            titled: String::new(),
            criteria: Vec::new(),
            folders: Folders::default(),
            sidebar: None,
            sidebar_rows: sidebar::stored_rows(&groups),
            closed_sections: sidebar::closed_sections(),
            sidebar_hidden: false,
            sidebar_groups: groups,
            toolbar_for: None,
            services_for: 0,
            history: vec![start],
            at: 0,
            view,
            sort: finder.sort.0,
            ascending: finder.sort.1,
            banded: None,
            sorting: Sorting::of(start),
            column_sort: prefs::arranged(COLUMN_VIEW).unwrap_or(finder.arranged.0),
            pending_view: None,
            list_columns: Vec::new(),
            name_w: 0.,
            header_drag: None,
            header: None,
            header_press: None,
            query: Search::default(),
            raw_query: None,
            search_editing: false,
            filtered: Search::default(),
            scope: prefs::search_scope(),
            expanded: Keys::default(),
            expanding: HashMap::new(),
            selected: Keys::default(),
            cursor: None,
            anchor: None,
            renaming: None,
            menu_row: None,
            cache: RefCell::default(),
            stats: None,
            stats_pending: None,
            stats_stale: Arc::default(),
            followed: None,
            stack: RefCell::default(),
            typed: String::new(),
            typed_at: None,
            type_filter: None,
            type_filter_seen_match: false,
            preview: prefs::preview(view),
            preview_w: prefs::pane_width(view == GALLERY_VIEW).unwrap_or(PREVIEW_W),
            preview_gallery: view == GALLERY_VIEW,
            preview_window: None,
            preview_opening: false,
            preview_expanded: prefs::preview_info_expanded(),
            list_focused: false,
            list_room: 0.,
            pane_h: 0.,
            list_span: f32::NAN,
            path_bar_shown: prefs::shows_path_bar(),
            crumb_drop: None,
            status_bar_shown: prefs::shows_status_bar(),
            browse_view,
            spring: None,
            pressed_at: gpui::Point::default(),
            recents: Vec::new(),
            go_sheet: None,
            save_sheet: None,
            ql_icon: Anchor::default(),
            faces: Rc::default(),
            tag_editor: None,
            customizer: None,
            ql_shown: None,
            ql_fetch: None,
            list_look: ListLook::of(start),
            list_zoom: None,
            gallery_look: GalleryLook::load(start),
            scroll: UniformListScrollHandle::new(),
            hscroll: ScrollHandle::new(),
            strip: ScrollHandle::new(),
            column_scroll: ScrollHandle::new(),
            column_depth: 0,
            column_reveal: None,
            column_cursor: None,
            columns_from_computer: start == crate::locations::COMPUTER,
            column_widths: HashMap::new(),
            column_scrolls: Vec::new(),
            column_grab: None,
            column_look: ColumnLook::load(),
            column_fits: HashMap::new(),
            icons: Grid::default(),
            bands: Bands::default(),
            previews: Previews::of(start),
            scale: 2.,
            viewport: Size::default(),
            window_min_w: f32::NAN,
            watcher: Watcher::new(move |dirs, items| {
                let mut changes = sink.lock();
                changes.0.extend(dirs);
                changes.1.extend(items);
                drop(changes);
                read::EVENTS.ring();
            }),
            watching: HashSet::new(),
            hits: Vec::new(),
            hits_rev: 0,
            search_gen: Arc::default(),
            tag_listing: None,
            reveal_next: None,
            vanished: None,
            arriving: None,
            host: None,
            focus: cx.focus_handle(),
            frames: 0,
            quit_at: env::var("NG_QUIT").ok().and_then(|v| v.parse().ok()),
            queue,
            journal: Journal::new(),
            queue_events,
            conflict: None,
            jobs: Vec::new(),
            remote_jobs: crate::remote::Jobs::default(),
            weighing: HashMap::new(),
        };
        this.set_order(this.view_sort(this.view));
        this.load_columns();
        this.read_folder(start, cx);
        this.draw_crumbs(start, cx);
        cx.observe_self(|this, cx| this.faces.borrow_mut().touch(&this.selected, cx))
            .detach();
        this
    }

    pub(crate) fn order(&self) -> Order {
        Order {
            group: self.banded,
            sort: self.sort,
            ascending: self.ascending,
        }
    }

    pub(crate) fn set_order(&mut self, order: Order) {
        (self.sort, self.ascending, self.banded) = (order.sort, order.ascending, order.group);
    }

    pub(crate) fn view_sort(&self, view: usize) -> Order {
        self.sorting.order(view, self.column_sort)
    }

    pub(crate) fn order_for(&self, folder: &'static str) -> Order {
        Sorting::of(folder).order(self.pending_view.unwrap_or(self.view), self.column_sort)
    }

    pub(crate) fn load_sorting(&mut self, cx: &mut Context<Self>) {
        self.sorting = Sorting::of(self.here());
        let wanted = self.view_sort(self.view);
        if self.pending_view.is_none() && wanted != self.order() {
            self.set_order(wanted);
            self.sort_held(wanted, cx);
        }
    }

    pub(crate) fn group(&self) -> Option<Column> {
        self.sorting.group.filter(|_| self.view != GALLERY_VIEW)
    }

    pub(crate) fn set_group(&mut self, group: Option<Column>, cx: &mut Context<Self>) {
        let here = self.here();
        prefs::set_sort_text(LIST_VIEW, here, "groupBy", group.map_or("none", Column::id));
        if let Some(group) = group {
            prefs::set_sort_text(LIST_VIEW, here, "lastGroupBy", group.id());
        }
        self.sorting.group = group;
        let wanted = self.view_sort(self.pending_view.unwrap_or(self.view));
        if wanted != self.order() {
            self.sort_held(wanted, cx);
        }
        cx.notify();
    }

    pub(crate) fn toggle_groups(&mut self, cx: &mut Context<Self>) {
        let group = self.sorting.group.is_none().then(|| {
            kept_column(LIST_VIEW, self.here(), "lastGroupBy").unwrap_or(prefs::finder().group)
        });
        self.set_group(group, cx);
    }

    pub(crate) fn wants_extras(&self) -> bool {
        self.extras_with(&self.list_columns)
    }

    pub(crate) fn extras_in(&self, folder: &str) -> bool {
        self.extras_with(&columns_for(folder))
    }

    fn extras_with(&self, columns: &[ColumnSpec]) -> bool {
        self.sort.extra() || columns.iter().any(|spec| spec.column.extra())
    }

    pub(crate) fn sort_by(&mut self, column: Column, cx: &mut Context<Self>) {
        if self.view == ICON_VIEW && self.arrangement().is_some() {
            return self.icon_sort(column, cx);
        }
        let turn = self.sort == column && !matches!(self.view, COLUMN_VIEW | GALLERY_VIEW);
        let ascending = if turn {
            !self.ascending
        } else {
            column.first_ascending()
        };
        self.resort((column, ascending), cx);
    }

    pub(crate) fn resort(&mut self, by: (Column, bool), cx: &mut Context<Self>) {
        let here = self.here();
        match self.view {
            COLUMN_VIEW => {
                self.column_sort = by.0;
                prefs::set_arranged(COLUMN_VIEW, by.0);
            }
            GALLERY_VIEW => {
                self.sorting.gallery = by.0;
                prefs::set_sort_text(GALLERY_VIEW, here, "arrangeBy", by.0.id());
            }
            _ => {
                self.sorting.list = by;
                prefs::set_sort_text(LIST_VIEW, here, "sortColumn", by.0.id());
                prefs::set_sort_on(LIST_VIEW, here, "ascending", by.1);
            }
        }
        self.sort_held(self.view_sort(self.view), cx);
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

#[derive(Clone, Copy)]
pub(crate) struct Sorting {
    list: (Column, bool),
    group: Option<Column>,
    gallery: Column,
}

impl Sorting {
    fn of(folder: &str) -> Self {
        let finder = prefs::finder();
        let list = kept_column(LIST_VIEW, folder, "sortColumn").map_or(finder.sort, |sort| {
            let ascending = prefs::look_on(LIST_VIEW, folder, "ascending");
            (sort, ascending.unwrap_or(sort.first_ascending()))
        });
        Self {
            list,
            group: kept_column(LIST_VIEW, folder, "groupBy"),
            gallery: kept_column(GALLERY_VIEW, folder, "arrangeBy")
                .or_else(|| prefs::arranged(GALLERY_VIEW))
                .unwrap_or(finder.arranged.1),
        }
    }

    fn order(self, view: usize, column: Column) -> Order {
        let (sort, ascending) = match view {
            COLUMN_VIEW => (column, column.first_ascending()),
            GALLERY_VIEW => (self.gallery, self.gallery.first_ascending()),
            _ => self.list,
        };
        Order {
            group: self.group.filter(|_| view == LIST_VIEW),
            sort,
            ascending,
        }
    }
}

fn kept_column(view: usize, folder: &str, key: &str) -> Option<Column> {
    prefs::look_text(view, folder, key)
        .and_then(|id| Column::from_id(&id))
        .filter(|column| column.offered_at(folder))
}
