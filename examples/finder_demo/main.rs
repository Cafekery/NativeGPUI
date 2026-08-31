//! A Finder front end. Real macOS chrome, entirely fictional files.
//!
//! Nothing here reads the filesystem: the tree in `DIRS` is invented, the sidebar's Locations are
//! props, and a row's icon comes from a *type* lookup — the system's own artwork for `.pdf`, for a
//! folder, for an application bundle — never from a file on disk. Dates and sizes are formatted by
//! `NSDateFormatter` and `NSByteCountFormatter`, so the columns read in the user's own locale,
//! timezone and unit conventions rather than in a format written down here. Relative date
//! formatting is what supplies Finder's `Today` and `Yesterday` tiers, and the switch happens on
//! the calendar day rather than after some number of hours: a file touched eight hours ago reads
//! `Yesterday` as soon as midnight has passed.
//!
//! What is native: the sidebar (`NSSplitViewItem.sidebar`, `NSOutlineView` rows), the unified
//! toolbar and its items, the back/forward and view-mode segmented controls, the search field and
//! the icon-size slider. What GPUI draws: the file list, its headers, the path bar, the status bar
//! and the info pane — all in system fonts and semantic colours so the two halves agree.
//!
//! `NG_QUIT=<frames>` exits after N frames, so a smoke run ends on its own.

mod column;
mod data;
mod gallery;
mod header;
mod icon;
mod list;
mod palette;
mod preview;
mod sidebar;
mod toolbar;
mod ui;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::env;

use gpui::{
    App, Application, Bounds, ClipboardItem, FocusHandle, KeyBinding, KeyDownEvent, MouseButton,
    ScrollStrategy, SharedString, UniformListScrollHandle, Window, WindowBounds, WindowOptions,
    actions, div, point, prelude::*, px, size,
};
use native_gpui::sidebar::{NativeSidebar, SidebarMetrics};
use native_gpui::tokens::{self, SystemTheme};
use native_gpui::{install_menus, native_root, window_options};

use crate::data::{Column, Folder, Line, Row, SIDEBAR, build_store};
use crate::icon::{TILE_GAP, TILE_MARGIN};
use crate::palette::Palette;
use crate::preview::PREVIEW_W;
use crate::ui::TAGS;

pub(crate) const ICON_VIEW: usize = 0;
pub(crate) const LIST_VIEW: usize = 1;
pub(crate) const COLUMN_VIEW: usize = 2;
pub(crate) const GALLERY_VIEW: usize = 3;

/// Keymap context for the file list. Scopes ⌘A and ⌘C to it, so they stay the menu's — and the
/// hosted search field's — whenever focus is anywhere else.
pub(crate) const LIST_CONTEXT: &str = "FinderList";

actions!(finder_demo, [SelectAll, CopyNames]);

// ---------------------------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------------------------

pub(crate) struct FinderDemo {
    pub(crate) store: HashMap<&'static str, Folder>,
    pub(crate) sidebar: Option<NativeSidebar>,
    pub(crate) toolbar: bool,
    pub(crate) history: Vec<&'static str>,
    pub(crate) at: usize,
    pub(crate) view: usize,
    pub(crate) sort: Column,
    pub(crate) ascending: bool,
    pub(crate) query: String,
    pub(crate) expanded: HashSet<SharedString>,
    pub(crate) selected: HashSet<SharedString>,
    pub(crate) cursor: Option<SharedString>,
    pub(crate) anchor: Option<SharedString>,
    pub(crate) tags: HashMap<SharedString, usize>,
    pub(crate) preview: bool,
    /// Both default off: Finder ships them off and the reference window has them off.
    pub(crate) path_bar_shown: bool,
    pub(crate) status_bar_shown: bool,
    pub(crate) icon_size: f64,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) focus: FocusHandle,
    pub(crate) frames: u64,
    pub(crate) quit_at: Option<u64>,
}

impl FinderDemo {
    pub(crate) fn new(cx: &mut App) -> Self {
        Self {
            store: build_store(),
            sidebar: None,
            toolbar: false,
            history: vec!["documents"],
            at: 0,
            view: LIST_VIEW,
            sort: Column::Name,
            ascending: true,
            query: String::new(),
            expanded: HashSet::new(),
            selected: HashSet::new(),
            cursor: None,
            anchor: None,
            tags: HashMap::new(),
            // On, as Finder's own window is: the right pane is why the list is 594pt wide in a
            // 1026pt window, and the column geometry only lands where Finder puts it at that width.
            preview: true,
            path_bar_shown: false,
            status_bar_shown: false,
            icon_size: 64.,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            frames: 0,
            quit_at: env::var("NG_QUIT").ok().and_then(|v| v.parse().ok()),
        }
    }

