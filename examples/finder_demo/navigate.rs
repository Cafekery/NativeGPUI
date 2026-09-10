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

/// The folder that shows `id`, and the key `id` is chosen by in it. The boot volume sits in no
/// folder: Finder reveals it among the volumes, the listing /Volumes is.
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

impl FinderDemo {
    pub(crate) fn here(&self) -> &'static str {
        self.history[self.at]
    }

    pub(crate) fn folder(&self) -> Option<&Folder> {
        self.folders.get(self.here())
    }

    /// The folder an operation lands in: where the cursor points, else the folder on screen.
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

    pub(crate) fn folder_at_path(&self, text: &str) -> Option<&'static str> {
        let path = typed_path(text);
        finder_fs::list::is_dir(&path).then(|| intern_path(&path))
    }

    pub(crate) fn crumb_path(&self) -> String {
        self.target().to_string()
    }

    /// A search takes the title over the way Finder's does, naming what it is scoped to rather than
    /// the folder the window still stands in.
    pub(crate) fn title(&self) -> String {
        if self.in_search() {
            return format!("Searching \u{201c}{}\u{201d}", self.scope_name());
        }
        match self.tag_listing.map(tag_title) {
            Some(title) => title.to_string(),
            None => finder_fs::name::display(&self.crumb_path()).into_owned(),
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

    /// Finder's Show in Enclosing Folder: the parent folder, with the crumb chosen in it.
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
        self.choose_revealed(key);
    }

    /// Finder's Enclosing Folder in New Window: the same, in a window of its own.
    pub(crate) fn enclosing_in_new_window(&self, cx: &mut Context<Self>) {
        let Some((parent, key)) = enclosing(self.here()) else {
            return;
        };
        cx.defer(move |cx| {
            open_finder_window(parent, 1, cx)
                .update(cx, |demo: &mut FinderDemo, _, _| demo.choose_revealed(key))
                .ok();
        });
    }

    fn choose_revealed(&mut self, key: SharedString) {
        self.selected = std::iter::once(key.clone()).collect();
        self.cursor = Some(key.clone());
        self.anchor = Some(key.clone());
        self.reveal_next = Some(key);
    }

    /// Finder drops the listing of a folder deleted under the window and shows the nearest folder
    /// above it that is still there.
    pub(crate) fn folder_gone(&mut self, id: &'static str, cx: &mut Context<Self>) {
        cx.notify();
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
        self.history.retain(|been| !inside(been));
        if !leaving {
            return;
        }
        let above = Path::new(id)
            .ancestors()
            .skip(1)
            .find(|up| up.is_dir())
            .map_or("/", intern_path);
        if self.history.is_empty() {
            self.history.push(above);
        }
        self.at = self.at.min(self.history.len() - 1);
        self.vanished = Some(above);
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
        if let Some(id) = line.row.opens {
            self.go(id, window, cx);
        } else if crate::smart::is_saved(&line.key) {
            self.go(intern(&line.key), window, cx);
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

    pub(crate) fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.view == COLUMN_VIEW {
            return;
        }
        let chosen: Vec<crate::data::Line> = self
            .lines(self.view == LIST_VIEW)
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .cloned()
            .collect();
        // Finder opens what iCloud has evicted by asking for it back.
        for line in chosen.iter().filter(|line| line.row.dataless) {
            let _ = finder_fs::icloud::download(Path::new(line.key.as_ref()));
        }
        let folders: Vec<&'static str> = chosen.iter().filter_map(|line| line.row.opens).collect();
        if folders.is_empty() {
            if let Some(saved) = chosen.iter().find(|line| crate::smart::is_saved(&line.key)) {
                self.go(intern(&saved.key), window, cx);
            }
            return;
        }
        let mut folders = folders.into_iter();
        if let Some(first) = folders.next() {
            self.go(first, window, cx);
        }
        for (extra, id) in folders.enumerate() {
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
