use std::borrow::Cow;
use std::cell::RefCell;
use std::path::{Path, PathBuf};

use gpui::{Context, SharedString, Window};

use crate::app::FinderDemo;
use crate::data::{Folder, Line, intern, intern_path, split_key, volume_at};
use crate::menus::{self, Back, Forward};
use crate::tags::tag_title;
use crate::view::{COLUMN_VIEW, LIST_VIEW};
use crate::{home, open_finder_window};

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

/// A typed name is NFC and any case, but the index keys the bytes on disk, which `realpath`
/// gives; a symlink stays as typed.
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
    match Path::new(id).parent() {
        Some(parent) => Some((intern_path(parent), SharedString::from(id))),
        None => volume_at(id).map(|volume| {
            (
                intern("/Volumes"),
                SharedString::from(format!("/Volumes/{}", volume.name)),
            )
        }),
    }
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
            .filter(|row| row.readable)
            .and_then(|row| row.opens)
            .unwrap_or(holder)
    }

    pub(crate) fn crumb_path(&self) -> String {
        self.target().to_string()
    }

    pub(crate) fn title(&self) -> Cow<'static, str> {
        if self.in_search() {
            return format!("Searching \u{201c}{}\u{201d}", self.scope_name()).into();
        }
        match self.tag_listing.map(tag_title) {
            Some(title) => title.into(),
            None if crate::prefs::finder().posix_title => finder_fs::name::display(self.target()),
            None => match self.folders.get(self.target()) {
                Some(folder) => folder.title.into(),
                None => display_name(self.target()).into(),
            },
        }
    }

    pub(crate) fn retitle(&self, window: &mut Window) {
        window.set_window_title(&self.title());
    }

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

    fn remember_recent(&mut self, id: &'static str) {
        let title = crate::data::folder_of(id).title.to_string();
        self.recents.retain(|(seen, _)| *seen != id);
        self.recents.insert(0, (id, title));
        self.recents.truncate(10);
        native_gpui::set_menu("Go", menus::go_rows(&self.recents));
    }

    pub(crate) fn go(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(listing) = crate::smart::listing(id) {
            self.show_tag(listing, window, cx);
            return;
        }
        if self.here() == id {
            if self.results() {
                self.arrive(window, cx);
            }
            return;
        }
        self.history.truncate(self.at + 1);
        self.history.push(id);
        self.at = self.history.len() - 1;
        self.remember_recent(id);
        self.arrive(window, cx);
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

    /// Finder won't open a folder it can't list: it says why and stays where it was, with the
    /// folder still selected.
    pub(crate) fn folder_refused(
        &mut self,
        id: &'static str,
        error: &std::io::Error,
        cx: &mut Context<Self>,
    ) {
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

    /// A folder that went away, or would not open, hands the window back to one that is there.
    pub(crate) fn settle_vanished(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.vanished.take() else {
            return;
        };
        if self.here() != id {
            self.go(id, window, cx);
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
        self.at = next;
        self.arrive(window, cx);
    }

    pub(crate) fn arrive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected.clear();
        self.cursor = None;
        self.anchor = None;
        self.query.clear();
        self.filtered.clear();
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

    pub(crate) fn open(&mut self, line: &Line, window: &mut Window, cx: &mut Context<Self>) {
        if line.row.opens.is_some() && !line.row.readable {
            let denied = std::io::ErrorKind::PermissionDenied.into();
            self.refuse(refusal(&line.key, &denied), cx);
        } else if let Some(id) = line.row.opens {
            self.go(id, window, cx);
        } else if crate::smart::is_saved(&line.key) {
            self.go(intern(&line.key), window, cx);
        } else {
            self.open_files(vec![PathBuf::from(line.key.as_ref())], cx);
        }
    }

    /// Off the main thread, which a file on a share that stopped answering would stall.
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
