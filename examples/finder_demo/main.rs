//! A Finder front end: real macOS chrome, an invented filesystem, `NG_QUIT=<frames>` to exit.

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
    AnyElement, App, Bounds, ClipboardItem, FocusHandle, KeyBinding, KeyDownEvent, MouseButton,
    ScrollHandle, ScrollStrategy, SharedString, UniformListScrollHandle, Window, WindowBounds,
    WindowOptions, actions, div, point, prelude::*, px, relative, size,
};
use native_gpui::sidebar::{NativeSidebar, SidebarMetrics};
use native_gpui::tokens::{self, SystemTheme};
use native_gpui::{install_menus, native_root, window_options};
use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSWindow};
use objc2_foundation::NSSize;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::data::{Column, Folder, Line, Row, build_store};
use crate::header::{
    ColumnSpec, HEADER_H, HeaderDrag, NAME_MIN_W, PANE_INSET, band_glass, default_columns, pane_cap,
};
use crate::icon::{Grid, cell};
use crate::palette::Palette;
use crate::preview::{LIST_MIN_W, PREVIEW_W, PreviewWindow};
use crate::ui::{TAGS, axis_locked};

pub(crate) const ICON_VIEW: usize = 0;
pub(crate) const LIST_VIEW: usize = 1;
pub(crate) const COLUMN_VIEW: usize = 2;
pub(crate) const GALLERY_VIEW: usize = 3;

pub(crate) const LIST_CONTEXT: &str = "FinderList";

actions!(finder_demo, [SelectAll, CopyNames]);

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
    pub(crate) tags: HashMap<SharedString, usize>,
    pub(crate) preview: bool,
    pub(crate) preview_w: f32,
    pub(crate) preview_window: Option<PreviewWindow>,
    pub(crate) preview_opening: bool,
    pub(crate) preview_expanded: bool,
    /// What the pane window reads to lay itself out: the list's focus and the room beside the sidebar.
    pub(crate) list_focused: bool,
    pub(crate) list_room: f32,
    pub(crate) path_bar_shown: bool,
    pub(crate) status_bar_shown: bool,
    pub(crate) icon_size: f64,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) hscroll: ScrollHandle,
    pub(crate) column_scroll: ScrollHandle,
    pub(crate) column_depth: usize,
    pub(crate) icons: Grid,
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
            preview: true,
            preview_w: PREVIEW_W,
            preview_window: None,
            preview_opening: false,
            preview_expanded: false,
            list_focused: false,
            list_room: 0.,
            path_bar_shown: false,
            status_bar_shown: false,
            icon_size: 64.,
            scroll: UniformListScrollHandle::new(),
            hscroll: ScrollHandle::new(),
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
            key @ ("up" | "down") if modifiers.platform => {
                if key == "up" {
                    self.up(window, cx);
                } else if let Some(line) = current.map(|index| lines[index].clone()) {
                    self.open(&line, window, cx);
                }
            }
            key @ ("up" | "down" | "left" | "right") if self.view == ICON_VIEW => {
                let step: isize = match key {
                    "up" => -(self.columns(window) as isize),
                    "down" => self.columns(window) as isize,
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
                self.icons
                    .reveal(&line.key, next, self.columns(window), self.icon_size as f32);
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
                self.scroll
                    .scroll_to_item(next + self.list_inset().0, ScrollStrategy::Center);
                cx.notify();
            }
            key @ ("right" | "left") => {
                let open = key == "right";
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

    /// The list's own width: its columns never shrink to fit, so the visible strip scrolls over them.
    fn list_content_w(&self) -> f32 {
        self.name_w + self.list_columns.iter().map(|spec| spec.width).sum::<f32>() + PANE_INSET
    }

    fn list_column(
        &self,
        body: AnyElement,
        headers: Option<AnyElement>,
        palette: &Palette,
        metrics: SidebarMetrics,
    ) -> AnyElement {
        let top = metrics.top;
        let banded = headers.is_some();
        let opaque = matches!(self.view, ICON_VIEW | LIST_VIEW | COLUMN_VIEW);
        let mut surface = div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .top(if banded { px(0.) } else { top })
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(opaque, |surface| surface.bg(palette.surface))
            .when(!banded && self.view != ICON_VIEW, |column| {
                column.border_t_1().border_color(palette.separator)
            });
        // The list starts under the sidebar and scrolls sideways beneath it and the pane; the pane's
        // width is padded on so the last column can be scrolled clear of it.
        let pane = if self.preview_shown() {
            self.preview_w
        } else {
            0.
        };
        let width = metrics.left + px(self.list_content_w() + pane);
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
                column.child(band_glass(metrics.left, px(pane), top))
            })
            .when(banded && pane > 0., |column| column.child(pane_cap(px(pane), top)))
            // A second scroller on the same handle, so the header clamps exactly as the rows do.
            .children(headers.map(|headers| {
                axis_locked(
                    div()
                        .id("ng.finder.hscroll.header")
                        .absolute()
                        .top(top)
                        .left_0()
                        .right_0()
                        .h(px(HEADER_H))
                        .overflow_x_scroll()
                        .track_scroll(&self.hscroll),
                )
                .child(
                    div()
                        .h_full()
                        .w(width)
                        .min_w(relative(1.))
                        .pl(metrics.left)
                        .child(headers),
                )
            }))
            .into_any_element()
    }

    pub(crate) fn columns(&self, window: &Window) -> usize {
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
        let palette = Palette::new(self.list_focused);
        let family = tokens::text_font_family(cx).unwrap_or_else(tokens::system_font_family);

        let path_bar = self.path_bar(&palette);
        let status_bar = self.status_bar(&palette, cx);
        let body = match self.view {
            ICON_VIEW => self.icon_body(&palette, window, cx),
            COLUMN_VIEW => self.column_body(&palette, cx),
            GALLERY_VIEW => self.gallery_body(&palette, cx),
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
                                .child(self.list_column(body, headers, &palette, metrics)),
                        )
                        .when(self.path_bar_shown, |c| {
                            c.child(div().flex_none().pl(metrics.left).child(path_bar))
                        })
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
