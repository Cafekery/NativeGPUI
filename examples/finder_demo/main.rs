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
    AnyElement, App, Application, Bounds, ClipboardItem, FocusHandle, KeyBinding, KeyDownEvent,
    MouseButton, Pixels, ScrollHandle, ScrollStrategy, SharedString, UniformListScrollHandle,
    Window, WindowBounds, WindowOptions, actions, div, point, prelude::*, px, size,
};
use native_gpui::sidebar::{NativeSidebar, SidebarMetrics};
use native_gpui::tokens::{self, SystemTheme};
use native_gpui::{install_menus, native_root, window_options};
use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSWindow};
use objc2_foundation::NSSize;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::data::{Column, Folder, Line, Row, build_store};
use crate::header::{ColumnSpec, HEADER_H, HeaderDrag, NAME_MIN_W, band_glass, default_columns};
use crate::icon::{Grid, cell};
use crate::palette::Palette;
use crate::preview::{LIST_MIN_W, PREVIEW_W};
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
    /// Sidebar row ids per group. The user can drag rows and eject a volume, so the order and the
    /// membership are the demo's own data rather than the table's.
    pub(crate) sidebar_rows: Vec<Vec<&'static str>>,
    pub(crate) toolbar: bool,
    pub(crate) history: Vec<&'static str>,
    pub(crate) at: usize,
    pub(crate) view: usize,
    pub(crate) sort: Column,
    pub(crate) ascending: bool,
    /// The three fixed columns in the order they are drawn, each at the width it is drawn at.
    /// Finder persists both per folder and lets the user drag either, so they are state and not
    /// constants. Name is absent on purpose: it is pinned first and carries its own width below.
    pub(crate) list_columns: Vec<ColumnSpec>,
    /// The Name column's width. Fixed like the rest — the list clips rather than reflowing — but
    /// draggable, so it is state. Starts on its own floor, which is where Finder starts it.
    pub(crate) name_w: f32,
    /// The header drag in flight — carrying a column, or moving a boundary.
    pub(crate) header_drag: Option<HeaderDrag>,
    pub(crate) query: String,
    pub(crate) expanded: HashSet<SharedString>,
    pub(crate) selected: HashSet<SharedString>,
    pub(crate) cursor: Option<SharedString>,
    pub(crate) anchor: Option<SharedString>,
    pub(crate) tags: HashMap<SharedString, usize>,
    pub(crate) preview: bool,
    /// The preview pane's width, which the user drags. Finder persists this per window rather than
    /// deriving it, so `PREVIEW_W` is only where it starts.
    pub(crate) preview_w: f32,
    /// Where the pane's trailing edge would be with the pointer at x 0, captured on mouse-down so
    /// a drag cannot accumulate error.
    pub(crate) preview_drag: Option<f32>,
    pub(crate) preview_expanded: bool,
    /// Both default off: Finder ships them off and the reference window has them off.
    pub(crate) path_bar_shown: bool,
    pub(crate) status_bar_shown: bool,
    pub(crate) icon_size: f64,
    pub(crate) scroll: UniformListScrollHandle,
    /// The column view's horizontal scroll, and the chain length it was last matched to. Finder
    /// scrolls the chain right when a selection opens a new column and otherwise leaves the offset
    /// where the user put it, so following it needs the previous depth as well as the current one:
    /// re-requesting the scroll every frame would drag the view back rightwards under a user who
    /// had just scrolled left.
    pub(crate) column_scroll: ScrollHandle,
    pub(crate) column_depth: usize,
    /// Where each icon-view item sits, and the drag in flight. Lives here because the keyboard
    /// scrolls the grid and `columns` sizes it.
    pub(crate) icons: Grid,
    /// The content minimum last written to the window, so a frame that changes nothing makes no
    /// AppKit call. `f32::NAN` writes on the first frame whatever the derived value is.
    pub(crate) window_min_w: f32,
    pub(crate) focus: FocusHandle,
    pub(crate) frames: u64,
    pub(crate) quit_at: Option<u64>,
}

