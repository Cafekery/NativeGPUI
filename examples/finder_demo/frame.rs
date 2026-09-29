use std::env;
use std::sync::OnceLock;
use std::time::Instant;

use gpui::{
    AnyElement, Context, Div, MouseButton, Render, Stateful, Window, div, prelude::*, px, relative,
};
use native_gpui::sidebar::SidebarMetrics;
use native_gpui::tokens;
use native_gpui::{NativeScrollBars, native_root, native_scroll_bars};
use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSWindow};
use objc2_foundation::NSSize;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::app::FinderDemo;
use crate::app::LIST_CONTEXT;
use crate::chrome::path_bar::PATH_H;
use crate::chrome::surface::{band_glass, toolbar_rule};
use crate::dragdrop::RowDrag;
use crate::elastic::stretching;
use crate::menus;
use crate::menus::{ZoomIn, ZoomOut};
use crate::palette::Palette;
use crate::preview::LIST_MIN_W;
use crate::ui::axis_locked;
use crate::view::{COLUMN_VIEW, GALLERY_VIEW, ICON_VIEW, LIST_VIEW, LIST_ZOOM_SECS};
use crate::views::header::HEADER_H;
use crate::views::icon::TileDrag;
use crate::views::list::LIST_FOOT_GAP;

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
        let rev = self.selected.rev();
        if self.services_for != rev {
            self.services_for = rev;
            let files = self
                .selected
                .snapshot()
                .iter()
                .map(|key| key.to_string().into())
                .collect();
            native_gpui::services_selection(window, files);
        }
        if self.toolbar_for != Some(self.toolbar_state()) {
            self.install_toolbar(window, cx);
        }
        if window.is_window_active() {
            menus::remember(self, cx);
        }
        let title = self.title();
        if self.titled != title {
            window.set_window_title(&title);
            self.titled = title.into_owned();
        }
        if self.host.is_none() {
            self.host = ns_window(window);
        }
        let began = Instant::now();
        self.scale = window.scale_factor();
        self.viewport = window.viewport_size();
        self.frames += 1;
        if let Some(limit) = self.quit_at {
            window.request_animation_frame();
            if self.frames >= limit {
                std::process::exit(0);
            }
        }
        self.sync_window_min();
        self.sync_stats(cx);
        self.sync_preview_window(window, cx);
        native_gpui::set_trailing_inset(window, f32::from(self.metrics().right));
        self.spring_tick(window, cx);
        if self.bands.tick() {
            window.request_animation_frame();
        }
        if let Some((_, _, started)) = self.list_zoom {
            if started.elapsed().as_secs_f32() >= LIST_ZOOM_SECS {
                self.list_zoom = None;
            } else {
                window.request_animation_frame();
            }
        }

        let metrics = self.metrics();
        let palette = Palette::new(self.list_focused);
        let family = tokens::text_font_family(cx).unwrap_or_else(tokens::system_font_family);

        let airdrop = self.airdrop_shown();
        let path_bar = (self.path_bar_shown && !airdrop).then(|| self.path_bar(&palette, cx));
        let status_bar = self.status_bar_shown.then(|| self.status_bar(cx));
        let list_view = self.view == LIST_VIEW && !airdrop;
        let body = match self.view {
            _ if airdrop => self.airdrop_body(cx),
            ICON_VIEW => self.icon_body(&palette, window, cx),
            COLUMN_VIEW => self.column_body(&palette, window, cx),
            GALLERY_VIEW => self.gallery_body(&palette, window, cx),
            _ => self.list_body(&palette, cx),
        };
        // The rows only: a menu whose element also spans the path bar swallows the crumbs' own.
        let body = menus::backdrop("ng.finder.background", cx.entity().downgrade(), body)
            .into_any_element();
        let headers = list_view.then(|| self.header_row(&palette, cx));
        let save_sheet = self
            .save_sheet
            .as_deref()
            .map(|name| self.save_sheet(name, window, cx));
        let conflict_sheet = self
            .conflict
            .is_some()
            .then(|| self.conflict_sheet(&palette, cx));
        let jobs_popover = self.jobs_open.then(|| self.jobs_popover(cx));
        self.pump(window, cx);
        self.sync_quick_look(window);
        let root =
            native_root(
                div()
                    .size_full()
                    .relative()
                    .font_family(family)
                    .text_size(tokens::system_font_size())
                    .on_modifiers_changed(cx.listener(|_, _, _, cx| cx.notify()))
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
                            .capture_any_mouse_down(cx.listener(
                                |this, event: &gpui::MouseDownEvent, _, _| {
                                    this.pressed_at = event.position;
                                },
                            ))
                            .on_drag_move(cx.listener(Self::hand_off_drag::<RowDrag>))
                            .on_drag_move(cx.listener(Self::hand_off_drag::<TileDrag>))
                            .on_drop(cx.listener(Self::files_dropped))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    // The sidebar is an NSView; bare keys only reach on_key from GPUI's.
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
                                    .when(self.view != ICON_VIEW, |row| row.overflow_hidden())
                                    .when(!list_view, |row| row.pl(metrics.left))
                                    .child(self.list_column(
                                        body, headers, path_bar, &palette, metrics, cx,
                                    )),
                            )
                            .children(
                                status_bar.map(|bar| div().flex_none().pl(metrics.left).child(bar)),
                            )
                            .children(save_sheet)
                            .children(conflict_sheet)
                            .children(jobs_popover),
                    ),
            );
        trace_frame(self, began);
        root
    }
}

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
    fn sync_window_min(&mut self) {
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
        let Some(host) = &self.host else {
            return;
        };
        self.window_min_w = width;
        let height = host.contentMinSize().height;
        host.setContentMinSize(NSSize {
            width: f64::from(width),
            height,
        });
    }

    fn scroll_bars(&self) -> Option<NativeScrollBars> {
        let bars = native_scroll_bars("ng.finder.scroll_bars");
        match self.view {
            LIST_VIEW => Some(
                bars.vertical(&self.scroll.0.borrow().base_handle)
                    .horizontal(&self.hscroll),
            ),
            ICON_VIEW => Some(
                bars.vertical(&self.icons.scroll)
                    .horizontal(&self.icons.scroll),
            ),
            _ => None,
        }
    }

    fn list_column(
        &self,
        rows: AnyElement,
        headers: Option<AnyElement>,
        path_bar: Option<AnyElement>,
        palette: &Palette,
        metrics: SidebarMetrics,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scope_h = if self.in_search() {
            self.search_bars_h()
        } else {
            0.
        };
        let search_bars = (scope_h > 0.).then(|| (self.scope_bar(cx), self.criteria_bar(cx)));
        let above = metrics.top;
        let top = above + px(scope_h);
        let bar = if path_bar.is_some() { PATH_H } else { 0. };
        let banded = headers.is_some();
        let header_h = px(if banded { HEADER_H } else { 0. });
        let width = metrics.left + px(self.list_room.max(self.list_content_w()));
        let sideways = self.bands.sideways.shown().x;
        let scroll_sideways = |scroller: Stateful<Div>, band: Div, cx: &mut Context<Self>| {
            axis_locked(
                scroller
                    .overflow_x_scroll()
                    .track_scroll(&self.hscroll)
                    .on_scroll_wheel(stretching(
                        |this| (&mut this.bands.sideways, this.hscroll.clone()),
                        cx,
                    )),
            )
            .child(
                band.h_full()
                    .w(width)
                    .min_w(relative(1.))
                    .relative()
                    .left(sideways)
                    .pl(metrics.left),
            )
        };
        let icons = self.view == ICON_VIEW;
        let surface = div()
            .absolute()
            .left_0()
            .right(if icons { -metrics.right } else { px(0.) })
            .bottom(px(bar))
            .top(if banded { px(0.) } else { top })
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(!banded && !icons && toolbar_rule(), |column| {
                column.border_t_1().border_color(palette.separator)
            })
            .child(if banded {
                scroll_sideways(
                    div().id("ng.finder.hscroll").size_full(),
                    div().flex().flex_col().child(rows),
                    cx,
                )
                .into_any_element()
            } else {
                rows
            });
        div()
            .flex_1()
            .relative()
            .when(!icons, |column| column.overflow_hidden())
            .child(surface)
            // The pane shows the icons beneath its glass, but a GPUI drag would still hit-test them.
            .when(icons, |column| {
                column.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right(-metrics.right)
                        .w(metrics.right)
                        .occlude(),
                )
            })
            // A Layer::Above material blurs only what is painted before it: the rows and the header.
            .when(banded || scope_h > 0., |column| {
                column.child(band_glass(top + header_h))
            })
            .children(search_bars.map(|(scope, criteria)| {
                div()
                    .absolute()
                    .top(above)
                    .left_0()
                    .right_0()
                    .h(px(scope_h))
                    .pl(metrics.left)
                    .flex()
                    .flex_col()
                    .child(scope)
                    .child(criteria)
            }))
            .children(path_bar.map(|bar| {
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    // Only the list view reaches here unshifted; the others are already inset.
                    .when(banded, |bar_row| bar_row.pl(metrics.left))
                    .child(bar)
            }))
            .children(headers.map(|headers| {
                scroll_sideways(
                    div()
                        .id("ng.finder.hscroll.header")
                        .absolute()
                        .top(top)
                        .left_0()
                        .right_0()
                        .h(px(HEADER_H)),
                    div().child(headers),
                    cx,
                )
            }))
            .children(self.scroll_bars().map(|bars| {
                div()
                    .absolute()
                    .top(top + header_h)
                    .bottom(px(bar))
                    .left(if banded { metrics.left } else { px(0.) })
                    .right_0()
                    .child(bars)
            }))
            .children(
                (self.view == LIST_VIEW)
                    .then(|| self.type_filter_pill(palette, bar + LIST_FOOT_GAP))
                    .flatten(),
            )
            .into_any_element()
    }
}
