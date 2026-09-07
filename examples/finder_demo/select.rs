//! What is chosen and where the cursor is: Finder's click rules, its type-select, its twists.

use std::path::Path;
use std::time::{Duration, Instant};

use finder_fs::index;
use gpui::{Context, ScrollStrategy, SharedString, Window};

use crate::app::FinderDemo;
use crate::data::Line;
use crate::menus::SelectAll;
use crate::view::LIST_VIEW;

/// How far under a row Option-disclose walks, and how many folders one window holds open.
const DEEP_LEVELS: usize = 16;
const DEEP_FOLDERS: usize = 5_000;

impl FinderDemo {
    pub(crate) fn reveal(&mut self, key: &SharedString) {
        let lines = self.lines(self.view == LIST_VIEW);
        if let Some(index) = self.line_at(&lines, key) {
            self.scroll.scroll_to_item(
                self.slot_of(&lines, index) + self.list_inset().0,
                ScrollStrategy::Center,
            );
        }
    }

    pub(crate) fn disclose_selected(&mut self, open: bool, deep: bool) {
        let lines = self.lines(true);
        let chosen = self.selected.snapshot();
        let cursor = self.cursor.clone().filter(|_| chosen.is_empty());
        let mut keys: Vec<SharedString> = Vec::new();
        let mut folders: Vec<(SharedString, &'static str)> = Vec::new();
        for line in lines.iter().filter(|line| match &cursor {
            Some(cursor) => *cursor == line.key,
            None => chosen.contains(&line.key),
        }) {
            keys.push(line.key.clone());
            if let Some(into) = line.row.opens.filter(|_| line.row.discloses) {
                folders.push((line.key.clone(), into));
            }
        }
        let expandable = folders
            .iter()
            .filter(|(key, _)| self.expanded.contains(key) != open)
            .count();
        if expandable > 0 || (deep && open) {
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
        if open {
            self.expand_under(into, 0);
        } else {
            self.collapse_under(into);
        }
    }

    /// The chosen rows, in the order the view lists them.
    pub(crate) fn chosen_lines(&self) -> Vec<Line> {
        self.focused_lines()
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .cloned()
            .collect()
    }

    pub(crate) fn chosen_keys(&self) -> Vec<SharedString> {
        self.chosen_lines()
            .into_iter()
            .map(|line| line.key)
            .collect()
    }

    /// The folders among the chosen rows, which are the ones a window or tab can open.
    pub(crate) fn chosen_folders(&self) -> Vec<&'static str> {
        self.chosen_lines()
            .iter()
            .filter_map(|line| line.row.opens)
            .collect()
    }

    /// Discloses every folder in memory under `id` and asks for the listings that are not.
    fn expand_under(&mut self, id: &'static str, depth: usize) {
        let home = crate::home();
        let mut opened: Vec<SharedString> = Vec::new();
        let mut walk = vec![(id, depth)];
        while let Some((id, depth)) = walk.pop() {
            if depth >= DEEP_LEVELS || self.expanded.len() + opened.len() >= DEEP_FOLDERS {
                continue;
            }
            let Some(folder) = self.folders.get(id) else {
                self.expanding.insert(id, depth);
                continue;
            };
            let loading = folder.loading;
            let children: Vec<(SharedString, &'static str)> = folder
                .rows
                .iter()
                .filter(|row| row.discloses)
                .filter_map(|row| row.opens.map(|deeper| (row.path.clone(), deeper)))
                .filter(|(_, deeper)| !index::off_limits(Path::new(deeper), &home))
                .collect();
            if loading {
                self.expanding.insert(id, depth);
            } else {
                self.expanding.remove(id);
            }
            for (key, deeper) in children {
                opened.push(key);
                walk.push((deeper, depth + 1));
            }
        }
        self.expanded.extend(opened);
    }

    /// Shuts every folder under `id`, whether or not this window ever had its listing.
    fn collapse_under(&mut self, id: &'static str) {
        let under = id.trim_end_matches('/');
        let inside = |path: &str| path.strip_prefix(under).is_some_and(|r| r.starts_with('/'));
        self.expanded.retain(|key| !inside(key));
        self.expanding.retain(|id, _| !inside(id));
    }

    /// The listing a walk was waiting on: disclose what it holds and go on down.
    pub(crate) fn expand_landed(&mut self, id: &'static str) {
        if let Some(depth) = self.expanding.remove(id) {
            self.expand_under(id, depth);
        }
    }

    pub(crate) fn tab(&mut self, backwards: bool) {
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

    /// Finder's type-select, falling back to a filter where no row starts with what is typed.
    pub(crate) fn type_select(&mut self, typed: &str, cx: &mut Context<Self>) {
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

    pub(crate) fn clear_type_filter(&mut self) {
        self.typed.clear();
        self.typed_at = None;
        self.type_filter = None;
        self.type_filter_seen_match = false;
    }

    /// Once a match existed an idle spell clears the filter; a run that never matched waits.
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

    pub(crate) fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self
            .focused_lines()
            .iter()
            .map(|line| line.key.clone())
            .collect();
        cx.notify();
    }
}