impl FinderDemo {
    pub(crate) fn new(cx: &mut App) -> Self {
        Self {
            store: build_store(),
            sidebar: None,
            sidebar_rows: crate::sidebar::default_rows(),
            toolbar: false,
            history: vec!["documents"],
            at: 0,
            view: LIST_VIEW,
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
            tags: HashMap::new(),
            // On, as Finder's own window is: the right pane is why the list is 594pt wide in a
            // 1026pt window, and the column geometry only lands where Finder puts it at that width.
            preview: true,
            preview_w: PREVIEW_W,
            preview_drag: None,
            preview_expanded: false,
            path_bar_shown: false,
            status_bar_shown: false,
            icon_size: 64.,
            scroll: UniformListScrollHandle::new(),
            column_scroll: ScrollHandle::new(),
            column_depth: 0,
            icons: Grid::default(),
            window_min_w: f32::NAN,
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
        self.icons.forget();
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
        // Column view answers its own arrows: ↑/↓ have to run against the focused column's
        // listing rather than the window folder's, and ←/→ move between columns instead of
        // opening a disclosure triangle this view does not have.
        //
        // The modifiers go with the key because claiming all four arrows unconditionally also
        // swallowed ⌘↑ — Go to Enclosing Folder was dead in column view, and only driving the
        // app found it: every arrow behaved, and the one chord that shares their key names did
        // not.
        if self.view == COLUMN_VIEW && self.column_key(event.keystroke.key.as_str(), modifiers, cx)
        {
            cx.notify();
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
            key @ ("up" | "down") | key @ ("left" | "right") if self.view == ICON_VIEW => {
                let step: isize = match key {
                    "up" => -(self.columns(window, cx) as isize),
                    "down" => self.columns(window, cx) as isize,
                    "left" => -1,
                    _ => 1,
                };
                let next = match current {
                    Some(index) => (index as isize + step).clamp(0, lines.len() as isize - 1),
                    None if step > 0 => 0,
                    None => lines.len() as isize - 1,
                } as usize;
                let line = lines[next].clone();
                self.pick(&line, modifiers.shift, false);
                self.icons.reveal(
                    &line.key,
                    next,
                    self.columns(window, cx),
                    self.icon_size as f32,
                );
                cx.notify();
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
                // Past the blank rows that stand in for the list's top content inset: every index
                // the list deals in is a line plus those.
                self.scroll
                    .scroll_to_item(next + self.list_inset().0, ScrollStrategy::Center);
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

    /// Whether the trailing preview pane is on screen. Column view has its own, in the chain.
    pub(crate) fn preview_shown(&self) -> bool {
        self.preview && self.view != COLUMN_VIEW
    }

    /// Hold the window to the width the list's own minimum implies.
    ///
    /// Finder derives its window minimum from the list rather than fixing it: the window stops at
    /// 748 with the pane at 240 and at 988 with the pane at 480, and both are the sidebar's 192
    /// plus the list's 316 plus the pane. So the minimum moves as the pane is dragged. gpui applies
    /// `window_min_size` once at creation and offers nothing after, so the derived value goes
    /// straight to `setContentMinSize:`; the height half is read back rather than written, because
    /// this derives a width and has no reading for the other axis.
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

    /// The list column: the file view, the column-header band, and the glass over both bands.
    ///
    /// It starts at the window's top edge rather than below the toolbar. Finder's list scroll
    /// surface does too — the 29 in its toolbar band is the list showing through a transparent
    /// titlebar, which is why blurred rows turn up inside the band while a folder is being scrolled
    /// — and running the surface up there is what gives the band's material something of the list's
    /// own to sample. Only the view that has a header band is built this way: the other three have
    /// no measured reading for what is behind their chrome, and taking one from a neighbour is what
    /// this file does not do.
    fn list_column(
        &self,
        body: AnyElement,
        headers: Option<AnyElement>,
        palette: &Palette,
        top: Pixels,
    ) -> AnyElement {
        let banded = headers.is_some();
        let opaque = matches!(self.view, ICON_VIEW | LIST_VIEW | COLUMN_VIEW);
        div()
            .flex_1()
            .relative()
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .top(if banded { px(0.) } else { top })
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .when(opaque, |surface| surface.bg(palette.surface))
                    // No line under the toolbar when the column header is there: the header carries
                    // the only hairline, and the toolbar and header share one continuous
                    // background. Either way it stops at the list and never crosses the preview —
                    // Finder's header band and its hairline span the list pane alone.
                    //
                    // Icon view is excluded because Finder draws no rule there: a full-width step
                    // scan over y 38…62 returns zero rows on Finder and y 51→53 at 100% on ours.
                    // Column view keeps it — Finder's rule sits at y 51 in two windows, luma 50.37
                    // and 50.04, which is `separatorColor` over the (29,29,28) body at 51.16,
                    // exactly what this draws. Gallery is UNMEASURED and keeps the rule rather than
                    // inheriting a neighbour's answer: the two headerless views that have been
                    // measured disagree with each other, which is the evidence that this cannot be
                    // reasoned across.
                    .when(!banded && self.view != ICON_VIEW, |column| {
                        column.border_t_1().border_color(palette.separator)
                    })
                    .child(body),
            )
            // Painted after the body and before the header, which is the whole ordering rule: a
            // `Layer::Above` view's z-position inside the hosted container is GPUI's paint order,
            // so the material lands over the rows and the header's own hosted labels land over the
            // material.
            .when(banded, |column| column.child(band_glass(top)))
            .children(headers.map(|headers| {
                div()
                    .absolute()
                    .top(top)
                    .left_0()
                    .right_0()
                    .h(px(HEADER_H))
                    .child(headers)
            }))
            .into_any_element()
    }

    /// Tiles per band in icon view. The width available is the viewport less the gutters the
    /// sidebar publishes, so a collapse re-flows the grid.
    pub(crate) fn columns(&self, window: &Window, _cx: &App) -> usize {
        let metrics = self.metrics();
        let avail = f32::from(window.viewport_size().width)
            - f32::from(metrics.left)
            - f32::from(metrics.right)
            - if self.preview { self.preview_w } else { 0. }
            - 24.;
        let tile = cell(self.icon_size as f32);
        ((avail / tile).floor() as usize).max(1)
    }
}

/// The window behind gpui's view. `native-gpui` keeps its own accessor private, and the one thing
/// wanted here is not on the `Window` API: gpui writes `contentMinSize` at creation and never again.
fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
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
                        // The pane only starts the divider drag; the root is what follows the
                        // pointer, because a hitbox 6pt wide loses it on the first fast move.
                        .on_mouse_move(cx.listener(Self::preview_resize))
                        .on_mouse_up(MouseButton::Left, cx.listener(Self::preview_resize_end))
                        .size_full()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .overflow_hidden()
                                .child(self.list_column(body, headers, &palette, metrics.top))
                                // Column view's preview is the last column of the chain and
                                // scrolls with it, so the pane pinned to the window's trailing
                                // edge is not additionally correct there — it is a second one.
                                .when(self.preview_shown(), |row| {
                                    row.child(self.preview_pane(
                                        cursor.as_ref(),
                                        &palette,
                                        metrics.top,
                                        cx,
                                    ))
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
