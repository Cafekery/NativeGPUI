//! A Finder front end: real macOS chrome, an invented filesystem, `NG_QUIT=<frames>` to exit.

mod column;
mod data;
mod gallery;
mod header;
mod icon;
mod list;
mod palette;
mod prefs;
mod preview;
mod sidebar;
mod toolbar;
mod ui;

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, ClipboardItem, FocusHandle, KeyBinding, KeyDownEvent, MouseButton,
    Pixels, RenderImage, ScrollHandle, ScrollStrategy, SharedString, UniformListScrollHandle,
    Window, WindowBounds, WindowOptions, actions, div, point, prelude::*, px, relative, size,
};
use native_gpui::sidebar::{NativeSidebar, SidebarMetrics};
use native_gpui::tokens::{self, FileType, SystemTheme};
use native_gpui::{
    install_menus, native_root, remember_frame, system_corner_radius, window_options,
};
use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSWindow};
use objc2_foundation::NSSize;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::data::{Column, Folder, Line, Row, build_store};
use crate::header::{
    ColumnSpec, HEADER_H, HeaderDrag, NAME_MIN_W, PANE_INSET, band_glass, default_columns,
    pane_surface,
};
use crate::icon::{Grid, cell};
use crate::list::{ICON_SIZE, ROW_H};
use crate::palette::Palette;
use crate::preview::{INSET, LIST_MIN_W, PREVIEW_W, PreviewWindow};
use crate::toolbar::PATH_H;
use crate::ui::{TAGS, axis_locked};

pub(crate) const ICON_VIEW: usize = 0;
pub(crate) const LIST_VIEW: usize = 1;
pub(crate) const COLUMN_VIEW: usize = 2;
pub(crate) const GALLERY_VIEW: usize = 3;

pub(crate) const LIST_CONTEXT: &str = "FinderList";

actions!(finder_demo, [SelectAll, CopyNames, Back, Forward]);

pub(crate) struct FinderDemo {
    pub(crate) store: HashMap<&'static str, Folder>,
    pub(crate) sidebar: Option<NativeSidebar>,
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
    pub(crate) expanded: HashSet<SharedString>,
    pub(crate) selected: HashSet<SharedString>,
    pub(crate) cursor: Option<SharedString>,
    pub(crate) anchor: Option<SharedString>,
    pub(crate) renaming: Option<SharedString>,
    /// Keys, side in pixels, image.
    pub(crate) stack: RefCell<Option<(Vec<SharedString>, u32, Arc<RenderImage>)>>,
    typed: String,
    typed_at: Option<Instant>,
    pub(crate) tags: HashMap<SharedString, usize>,
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
    pub(crate) icon_size: f64,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) hscroll: ScrollHandle,
    pub(crate) strip: ScrollHandle,
    pub(crate) column_scroll: ScrollHandle,
    pub(crate) column_depth: usize,
    pub(crate) icons: Grid,
    pub(crate) window_min_w: f32,
    pub(crate) focus: FocusHandle,
    pub(crate) frames: u64,
    pub(crate) quit_at: Option<u64>,
}

impl FinderDemo {
    pub(crate) fn new(start: &'static str, cx: &mut App) -> Self {
        Self {
            store: build_store(),
            sidebar: None,
            sidebar_rows: crate::sidebar::default_rows(),
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
            expanded: HashSet::new(),
            selected: HashSet::new(),
            cursor: None,
            anchor: None,
            renaming: None,
            stack: RefCell::new(None),
            typed: String::new(),
            typed_at: None,
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
            icon_size: prefs::icon_size().unwrap_or(64.) as f64,
            scroll: UniformListScrollHandle::new(),
            hscroll: ScrollHandle::new(),
            strip: ScrollHandle::new(),
            column_scroll: ScrollHandle::new(),
            column_depth: 0,
            icons: Grid::default(),
            window_min_w: f32::NAN,
            focus: cx.focus_handle(),
            frames: 0,
            quit_at: env::var("NG_QUIT").ok().and_then(|v| v.parse().ok()),
        }
    }

    /// Rasterise every icon kind up front, one per turn, so no folder pays for it in a frame.
    pub(crate) fn warm_icons(&self, cx: &mut Context<Self>) {
        let mut kinds: Vec<FileType<'static>> = self
            .store
            .values()
            .flat_map(|folder| folder.rows.iter().filter_map(|row| row.artwork))
            .collect();
        kinds.sort_by_key(|kind| format!("{kind:?}"));
        kinds.dedup();
        let sizes = [ICON_SIZE, PREVIEW_W - 1. - 2. * INSET];
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
        self.store.get(self.here())
    }

