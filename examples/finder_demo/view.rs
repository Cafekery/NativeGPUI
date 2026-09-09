use std::time::Instant;

use gpui::{Context, Window};
use native_gpui::sidebar::{NativeSidebar, SidebarMetrics};

use crate::app::FinderDemo;
use crate::prefs;
use crate::views::column::ICON as COLUMN_ICON;
use crate::views::header::PANE_INSET;
use crate::views::icon::cell;
use crate::views::list::{ICON_SIZE, ICON_SMALL, row_h};

pub(crate) const ICON_VIEW: usize = 0;

pub(crate) const LIST_VIEW: usize = 1;

pub(crate) const COLUMN_VIEW: usize = 2;

pub(crate) const GALLERY_VIEW: usize = 3;

pub(crate) const LIST_ZOOM_SECS: f32 = 0.2;

const ICON_STOPS: [f32; 9] = [16., 24., 32., 48., 64., 96., 128., 256., 512.];

const STRIP_STOPS: [f32; 3] = [32., 48., 64.];

impl FinderDemo {
    pub(crate) fn pick_view(&mut self, view: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.set_view(view);
        self.install_toolbar(window, cx);
        self.retitle(window);
        cx.notify();
    }

    pub(crate) fn set_view(&mut self, view: usize) {
        self.view = view;
        prefs::set_view(view);
        self.settle();
    }

    pub(crate) fn settle(&mut self) {
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

    pub(crate) fn metrics(&self) -> SidebarMetrics {
        self.sidebar
            .as_ref()
            .map(NativeSidebar::metrics)
            .unwrap_or_default()
    }

    pub(crate) fn preview_shown(&self) -> bool {
        self.preview && self.view != COLUMN_VIEW
    }

    pub(crate) fn list_icon_now(&self) -> f32 {
        match self.list_zoom {
            Some((from, to, started)) => {
                let t = (started.elapsed().as_secs_f32() / LIST_ZOOM_SECS).min(1.);
                from + (to - from) * gpui::ease_in_out(t)
            }
            None => self.list_icon,
        }
    }

    pub(crate) fn art_size(&self) -> f32 {
        match self.view {
            ICON_VIEW => self.icon_size as f32,
            GALLERY_VIEW => self.strip_icon,
            COLUMN_VIEW => COLUMN_ICON,
            _ => self.list_icon_now(),
        }
    }

    pub(crate) fn row_h(&self) -> f32 {
        row_h(self.list_icon_now())
    }

    /// ⌘= and ⌘-: the list toggles, the grid and the strip step, column view does nothing.
    pub(crate) fn zoom(&mut self, larger: bool, cx: &mut Context<Self>) {
        fn step(stops: &[f32], from: f32, larger: bool) -> Option<f32> {
            if larger {
                stops.iter().copied().find(|stop| *stop > from + 0.5)
            } else {
                stops.iter().rev().copied().find(|stop| *stop < from - 0.5)
            }
        }
        match self.view {
            LIST_VIEW => {
                let to = if larger { ICON_SIZE } else { ICON_SMALL };
                if to != self.list_icon {
                    let from = self.list_icon_now();
                    self.list_icon = to;
                    self.list_zoom = Some((from, to, Instant::now()));
                    prefs::set_list_icon(to);
                }
            }
            ICON_VIEW => {
                if let Some(size) = step(&ICON_STOPS, self.icon_size as f32, larger) {
                    self.icon_size = size as f64;
                    prefs::set_icon_size(size);
                }
            }
            GALLERY_VIEW => {
                if let Some(size) = step(&STRIP_STOPS, self.strip_icon, larger) {
                    self.strip_icon = size;
                    prefs::set_strip_icon(size);
                }
            }
            _ => return,
        }
        cx.notify();
    }

    pub(crate) fn list_content_w(&self) -> f32 {
        self.name_w + self.list_columns.iter().map(|spec| spec.width).sum::<f32>() + PANE_INSET
    }

    pub(crate) fn pane_w(&self) -> f32 {
        if self.preview_shown() {
            self.preview_w
        } else {
            0.
        }
    }

    /// The grid scrolls under the sidebar, so the pane's width still counts as room.
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