    pub(crate) fn here(&self) -> &'static str {
        self.history[self.at]
    }

    pub(crate) fn folder(&self) -> Option<&Folder> {
        self.store.get(self.here())
    }

    /// Finder's title is the full POSIX path, not the folder name, whenever
    /// `_FXShowPosixPathInTitle` is set — which is what the window this is matched against shows.
    /// The volume crumb is dropped, exactly as a real path drops it.
    pub(crate) fn retitle(&self, window: &mut Window) {
        let title = self.folder().map_or_else(
            || String::from("Finder"),
            |folder| match folder.crumbs.split_first() {
                Some((_volume, rest)) if !rest.is_empty() => format!("/{}", rest.join("/")),
                _ => folder.title.to_string(),
            },
        );
        window.set_window_title(&title);
    }

    // -- navigation ----------------------------------------------------------------------------

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
        self.retitle(window);
        if let Some(sidebar) = &self.sidebar {
            let here = self.here();
            let known = SIDEBAR
                .iter()
                .any(|(_, _, rows)| rows.iter().any(|(id, _, _, _)| *id == here));
            sidebar.set_selected(known.then_some(here));
        }
        cx.notify();
    }

    pub(crate) fn open(&mut self, line: &Line, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = line.row.opens {
            self.go(id, window, cx);
        }
    }

    pub(crate) fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(parent) = self.folder().and_then(|folder| folder.parent) {
            self.go(parent, window, cx);
        }
    }

    // -- listing -------------------------------------------------------------------------------

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

    /// `tree` follows open disclosure triangles; the other views are flat.
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
            let into = row.opens;
            out.push(Line {
                key,
                row: row.clone(),
                depth,
            });
            if open {
                if let Some(into) = into {
                    self.collect(into, depth + 1, tree, out);
                }
            }
        }
    }

    pub(crate) fn line_at(&self, lines: &[Line], key: &SharedString) -> Option<usize> {
        lines.iter().position(|line| &line.key == key)
    }

    /// Selection, AppKit's rules: plain replaces and re-anchors, ⌘ toggles, ⇧ extends from the
    /// anchor. The anchor is separate from the cursor because a run of ⇧↓ must grow one range from
    /// where the run started, not repeatedly select the last two rows.
    pub(crate) fn pick(&mut self, line: &Line, extend: bool, toggle: bool) {
        if toggle {
            if !self.selected.remove(&line.key) {
                self.selected.insert(line.key.clone());
            }
            self.anchor = Some(line.key.clone());
        } else if extend {
            let lines = self.lines(self.view == LIST_VIEW);
            let anchor = self
                .anchor
                .as_ref()
                .or(self.cursor.as_ref())
                .and_then(|key| self.line_at(&lines, key))
                .unwrap_or(0);
            if let Some(target) = self.line_at(&lines, &line.key) {
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

    // -- keyboard ------------------------------------------------------------------------------

    pub(crate) fn on_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        let tree = self.view == LIST_VIEW;
        let lines = self.lines(tree);
        if lines.is_empty() {
            return;
        }
        let current = self
            .cursor
            .as_ref()
            .and_then(|key| self.line_at(&lines, key));

        match event.keystroke.key.as_str() {
            "up" | "down" if modifiers.platform => {
                if event.keystroke.key == "up" {
                    self.up(window, cx);
                } else if let Some(line) = current.map(|index| lines[index].clone()) {
                    self.open(&line, window, cx);
                }
            }
            key @ ("up" | "down") => {
                let step: isize = if key == "up" { -1 } else { 1 };
                let next = match current {
                    Some(index) => (index as isize + step).clamp(0, lines.len() as isize - 1),
                    None if step > 0 => 0,
                    None => lines.len() as isize - 1,
                } as usize;
                let line = lines[next].clone();
                self.pick(&line, modifiers.shift, false);
                self.scroll.scroll_to_item(
                    if tree {
                        next
                    } else {
                        next / self.columns(window, cx)
                    },
                    ScrollStrategy::Center,
                );
                cx.notify();
            }
            "right" | "left" => {
                let open = event.keystroke.key == "right";
                if let Some(line) = current.map(|index| lines[index].clone()) {
                    if line.row.opens.is_some() {
                        self.disclose(&line.key, open);
                        cx.notify();
                    }
                }
            }
            "enter" => {
                if let Some(line) = current.map(|index| lines[index].clone()) {
                    self.open(&line, window, cx);
                }
            }
            _ => {}
        }
    }

    /// ⌘A and ⌘C arrive as actions rather than through [`Self::on_key`]. `install_menus` binds both
    /// chords so the Edit menu can carry their key equivalents, and a matched binding stops the
    /// key event before a bubble-phase listener sees it — so a plain `on_key_down` for ⌘A is dead
    /// code. Binding them again under [`LIST_CONTEXT`] wins only while this list has GPUI focus,
    /// which leaves the chords to the menu, and thus to the hosted search field, everywhere else.
    pub(crate) fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self
            .lines(self.view == LIST_VIEW)
            .iter()
            .map(|line| line.key.clone())
            .collect();
        cx.notify();
    }

    pub(crate) fn copy_names(&mut self, _: &CopyNames, _: &mut Window, cx: &mut Context<Self>) {
        let names: Vec<String> = self
            .lines(self.view == LIST_VIEW)
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .map(|line| line.row.name.to_string())
            .collect();
        if !names.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(names.join("\n")));
        }
    }

    /// The sidebar's gutters, read off the handle rather than the `SidebarMetrics` global. The
    /// global is published from an AppKit layout pass through a spawned task, so it is unset for
    /// the first frames and each of those lays the tree out under the sidebar; the handle carries
    /// the same numbers synchronously.
    pub(crate) fn metrics(&self) -> SidebarMetrics {
        self.sidebar
            .as_ref()
            .map(NativeSidebar::metrics)
            .unwrap_or_default()
    }

    /// Tiles per band in icon view. The width available is the viewport less the gutters the
    /// sidebar publishes, so a collapse re-flows the grid.
    pub(crate) fn columns(&self, window: &Window, _cx: &App) -> usize {
        let metrics = self.metrics();
        let avail = f32::from(window.viewport_size().width)
            - f32::from(metrics.left)
            - f32::from(metrics.right)
            - if self.preview { PREVIEW_W } else { 0. }
            - 24.;
        let tile = self.icon_size as f32 + TILE_MARGIN + TILE_GAP;
        ((avail / tile).floor() as usize).max(1)
    }
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

        let metrics = self.metrics();
        let palette = Palette::new(self.focus.is_focused(window));
        let family = tokens::text_font_family(cx).unwrap_or_else(tokens::system_font_family);

        let path_bar = self.path_bar(&palette);
        let status_bar = self.status_bar(&palette, cx);
        let cursor = {
            let lines = self.lines(self.view == LIST_VIEW);
            self.cursor
                .as_ref()
                .and_then(|key| lines.iter().find(|line| &line.key == key))
                .cloned()
        };
        let body = match self.view {
            ICON_VIEW => self.icon_body(&palette, window, cx),
            COLUMN_VIEW => self.column_body(&palette, cx),
            GALLERY_VIEW => self.gallery_body(&palette, cx),
            _ => self.list_body(&palette, cx),
        };
        let headers = (self.view == LIST_VIEW).then(|| self.header_row(&palette, cx));
        let opaque = matches!(self.view, ICON_VIEW | LIST_VIEW | COLUMN_VIEW);

        // The gutters come from `SidebarMetrics` as padding, never as a background: a fill over
        // the sidebar strip would paint on top of its material and flatten it.
        native_root(
            div()
                .size_full()
                .font_family(family)
                .text_size(tokens::system_font_size())
                .flex()
                .flex_col()
                .pl(metrics.left)
                .pt(metrics.top)
                .pr(metrics.right)
                .pb(metrics.bottom)
                .child(
                    div()
                        .track_focus(&self.focus)
                        .key_context(LIST_CONTEXT)
                        .on_action(cx.listener(Self::select_all))
                        .on_action(cx.listener(Self::copy_names))
                        .on_key_down(cx.listener(Self::on_key))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, _| window.focus(&this.focus)),
                        )
                        .size_full()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .overflow_hidden()
                                // No line under the toolbar when the column header is there: the
                                // header carries the only hairline, and the toolbar and header
                                // share one continuous background. The other views have no header,
                                // so the body needs the line drawn for them.
                                .when(headers.is_none(), |row| {
                                    row.border_t_1().border_color(palette.separator)
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .overflow_hidden()
                                        .children(headers)
                                        .child(
                                            div()
                                                .flex_1()
                                                .flex()
                                                .flex_col()
                                                .overflow_hidden()
                                                .when(opaque, |body| body.bg(palette.surface))
                                                .child(body),
                                        ),
                                )
                                .when(self.preview, |row| {
                                    row.child(
                                        div()
                                            .w(px(PREVIEW_W))
                                            .flex_none()
                                            .flex()
                                            .flex_col()
                                            .items_center()
                                            .justify_center()
                                            .p(px(18.))
                                            .border_l_1()
                                            .border_color(palette.separator)
                                            .child(self.info_card(cursor.as_ref(), &palette, 96.)),
                                    )
                                }),
                        )
                        .when(self.path_bar_shown, |c| c.child(path_bar))
                        .when(self.status_bar_shown, |c| c.child(status_bar)),
                ),
        )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        tokens::init(cx);
        install_menus(cx);
        // After `install_menus`, never before: it binds both chords with no context, and the more
        // specific binding has to be the later one.
        cx.bind_keys([
            KeyBinding::new("cmd-a", SelectAll, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-c", CopyNames, Some(LIST_CONTEXT)),
        ]);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(160.), px(120.)),
                    size: size(px(1026.), px(772.)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|cx| {
                    cx.observe_global::<SystemTheme>(|_, cx| cx.notify())
                        .detach();
                    FinderDemo::new(cx)
                })
            },
        )
        .expect("the Finder window opens");
        cx.activate(true);
    });
}