    pub(crate) fn target(&self) -> &'static str {
        let here = self.here();
        if self.view != COLUMN_VIEW {
            return here;
        }
        let Some((holder, name)) = self.cursor.as_ref().and_then(|key| key.rsplit_once('/')) else {
            return here;
        };
        let Some((holder, folder)) = self.store.get_key_value(holder) else {
            return here;
        };
        if self.selected.len() > 1 {
            return holder;
        }
        folder
            .rows
            .iter()
            .find(|row| row.name == name)
            .and_then(|row| row.opens)
            .unwrap_or(holder)
    }

    pub(crate) fn retitle(&self, window: &mut Window) {
        let title = self.store.get(self.target()).map_or_else(
            || String::from("Finder"),
            |folder| match folder.crumbs.split_first() {
                Some((_volume, rest)) if !rest.is_empty() => format!("/{}", rest.join("/")),
                _ => folder.title.to_string(),
            },
        );
        window.set_window_title(&title);
    }

    pub(crate) fn go(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if self.here() == id {
            return;
        }
        self.history.truncate(self.at + 1);
        self.history.push(id);
        self.at = self.history.len() - 1;
        self.arrive(window, cx);
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
        self.icons.forget();
        self.settle();
        self.retitle(window);
        self.sidebar_follow();
        cx.notify();
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
        let left = self
            .lines(self.view == LIST_VIEW)
            .into_iter()
            .find(|line| line.row.opens == Some(from));
        if let Some(line) = left {
            self.pick(&line, false, false);
            self.reveal(&line.key);
        }
    }

    pub(crate) fn reveal(&mut self, key: &SharedString) {
        let lines = self.lines(self.view == LIST_VIEW);
        if let Some(index) = self.line_at(&lines, key) {
            self.scroll
                .scroll_to_item(index + self.list_inset().0, ScrollStrategy::Center);
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

    fn disclose_tree(&mut self, key: &SharedString, into: &'static str, open: bool) {
        self.disclose(key, open);
        let Some(folder) = self.store.get(into) else {
            return;
        };
        let children: Vec<(SharedString, &'static str)> = folder
            .rows
            .iter()
            .filter_map(|row| {
                row.opens
                    .map(|deeper| (SharedString::from(format!("{into}/{}", row.name)), deeper))
            })
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
        by_name.sort_by_key(|&index| lines[index].row.name.to_lowercase());
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

    fn type_select(&mut self, typed: &str) {
        let now = Instant::now();
        if self
            .typed_at
            .is_some_and(|at| now.duration_since(at) > Duration::from_secs(1))
        {
            self.typed.clear();
        }
        self.typed.push_str(&typed.to_lowercase());
        self.typed_at = Some(now);
        let lines = self.focused_lines();
        let mut by_name: Vec<&Line> = lines.iter().collect();
        by_name.sort_by_key(|line| line.row.name.to_lowercase());
        let pick = by_name
            .iter()
            .find(|line| line.row.name.to_lowercase() >= self.typed)
            .or(by_name.last())
            .map(|line| (*line).clone());
        if let Some(line) = pick {
            self.pick_in(&lines, &line, false, false);
            self.reveal(&line.key);
        }
    }

    pub(crate) fn rename(&mut self, key: &SharedString, name: &str) {
        let Some((folder, old)) = key.rsplit_once('/') else {
            return;
        };
        let name = name.trim();
        if name.is_empty() || name == old {
            return;
        }
        let (folder, old) = (folder.to_owned(), old.to_owned());
        let Some(row) = self
            .store
            .get_mut(folder.as_str())
            .and_then(|f| f.rows.iter_mut().find(|row| row.name.as_ref() == old))
        else {
            return;
        };
        row.name = SharedString::from(name.to_owned());
        let new_key = SharedString::from(format!("{folder}/{name}"));
        let follow = |set: &mut HashSet<SharedString>| {
            if set.remove(key) {
                set.insert(new_key.clone());
            }
        };
        follow(&mut self.selected);
        follow(&mut self.expanded);
        if let Some(tag) = self.tags.remove(key) {
            self.tags.insert(new_key.clone(), tag);
        }
        for slot in [&mut self.cursor, &mut self.anchor] {
            if slot.as_ref() == Some(key) {
                *slot = Some(new_key.clone());
            }
        }
    }

    pub(crate) fn order(&self, a: &Row, b: &Row) -> Ordering {
        let by = match self.sort {
            Column::Name => Ordering::Equal,
            Column::Modified => a.stamp.cmp(&b.stamp),
            Column::Size => a.bytes.cmp(&b.bytes),
            Column::Kind => a.face.kind().cmp(b.face.kind()),
        };
        let by = by.then_with(|| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name))
        });
        if self.ascending { by } else { by.reverse() }
    }

    pub(crate) fn lines(&self, tree: bool) -> Vec<Line> {
        let mut out = Vec::new();
        self.collect(self.here(), 0, tree, &mut out);
        out
    }

    pub(crate) fn collect(&self, id: &'static str, depth: usize, tree: bool, out: &mut Vec<Line>) {
        let Some(folder) = self.store.get(id) else {
            return;
        };
        let query = self.query.trim().to_lowercase();
        let mut rows: Vec<&Row> = folder
            .rows
            .iter()
            .filter(|row| query.is_empty() || row.name.to_lowercase().contains(&query))
            .collect();
        rows.sort_by(|a, b| self.order(a, b));

        for row in rows {
            let key = SharedString::from(format!("{id}/{}", row.name));
            let open = tree && self.expanded.contains(&key);
            out.push(Line {
                key,
                row: row.clone(),
                depth,
            });
            if let Some(into) = row.opens.filter(|_| open) {
                self.collect(into, depth + 1, tree, out);
            }
        }
    }

    pub(crate) fn line_at(&self, lines: &[Line], key: &SharedString) -> Option<usize> {
        lines.iter().position(|line| &line.key == key)
    }

    pub(crate) fn pick(&mut self, line: &Line, extend: bool, toggle: bool) {
        let lines = self.lines(self.view == LIST_VIEW);
        self.pick_in(&lines, line, extend, toggle);
    }

    pub(crate) fn pick_in(&mut self, lines: &[Line], line: &Line, extend: bool, toggle: bool) {
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

    pub(crate) fn cycle_tag(&mut self) {
        for key in self.selected.clone() {
            match self.tags.get(&key).copied() {
                Some(index) if index + 1 >= TAGS.len() => {
                    self.tags.remove(&key);
                }
                Some(index) => {
                    self.tags.insert(key, index + 1);
                }
                None => {
                    self.tags.insert(key, 0);
                }
            }
        }
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
                let page = (f32::from(window.viewport_size().height) / ROW_H) as usize;
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
                self.type_select(typed);
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
        let names: Vec<String> = self
            .focused_lines()
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .map(|line| line.row.name.to_string())
            .collect();
        if !names.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(names.join("\n")));
        }
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

    pub(crate) fn cursor_line(&self) -> Option<Line> {
        let lines = self.lines(self.view == LIST_VIEW);
        self.cursor
            .as_ref()
            .and_then(|key| lines.iter().find(|line| &line.key == key))
            .cloned()
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
    ) -> AnyElement {
        let top = metrics.top;
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
            .when(banded, |column| {
                column.child(band_glass(px(pane), top))
            })
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
        self.frames += 1;
        if let Some(limit) = self.quit_at {
            window.request_animation_frame();
            if self.frames >= limit {
                std::process::exit(0);
            }
        }
        self.sync_window_min(window);
        self.sync_preview_window(window, cx);

        let metrics = self.metrics();
        let corner = system_corner_radius(window);
        let palette = Palette::new(self.list_focused);
        let family = tokens::text_font_family(cx).unwrap_or_else(tokens::system_font_family);

        let path_bar = self.path_bar_shown.then(|| self.path_bar(&palette, window));
        let status_bar = self.status_bar(&palette, cx);
        let body = match self.view {
            ICON_VIEW => self.icon_body(&palette, window, cx),
            COLUMN_VIEW => self.column_body(&palette, cx),
            GALLERY_VIEW => self.gallery_body(&palette, window, cx),
            _ => self.list_body(&palette, cx),
        };
        let headers = (self.view == LIST_VIEW).then(|| self.header_row(&palette, cx));

        let list_view = self.view == LIST_VIEW;
        native_root(
            div()
                .size_full()
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
                        .on_key_down(cx.listener(Self::on_key))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
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
                                    body, headers, path_bar, &palette, metrics, corner,
                                )),
                        )
                        .when(self.status_bar_shown, |c| {
                            c.child(div().flex_none().pl(metrics.left).child(status_bar))
                        }),
                ),
        )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        tokens::init(cx);
        install_menus(cx);
        // After `install_menus`, never before: the more specific binding has to be the later one.
        cx.bind_keys([
            KeyBinding::new("cmd-a", SelectAll, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-c", CopyNames, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-[", Back, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-]", Forward, Some(LIST_CONTEXT)),
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
        open_finder_window("documents", 0, cx);
        cx.activate(true);
    });
}

fn open_finder_window(start: &'static str, extra: usize, cx: &mut App) {
    let step = px(extra as f32 * 30.);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(160.) + step, px(120.) + step),
                size: size(px(1200.), px(772.)),
            })),
            ..window_options()
        },
        |window, cx| {
            if extra == 0 {
                remember_frame(window, "ng.finder.window");
            }
            cx.new(|cx| {
                cx.observe_global::<SystemTheme>(|_, cx| cx.notify())
                    .detach();
                FinderDemo::new(start, cx)
            })
        },
    )
    .expect("the Finder window opens");
}
