//! Where the window is: the folder on screen, the trail behind it, and every way of leaving it.

use std::path::Path;

use gpui::{Context, SharedString, Window};

use crate::app::FinderDemo;
use crate::commands::{self, Back, Forward};
use crate::data::{Folder, Line, intern_path, split_key};
use crate::view::{COLUMN_VIEW, LIST_VIEW};
use crate::{home, open_finder_window, tag_title};

impl FinderDemo {
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

    pub(crate) fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(crate) fn back(&mut self, _: &Back, window: &mut Window, cx: &mut Context<Self>) {
        self.step(false, window, cx);
    }

    pub(crate) fn forward(&mut self, _: &Forward, window: &mut Window, cx: &mut Context<Self>) {
        self.step(true, window, cx);
    }
}
