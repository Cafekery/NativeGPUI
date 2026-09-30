use std::borrow::Cow;
use std::cell::RefCell;
use std::iter;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{App, Context, SharedString, Window};

use crate::app::FinderDemo;
use crate::art::Previews;
use crate::data::{Folder, Line, intern, intern_path, parent_of, split_key};
use crate::menus::{self, Back, Forward};
use crate::tags::tag_title;
use crate::view::{COLUMN_VIEW, LIST_VIEW};
use crate::views::column::ColumnLook;
use crate::views::gallery::GalleryLook;
use crate::views::list::ListLook;
use crate::{home, open_finder_window};

const HOLD: Duration = Duration::from_millis(100);

pub(crate) struct Arriving {
    history: Vec<&'static str>,
    at: usize,
    recent: bool,
    chosen: Option<Vec<SharedString>>,
}

impl Arriving {
    fn id(&self) -> &'static str {
        self.history[self.at]
    }
}

pub(crate) fn arrive_on_keys(cx: &mut App) {
    cx.intercept_keystrokes(|_, window, cx| {
        if let Some(Some(demo)) = window.root::<FinderDemo>() {
            demo.update(cx, |demo, cx| demo.arrive_now(cx));
        }
    })
    .detach();
}

pub(crate) fn typed_path(text: &str) -> PathBuf {
    let text = text.trim();
    let full = match text.strip_prefix('~') {
        Some(rest) => format!("{}{rest}", home().to_string_lossy()),
        None => text.to_string(),
    };
    let trimmed = full.trim_end_matches('/');
    PathBuf::from(match trimmed.is_empty() {
        true => "/",
        false => trimmed,
    })
}

fn display_name(path: &'static str) -> String {
    thread_local! {
        static NAMED: RefCell<Option<(&'static str, String)>> = const { RefCell::new(None) };
    }
    NAMED.with_borrow_mut(|named| match named {
        Some((at, name)) if *at == path => name.clone(),
        _ => named
            .insert((path, finder_fs::name::of_path(Path::new(path))))
            .1
            .clone(),
    })
}

/// The index keys the bytes on disk, which `realpath` gives, not the typed form; a symlink stays.
pub(crate) fn folder_at_path(text: &str) -> Option<&'static str> {
    let path = typed_path(text);
    if !finder_fs::list::is_dir(&path) {
        return None;
    }
    let fold = |path: &Path| finder_fs::sort::fold(&path.to_string_lossy());
    let stored = std::fs::canonicalize(&path)
        .ok()
        .filter(|real| fold(real) == fold(&path));
    Some(intern_path(stored.as_deref().unwrap_or(&path)))
}

fn enclosing(id: &'static str) -> Option<(&'static str, SharedString)> {
    Some((parent_of(id)?, SharedString::from(id)))
}

fn refusal(id: &str, error: &std::io::Error) -> String {
    let name = Path::new(id)
        .file_name()
        .map_or_else(|| id.into(), |name| name.to_string_lossy());
    let name = finder_fs::name::display(&name);
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => format!(
            "The folder \u{201c}{name}\u{201d} can\u{2019}t be opened because you don\u{2019}t have permission to see its contents."
        ),
        _ => {
            let why = error.to_string();
            let why = why.split(" (os error").next().unwrap_or_default();
            format!("The folder \u{201c}{name}\u{201d} can\u{2019}t be opened. {why}.")
        }
    }
}

