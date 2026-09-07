//! One frame of the window: the surfaces the view sits on, and everything drawn over them.

use std::env;
use std::sync::OnceLock;
use std::time::Instant;

use gpui::{
    AnyElement, Context, MouseButton, Pixels, Render, Window, div, prelude::*, px, relative,
};
use native_gpui::sidebar::SidebarMetrics;
use native_gpui::tokens;
use native_gpui::{NativeScroller, native_root, system_corner_radius};
use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSWindow};
use objc2_foundation::NSSize;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::app::FinderDemo;
use crate::app::LIST_CONTEXT;
use crate::chrome::path_bar::PATH_H;
use crate::chrome::scope_bar;
use crate::chrome::surface::{band_glass, pane_surface};
use crate::menus;
use crate::menus::{ZoomIn, ZoomOut};
use crate::palette::Palette;
use crate::preview::LIST_MIN_W;
use crate::ui::{axis_locked, scroller};
use crate::view::{COLUMN_VIEW, GALLERY_VIEW, ICON_VIEW, LIST_VIEW, LIST_ZOOM_SECS};
use crate::views::header::HEADER_H;

pub(crate) fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }?.window()
}

/// What the view drew this frame: the rows, the header band above them, the path bar below.
struct Column {
    rows: AnyElement,
    headers: Option<AnyElement>,
    path_bar: Option<AnyElement>,
}

impl Render for FinderDemo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.sidebar.is_none() {
            self.install(window, cx);
        }
        if !self.toolbar {
            self.install_toolbar(window, cx);
        }
        if self.host.is_none() {
            self.host = ns_window(window);
        }
        let began = Instant::now();
        self.scale = window.scale_factor();
        self.frames += 1;
        if let Some(limit) = self.quit_at {
            window.request_animation_frame();
            if self.frames >= limit {
                std::process::exit(0);
            }
        }
        self.sync_window_min(window);
        self.sync_preview_window(window, cx);
        native_gpui::set_trailing_inset(window, self.pane_w());
        self.spring_tick(window, cx);
        if let Some((_, _, started)) = self.list_zoom {
            if started.elapsed().as_secs_f32() >= LIST_ZOOM_SECS {
                self.list_zoom = None;
            } else {
                window.request_animation_frame();
            }
        }

        let metrics = self.metrics();
        let corner = system_corner_radius(window);
        let palette = Palette::new(self.list_focused);
        let family = tokens::text_font_family(cx).unwrap_or_else(tokens::system_font_family);

        let path_bar = self
            .path_bar_shown
            .then(|| self.path_bar(&palette, window, cx));
        let status_bar = self.status_bar_shown.then(|| self.status_bar(&palette, cx));
        let body = match self.view {
            ICON_VIEW => self.icon_body(&palette, window, cx),
            COLUMN_VIEW => self.column_body(&palette, cx),
            GALLERY_VIEW => self.gallery_body(&palette, window, cx),
            _ => self.list_body(&palette, cx),
        };
        // The rows only: a menu whose element also spans the path bar swallows the crumbs' own.
        let body = menus::backdrop("ng.finder.background", cx.entity().downgrade(), body)
            .into_any_element();
        let headers = (self.view == LIST_VIEW).then(|| self.header_row(&palette, cx));

        let list_view = self.view == LIST_VIEW;
        let sheet = self
            .go_sheet
            .clone()
            .map(|text| self.go_sheet(&text, &palette, window, cx));
        let conflict_sheet = self
            .conflict
            .is_some()
            .then(|| self.conflict_sheet(&palette, cx));
        let jobs_popover = self.jobs_open.then(|| self.jobs_popover(&palette, cx));
        self.pump(window, cx);
        self.sync_quick_look(window);
        let root = native_root(
            div()
                .size_full()
                .relative()
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
                        .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(true, cx)))
                        .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(false, cx)))
                        .map(|element| self.menu_bar_actions(element, cx))
                        .map(|element| self.wire_undo_redo(element, cx))
                        .on_key_down(cx.listener(Self::on_key))
                        .on_drag_move(cx.listener(Self::drag_to_sidebar))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                // The sidebar is an NSView: this takes first responder back to
                                // the GPUI view so bare keys reach on_key.
                                native_gpui::focus_gpui(window);
                                window.focus(&this.focus, cx);
                            }),
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
                                    Column {
                                        rows: body,
                                        headers,
                                        path_bar,
                                    },
                                    &palette,
                                    metrics,
                                    corner,
                                    cx,
                                )),
                        )
                        .children(
                            status_bar.map(|bar| div().flex_none().pl(metrics.left).child(bar)),
                        )
                        .children(sheet)
                        .children(conflict_sheet)
                        .children(jobs_popover),
                ),
        );
        trace_frame(self, began);
        root
    }
}

/// `NG_FRAME_TRACE=1`: how long this window spent building each frame, and over how much.
fn trace_frame(demo: &FinderDemo, began: Instant) {
    static ON: OnceLock<bool> = OnceLock::new();
    if !ON.get_or_init(|| env::var("NG_FRAME_TRACE").is_ok()) {
        return;
    }
    eprintln!(
        "frame {} build {:.2}ms rows {} selected {} folders {}",
        demo.frames,
        began.elapsed().as_secs_f64() * 1000.,
        demo.folder().map_or(0, |folder| folder.rows.len()),
        demo.selected.len(),
        demo.folders.len(),
    );
}

impl FinderDemo {
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

    /// The rows' own scroller; the browser's columns carry theirs and the gallery has none.
    fn scroller_bar(&self) -> Option<NativeScroller> {
        match self.view {
            LIST_VIEW => {
                let inset = self.list_inset().0 as f32 * self.row_h();
                scroller(
                    "ng.finder.scroller",
                    &self.scroll.0.borrow().base_handle,
                    inset,
                )
            }
            ICON_VIEW => scroller("ng.finder.scroller", &self.icons.scroll, 0.),
            _ => None,
        }
    }

    fn list_column(
        &self,
        column: Column,
        palette: &Palette,
        metrics: SidebarMetrics,
        corner: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Column {
            rows,
            headers,
            path_bar,
        } = column;
        // The scope bar shares the hosted band with the toolbar, so it grows the top inset.
        let scope_h = if self.searching() {
            scope_bar::SCOPE_H
        } else {
            0.
        };
        let scope_bar = (scope_h > 0.).then(|| self.scope_bar(cx));
        let above = metrics.top;
        let top = above + px(scope_h);
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
                        .child(rows),
                ),
            );
        } else {
            surface = surface.child(rows);
        }
        div()
            .flex_1()
            .relative()
            .overflow_hidden()
            .child(surface)
            // A Layer::Above material blurs only what is painted before it: rows, glass, header.
            .when(banded || scope_h > 0., |column| {
                let height = top + if banded { px(HEADER_H) } else { px(0.) };
                column.child(band_glass(px(pane), height))
            })
            .children(scope_bar.map(|bar| {
                div()
                    .absolute()
                    .top(above)
                    .left_0()
                    .right(px(pane))
                    .h(px(scope_h))
                    .pl(metrics.left)
                    .child(bar)
            }))
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
            // A second scroller on the same handle, so header and rows clamp and scroll alike.
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
            .children(self.scroller_bar().map(|bar| {
                div()
                    .absolute()
                    .top(top + px(if banded { HEADER_H } else { 0. }))
                    .bottom(px(foot))
                    .right(px(pane))
                    .child(bar)
            }))
            .children(
                (self.view == LIST_VIEW)
                    .then(|| self.type_filter_pill(palette, pane, foot))
                    .flatten(),
            )
            .into_any_element()
    }
}
