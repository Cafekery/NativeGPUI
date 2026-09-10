use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use finder_fs::index;
use gpui::{Context, ScrollStrategy, SharedString, Window};

use crate::app::FinderDemo;
use crate::data::Line;
use crate::listing::Edit;
use crate::menus::SelectAll;
use crate::view::LIST_VIEW;

const TYPING: Duration = Duration::from_secs(1);

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
        let one = match chosen.len() {
            0 => self.cursor.clone(),
            1 => chosen.iter().next().cloned(),
            _ => None,
        };
        let only = one
            .as_ref()
            .and_then(|key| lines.iter().position(|line| &line.key == key));
        let mut folders: Vec<(SharedString, &'static str)> = Vec::new();
        let mut edits: Vec<Edit> = Vec::new();
        let mut take = |at: usize, line: &Line| {
            if let Some(into) = line.row.opens.filter(|_| line.row.discloses) {
                folders.push((line.key.clone(), into));
                edits.push((at, line.key.clone(), open.then_some(into)));
            }
        };
        if let Some(at) = only {
            take(at, &lines[at]);
        } else if one.is_none() {
            for (at, line) in lines.iter().enumerate() {
                if line.row.opens.is_some() && chosen.contains(&line.key) {
                    take(at, line);
                }
            }
        }
        let expandable = folders
            .iter()
            .filter(|(key, _)| self.expanded.contains(key) != open)
            .count();
        if expandable > 0 || (deep && open) {
            let was = self.expanded.rev();
            self.disclose_all(&folders, open, deep);
            self.disclosed(was, edits);
            return;
        }
        if open {
            return;
        }
        let Some(index) = only else {
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

    pub(crate) fn disclose_at(
        &mut self,
        at: usize,
        key: &SharedString,
        into: Option<&'static str>,
        open: bool,
        deep: bool,
    ) {
        let was = self.expanded.rev();
        match into.filter(|_| deep) {
            Some(into) => self.disclose_all(&[(key.clone(), into)], open, true),
            None => self.disclose(key, open),
        }
        if let Some(into) = into {
            self.disclosed(was, vec![(at, key.clone(), open.then_some(into))]);
        }
    }

    fn disclose_all(&mut self, folders: &[(SharedString, &'static str)], open: bool, deep: bool) {
        if open {
            let mut opened: Vec<SharedString> = Vec::with_capacity(folders.len());
            for (key, into) in folders {
                opened.push(key.clone());
                if deep {
                    self.expand_under(into, 0, &mut opened);
                }
            }
            self.expanded.extend(opened);
            return;
        }
        let shut: HashSet<&SharedString> = folders.iter().map(|(key, _)| key).collect();
        let under: HashSet<&str> = match deep {
            true => folders
                .iter()
                .map(|(_, into)| into.trim_end_matches('/'))
                .collect(),
            false => HashSet::new(),
        };
        let inside = |path: &str| {
            let mut rest = path;
            while let Some(cut) = rest.rfind('/') {
                rest = &rest[..cut];
                if under.contains(rest) {
                    return true;
                }
            }
            false
        };
        self.expanded
            .retain(|key| !shut.contains(key) && !inside(key));
        if deep {
            self.expanding.retain(|id, _| !inside(id));
        }
    }

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

    pub(crate) fn chosen_folders(&self) -> Vec<&'static str> {
        self.chosen_lines()
            .iter()
            .filter_map(|line| line.row.opens)
            .collect()
    }

    fn expand_under(&mut self, id: &'static str, depth: usize, opened: &mut Vec<SharedString>) {
        let home = crate::home();
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
    }

    pub(crate) fn expand_landed(&mut self, id: &'static str) {
        if let Some(depth) = self.expanding.remove(id) {
            let mut opened: Vec<SharedString> = Vec::new();
            self.expand_under(id, depth, &mut opened);
            self.expanded.extend(opened);
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

    pub(crate) fn type_select(&mut self, typed: &str, cx: &mut Context<Self>) {
        let now = Instant::now();
        if self
            .typed_at
            .is_some_and(|at| now.duration_since(at) > TYPING)
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

    pub(crate) fn typing(&self) -> bool {
        self.typed_at.is_some_and(|at| at.elapsed() <= TYPING)
    }

    pub(crate) fn clear_type_filter(&mut self) {
        self.typed.clear();
        self.typed_at = None;
        self.type_filter = None;
        self.type_filter_seen_match = false;
    }

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
