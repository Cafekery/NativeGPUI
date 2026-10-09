use gpui::{Context, Size, Window, size};
use native_gpui::sidebar::{NativeSidebar, SidebarMetrics};

use crate::app::FinderDemo;
use crate::icons::FIRST_ROWS;
use crate::prefs;
use crate::views::column::ICON as COLUMN_ICON;
use crate::views::gallery::GalleryLook;
use crate::views::header::{LAST_EXTRA, PANE_INSET};
use crate::views::icon::Arrangement;
use crate::views::list::{ICON_SIZE, ICON_SMALL, row_h};

pub(crate) const ICON_VIEW: usize = 0;
pub(crate) const LIST_VIEW: usize = 1;
pub(crate) const COLUMN_VIEW: usize = 2;
pub(crate) const GALLERY_VIEW: usize = 3;

pub(crate) const LIST_ZOOM_SECS: f32 = 0.2;
const ICON_STOPS: [f32; 9] = [16., 24., 32., 48., 64., 96., 128., 256., 512.];
const STRIP_STOPS: [f32; 3] = [32., 48., 64.];

/// Rounded to hundredths as GPUI rounds a scroll range, so drift in Name's fill never counts.
pub(crate) fn overflows(content: f32, room: f32) -> bool {
    ((content - room) * 100.).round() > 0.
}

impl FinderDemo {
    pub(crate) fn pick_view(&mut self, view: usize, window: &mut Window, cx: &mut Context<Self>) {
        prefs::set_view(view);
        self.browse_as(view, window, cx);
    }

    pub(crate) fn browse_as(&mut self, view: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.browse_view = view;
        self.set_view(view, cx);
        self.install_toolbar(window, cx);
        self.retitle(window);
        cx.notify();
    }

    pub(crate) fn open_as_kept(&mut self, cx: &mut Context<Self>) {
        let view = match prefs::opens_in(self.here()) {
            Some((view, true)) => {
                self.browse_view = view;
                view
            }
            Some((view, false)) => view,
            None => self.browse_view,
        };
        if view != self.pending_view.unwrap_or(self.view) {
            self.set_view(view, cx);
        }
    }

    pub(crate) fn always_open(&mut self, on: bool) {
        prefs::set_opens_in(self.here(), on.then_some((self.view, false)));
    }

    pub(crate) fn always_browse(&mut self, on: bool) {
        prefs::set_opens_in(self.here(), Some((self.view, on)));
        if on {
            self.browse_view = self.view;
        }
    }

    pub(crate) fn set_view(&mut self, view: usize, cx: &mut Context<Self>) {
        let by = self.view_sort(view);
        self.pending_view = (by != self.order()).then_some(view);
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
        if view == GALLERY_VIEW {
            self.gallery_look = GalleryLook::load(self.here());
        }
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
        self.preview && self.view != COLUMN_VIEW && !self.airdrop_shown()
    }

    pub(crate) fn list_icon_now(&self) -> f32 {
        match self.list_zoom {
            Some((from, to, started)) => {
                let t = (started.elapsed().as_secs_f32() / LIST_ZOOM_SECS).min(1.);
                from + (to - from) * gpui::ease_in_out(t)
            }
            None => self.list_look.icon,
        }
    }

    pub(crate) fn art_size(&self) -> f32 {
        match self.view {
            ICON_VIEW => self.icon_options().size,
            GALLERY_VIEW => self.gallery_look.thumb,
            COLUMN_VIEW => COLUMN_ICON,
            _ => self.list_icon_now(),
        }
    }

    pub(crate) fn row_h(&self) -> f32 {
        row_h(self.list_icon_now(), self.list_look.text)
    }

    pub(crate) fn on_screen(&self) -> usize {
        let (width, height) = (
            f32::from(self.viewport.width),
            f32::from(self.viewport.height),
        );
        let fits = match self.view {
            ICON_VIEW => {
                let tile = self.icon_options().cell();
                (width / tile.width).floor().max(1.) * (height / tile.height).ceil()
            }
            GALLERY_VIEW => width / self.gallery_look.thumb,
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
                let icon = if larger { ICON_SIZE } else { ICON_SMALL };
                self.change_list_look(|look| look.icon = icon, cx);
            }
            ICON_VIEW => {
                if let Some(size) = step(&ICON_STOPS, self.icon_options().size, larger) {
                    self.change_icon_options(|options| options.icon_size = Some(size), cx);
                }
            }
            GALLERY_VIEW => {
                if let Some(size) = step(&STRIP_STOPS, self.gallery_look.thumb, larger) {
                    self.change_gallery_look(|look| look.thumb = size, cx);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn list_content_w(&self) -> f32 {
        let columns: f32 = self.list_columns.iter().map(|spec| spec.width).sum();
        let extra = match self.list_columns.is_empty() {
            true => 0.,
            false => LAST_EXTRA,
        };
        self.name_w + columns + extra + PANE_INSET
    }

    pub(crate) fn list_bleeds(&self) -> bool {
        overflows(self.list_content_w(), self.list_room)
    }

    pub(crate) fn icon_room(&self, width: f32) -> Size<f32> {
        let pane = match self.preview_shown() {
            true => self.preview_w,
            false => 0.,
        };
        let height = f32::from(self.icons.scroll.bounds().size.height - self.under_bars());
        size(
            width - f32::from(self.metrics().left) - pane - 4.,
            match height > 0. {
                true => height,
                false => f32::from(self.viewport.height),
            },
        )
    }

    pub(crate) fn fits(&self, window: &Window) -> usize {
        let room = self.icon_room(f32::from(window.viewport_size().width));
        self.icon_options().lanes(room)
    }

    pub(crate) fn columns(&self, window: &Window) -> usize {
        let sorted = matches!(self.arrangement(), Some(Arrangement::By(_)));
        // Finder arranges a folder when its items first show, not while its window opens.
        match sorted || self.metrics() == SidebarMetrics::default() || self.lines(false).is_empty()
        {
            true => self.fits(window),
            false => {
                let options = self.icon_options();
                let now = f32::from(window.viewport_size().width);
                let first = self.icon_room(self.icons.first_width(now));
                self.icons.hold(options.lanes(first), options)
            }
        }
    }
}
