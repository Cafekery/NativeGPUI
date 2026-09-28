use std::time::Instant;

use gpui::{Context, Window};
use native_gpui::sidebar::{NativeSidebar, SidebarMetrics};

use crate::app::FinderDemo;
use crate::icons::FIRST_ROWS;
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
        self.set_view(view, cx);
        self.install_toolbar(window, cx);
        self.retitle(window);
        cx.notify();
    }

    /// Shows `view` once the rows are in its order: at once when they already are.
    pub(crate) fn set_view(&mut self, view: usize, cx: &mut Context<Self>) {
        let by = self.view_sort(view);
        self.pending_view = (by != (self.sort, self.ascending)).then_some(view);
        match self.pending_view {
            Some(_) => self.sort_held(by, cx),
            None => self.show_view(view),
        }
    }

    pub(crate) fn show_view(&mut self, view: usize) {
        if view != self.view {
            self.column_depth = 0;
        }
        self.view = view;
        self.preview = prefs::preview(view);
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

    /// Items a folder's first frame shows in this view.
    pub(crate) fn on_screen(&self) -> usize {
        let (width, height) = (
            f32::from(self.viewport.width),
            f32::from(self.viewport.height),
        );
        let fits = match self.view {
            ICON_VIEW => {
                let tile = cell(self.icon_size as f32);
                (width / tile).floor().max(1.) * (height / tile).ceil()
            }
            GALLERY_VIEW => width / self.strip_icon,
            _ => height / self.row_h(),
        };
        match fits >= 1. {
            true => (fits as usize + 1).min(FIRST_ROWS),
            false => FIRST_ROWS,
        }
    }

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

    /// The columns overflow the list, which then scrolls sideways: rounded to hundredths as GPUI
    /// rounds a scroll range, so drift in Name's fill never counts.
    pub(crate) fn list_bleeds(&self) -> bool {
        ((self.list_content_w() - self.list_room) * 100.).round() > 0.
    }

    pub(crate) fn columns(&self, window: &Window) -> usize {
        let metrics = self.metrics();
        let pane = match self.preview_shown() {
            true => self.preview_w,
            false => 0.,
        };
        let avail = f32::from(window.viewport_size().width) - f32::from(metrics.left) - pane - 24.;
        let tile = cell(self.icon_size as f32);
        let fits = ((avail / tile).floor() as usize).max(1);
        // Finder arranges a folder when its items first show, not while the window is still opening.
        match metrics == SidebarMetrics::default() || self.lines(false).is_empty() {
            true => fits,
            false => self.icons.hold(fits, tile),
        }
    }
}
