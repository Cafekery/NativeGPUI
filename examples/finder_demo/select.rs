//! What is chosen and where the cursor is: Finder's click, Shift and Command rules, its
//! type-select, and the disclosure triangles the list opens with them.

use std::time::{Duration, Instant};

use gpui::{Context, ScrollStrategy, SharedString, Window};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSString, NSURL};

use crate::app::FinderDemo;
use crate::commands::{CopyNames, SelectAll};
use crate::data::{Line, split_key};
use crate::view::LIST_VIEW;

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

    /// Finder's type-select: jump to the first row starting with what is typed. When nothing
    /// starts with it, fall back to filtering rather than Finder's beep-and-do-nothing.
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
}