impl FinderDemo {
    pub(crate) fn here(&self) -> &'static str {
        self.history[self.at]
    }

    pub(crate) fn folder(&self) -> Option<&Folder> {
        self.folders.get(self.here())
    }

    pub(crate) fn target(&self) -> &'static str {
        let here = self.here();
        let Some(key) = self.cursor.as_ref() else {
            return here;
        };
        if self.results() {
            return split_key(key).map_or(here, |(holder, _)| holder);
        }
        if self.view != COLUMN_VIEW {
            return here;
        }
        let Some(holder) = self.column_holder(key) else {
            return here;
        };
        if self.selected.len() > 1 {
            return holder;
        }
        self.column_row(holder, key)
            .filter(|row| row.readable)
            .and_then(|row| row.opens)
            .unwrap_or(holder)
    }

    pub(crate) fn title(&self) -> Cow<'static, str> {
        if self.in_search() || self.search_editing {
            return format!("Searching \u{201c}{}\u{201d}", self.scope_name()).into();
        }
        if let Some(tag) = self.tag_listing {
            return tag_title(tag).into();
        }
        let target = self.target();
        if crate::prefs::finder().posix_title && target.starts_with('/') {
            return finder_fs::name::display(target);
        }
        if let Some((title, _)) = crate::locations::place(target) {
            return title.into();
        }
        match self.folders.get(target) {
            Some(folder) => folder.title.into(),
            None => display_name(target).into(),
        }
    }

    pub(crate) fn retitle(&self, window: &mut Window) {
        window.set_window_title(&self.title());
    }

    /// The folders column view shows, from the target's volume down to it.
    pub(crate) fn column_ids(&self) -> Vec<&'static str> {
        let mut chain: Vec<&'static str> = iter::successors(Some(self.target()), |id| {
            parent_of(id).filter(|&up| up != crate::locations::COMPUTER)
        })
        .collect();
        chain.reverse();
        chain
    }

    fn remember_recent(&mut self, id: &'static str) {
        let title = crate::data::folder_of(id).title.to_string();
        self.recents.retain(|(seen, _)| *seen != id);
        self.recents.insert(0, (id, title));
        self.recents.truncate(10);
        native_gpui::set_menu("Go", menus::go_rows(&self.recents));
    }

    pub(crate) fn go(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.visit(id, true, window, cx);
    }

    fn visit(&mut self, id: &'static str, hold: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(listing) = crate::smart::listing(id) {
            self.show_tag(listing, window, cx);
            return;
        }
        if self.here() == id {
            self.arriving = None;
            if self.results() {
                self.arrive(window, cx);
            }
            return;
        }
        let mut history = self.history[..=self.at].to_vec();
        history.push(id);
        let at = history.len() - 1;
        let arriving = Arriving {
            history,
            at,
            recent: true,
            chosen: None,
        };
        self.travel(arriving, hold, window, cx);
    }

    fn travel(
        &mut self,
        arriving: Arriving,
        hold: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = arriving.id();
        if hold && self.holds_for(id) {
            self.arriving = Some(arriving);
            self.read_folder(id, cx);
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(HOLD).await;
                this.update(cx, |this, cx| {
                    if this.arrived(id, cx) {
                        this.settle();
                    }
                })
                .ok();
            })
            .detach();
            return;
        }
        self.land(arriving);
        self.arrive(window, cx);
    }

    fn land(&mut self, arriving: Arriving) -> Option<Vec<SharedString>> {
        let id = arriving.id();
        self.history = arriving.history;
        self.at = arriving.at;
        if arriving.recent {
            self.remember_recent(id);
        }
        arriving.chosen
    }

    fn holds_for(&self, id: &'static str) -> bool {
        id.starts_with('/')
            && (!self.folders.has(id)
                || self
                    .folders
                    .get(id)
                    .is_some_and(|folder| folder.loading && folder.rows.is_empty())
                || self.order_for(id) != self.order())
    }

    pub(crate) fn headed_for(&self) -> Option<&'static str> {
        self.arriving.as_ref().map(Arriving::id)
    }

    pub(crate) fn arrived(&mut self, id: &'static str, cx: &mut Context<Self>) -> bool {
        let Some(arriving) = self.arriving.take_if(|arriving| arriving.id() == id) else {
            return false;
        };
        let chosen = self.land(arriving);
        self.enter(cx);
        if let Some(keys) = chosen {
            self.choose(keys);
        }
        true
    }

    pub(crate) fn arrive_now(&mut self, cx: &mut Context<Self>) {
        if self.headed_for().is_some_and(|id| self.arrived(id, cx)) {
            self.settle();
        }
    }

    pub(crate) fn show_enclosing(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((parent, key)) = enclosing(id) else {
            return;
        };
        self.go(parent, window, cx);
        self.choose([key]);
    }

    pub(crate) fn enclosing_in_new_window(&self, cx: &mut Context<Self>) {
        let Some((parent, key)) = enclosing(self.here()) else {
            return;
        };
        cx.defer(move |cx| {
            open_finder_window(parent, 1, cx)
                .update(cx, |demo: &mut FinderDemo, _, _| demo.choose([key]))
                .ok();
        });
    }

    pub(crate) fn choose(&mut self, keys: impl IntoIterator<Item = SharedString>) {
        if let Some(arriving) = &mut self.arriving {
            arriving.chosen = Some(keys.into_iter().collect());
            return;
        }
        let mut last = None;
        self.selected = keys
            .into_iter()
            .inspect(|key| last = Some(key.clone()))
            .collect();
        self.cursor = last.clone();
        self.anchor = last.clone();
        self.reveal_next = last;
    }

    fn forget_under(&mut self, id: &'static str) -> bool {
        let inside = |path: &str| {
            path == id
                || path
                    .strip_prefix(id.trim_end_matches('/'))
                    .is_some_and(|rest| rest.starts_with('/'))
        };
        let leaving = inside(self.here());
        self.folders.forget(id);
        self.expanded.retain(|key| !inside(key));
        self.expanding.retain(|held, _| !inside(held));
        let before = self.history[..self.at]
            .iter()
            .filter(|been| inside(been))
            .count();
        self.history.retain(|been| !inside(been));
        self.at -= before;
        leaving
    }

    pub(crate) fn folder_gone(
        &mut self,
        id: &'static str,
        above: &'static str,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
        self.arrive_now(cx);
        if !self.forget_under(id) {
            return;
        }
        if self.history.is_empty() {
            self.history.push(above);
        }
        self.at = self.at.min(self.history.len() - 1);
        self.vanished = Some(above);
    }

    pub(crate) fn folder_renamed(
        &mut self,
        id: &'static str,
        now: &'static str,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
        self.arrive_now(cx);
        let moved = |path: &'static str| match path.strip_prefix(id) {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                intern(&format!("{now}{rest}"))
            }
            _ => path,
        };
        let leaving = moved(self.here()) != self.here();
        self.folders.forget(id);
        self.expanded
            .retain(|key| !Path::new(key.as_ref()).starts_with(id));
        self.history = self.history.iter().map(|&been| moved(been)).collect();
        if leaving {
            self.vanished = Some(self.here());
        }
    }

    pub(crate) fn folder_refused(
        &mut self,
        id: &'static str,
        error: &std::io::Error,
        cx: &mut Context<Self>,
    ) {
        if self.headed_for() == Some(id) {
            self.arriving = None;
            self.folders.forget(id);
            self.refuse(refusal(id, error), cx);
            return;
        }
        if id != self.here() {
            return;
        }
        self.recents.retain(|(seen, _)| *seen != id);
        native_gpui::set_menu("Go", menus::go_rows(&self.recents));
        self.folders.forget(id);
        self.history.remove(self.at);
        match self.at.checked_sub(1) {
            Some(back) => self.at = back,
            None => self
                .history
                .insert(0, Path::new(id).parent().map_or("/", intern_path)),
        }
        self.vanished = Some(self.here());
        self.reveal_next = Some(SharedString::from(id));
        cx.notify();
        self.refuse(refusal(id, error), cx);
    }

    pub(crate) fn settle_vanished(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.vanished.take() else {
            return;
        };
        if self.here() != id {
            self.visit(id, false, window, cx);
            return;
        }
        let chosen = self.reveal_next.take();
        self.arrive(window, cx);
        if let Some(key) = chosen {
            self.choose([key]);
        }
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
        let arriving = Arriving {
            history: self.history.clone(),
            at: next,
            recent: false,
            chosen: None,
        };
        self.travel(arriving, true, window, cx);
    }

    fn arrive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.arriving = None;
        self.enter(cx);
        self.read_folder(self.here(), cx);
        self.settle();
        self.retitle(window);
    }

    fn enter(&mut self, cx: &mut Context<Self>) {
        self.deselect();
        self.query.clear();
        self.filtered.clear();
        self.hits.clear();
        self.tag_listing = None;
        self.icons.forget();
        self.clear_type_filter();
        let here = self.here();
        self.list_look = ListLook::of(here);
        self.load_columns();
        self.columns_from_computer = here == crate::locations::COMPUTER;
        self.column_look = ColumnLook::load();
        self.gallery_look = GalleryLook::load(here);
        self.load_sorting(cx);
        self.sidebar_follow();
        self.previews = Previews::of(here);
        self.open_as_kept(cx);
        cx.notify();
    }

    pub(crate) fn open(&mut self, line: &Line, window: &mut Window, cx: &mut Context<Self>) {
        if line.row.opens.is_some() && !line.row.readable {
            let denied = std::io::ErrorKind::PermissionDenied.into();
            self.refuse(refusal(&line.key, &denied), cx);
        } else if let Some(id) = line.row.opens {
            self.go(id, window, cx);
        } else if crate::smart::is_saved(&line.key) {
            self.go(intern(&line.key), window, cx);
        } else if crate::locations::server(&line.key).is_some() {
            self.connect_server(&line.key, window, cx);
        } else {
            self.open_files(vec![PathBuf::from(line.key.as_ref())], cx);
        }
    }

    // Off the main thread: a file on a share that stopped answering would stall it.
    fn open_files(&self, files: Vec<PathBuf>, cx: &mut Context<Self>) {
        if files.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let (folder, lost) = cx
                .background_executor()
                .spawn(async move {
                    let (found, lost): (Vec<PathBuf>, Vec<PathBuf>) = files
                        .into_iter()
                        .partition(|file| std::fs::metadata(file).is_ok());
                    let mut folder = None;
                    let mut documents = Vec::with_capacity(found.len());
                    for file in found {
                        match crate::openwith::alias_folder(&file) {
                            Some(target) => folder = folder.or(Some(target)),
                            None => documents.push(file),
                        }
                    }
                    crate::openwith::open_default(&documents);
                    let lost = lost
                        .into_iter()
                        .find(|link| std::fs::symlink_metadata(link).is_ok());
                    (folder, lost)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(folder) = folder {
                    this.vanished = Some(intern_path(&folder));
                    cx.notify();
                }
                let Some(link) = lost else {
                    return;
                };
                let name = link.file_name().unwrap_or_default().to_string_lossy();
                this.complain(&format!(
                    "The operation can\u{2019}t be completed because the original item for \u{201c}{}\u{201d} can\u{2019}t be found.",
                    finder_fs::name::display(&name)
                ));
            })
            .ok();
        })
        .detach();
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
        if self.headed_for().is_some() {
            self.choose([SharedString::from(from)]);
            return;
        }
        let listing = self.lines(self.view == LIST_VIEW);
        match listing.iter().find(|line| line.row.opens == Some(from)) {
            Some(line) => {
                self.pick(line, false, false);
                self.reveal(&line.key);
            }
            None => self.choose([SharedString::from(from)]),
        }
    }

    pub(crate) fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = self.chosen_lines();
        for line in chosen.iter().filter(|line| line.row.dataless) {
            let _ = finder_fs::icloud::download(Path::new(line.key.as_ref()));
        }
        let files = chosen
            .iter()
            .filter(|line| line.row.opens.is_none() && !crate::smart::is_saved(&line.key))
            .map(|line| PathBuf::from(line.key.as_ref()))
            .collect();
        self.open_files(files, cx);
        let folders: Vec<&'static str> = chosen
            .iter()
            .filter(|line| line.row.readable)
            .filter_map(|line| line.row.opens)
            .collect();
        let Some((first, rest)) = folders.split_first() else {
            if let Some(saved) = chosen.iter().find(|line| crate::smart::is_saved(&line.key)) {
                self.go(intern(&saved.key), window, cx);
            }
            return;
        };
        self.go(first, window, cx);
        for (extra, id) in rest.iter().enumerate() {
            open_finder_window(id, extra + 1, cx);
        }
    }

    pub(crate) fn back(&mut self, _: &Back, window: &mut Window, cx: &mut Context<Self>) {
        self.step(false, window, cx);
    }

    pub(crate) fn forward(&mut self, _: &Forward, window: &mut Window, cx: &mut Context<Self>) {
        self.step(true, window, cx);
    }
}
