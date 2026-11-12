use gpui::{AnyElement, Context, Window, div, prelude::*, px};
use native_gpui::tokens::{self, SystemColor};
use native_gpui::{LabelAlign, native_label, native_slider};

use finder_fs::words;

use crate::app::FinderDemo;
use crate::chrome::path_bar::{BarPaint, EDGE_ROW, edge_row};
use crate::data::{self, available_bytes, byte_string};
use crate::frame::ns_window;
use crate::view::{GALLERY_VIEW, ICON_VIEW};
use crate::views::icon::{SIZE_STEPS, size_slider, slider_size};

const BAR_H: f32 = 28.;

const TEXT_TRAIL: f32 = 12.;
const SIDE_W: f32 = 120.;

impl FinderDemo {
    pub(crate) fn toggle_status_bar(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.status_bar_shown = !self.status_bar_shown;
        crate::prefs::set_shows_status_bar(self.status_bar_shown);
        cx.notify();
        let Some(ns_window) = ns_window(window) else {
            return;
        };
        let grow = f64::from(if self.status_bar_shown { BAR_H } else { -BAR_H });
        let mut frame = ns_window.frame();
        frame.origin.y -= grow;
        frame.size.height += grow;
        // Off this draw: a resize serviced inside it reaches gpui while it cannot take its app.
        cx.spawn(async move |_, _| ns_window.setFrame_display(frame, true))
            .detach();
    }

    pub(crate) fn follow_status_bar(&mut self) {
        self.status_bar_shown = crate::prefs::shows_status_bar();
    }

    pub(crate) fn status_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.lines(false).len();
        let chosen = self.selected.len();
        let available = byte_string(available_bytes(self.here()));
        let what = if chosen == 0 {
            words::fill(
                &words::plural("SB_COUNT", count, "^0 items"),
                &[&data::count(count)],
            )
        } else {
            let picked = words::fill(
                &words::finder("SB_1ofN_V1", "^1 of ^0 selected"),
                &[&data::count(count), &data::count(chosen)],
            );
            let size = self
                .stats
                .as_ref()
                .filter(|stats| stats.key == self.stats_key())
                .map(|stats| byte_string(stats.flat_bytes));
            match size {
                Some(size) => words::fill(&words::finder("SB1.1", "^0, ^1"), &[&picked, &size]),
                None => picked,
            }
        };
        let text = if self.airdrop_shown() {
            String::new()
        } else {
            words::fill(
                &words::finder("SB1", "^0, ^1 available"),
                &[&what, &available],
            )
        };
        let slider = matches!(self.view, ICON_VIEW | GALLERY_VIEW);
        let paint = BarPaint::now();

        div()
            .h(px(BAR_H))
            .flex_none()
            .relative()
            .flex()
            .items_center()
            .bg(paint.floor)
            .px(px(12.))
            .child(edge_row(0., paint.rule[0]))
            .child(edge_row(EDGE_ROW, paint.rule[1]))
            .child(div().w(px(SIDE_W)).flex_none())
            .child(
                div().flex_1().mr(px(TEXT_TRAIL)).child(
                    native_label("ng.finder.status.text", &text)
                        .font_size(tokens::small_system_font_size())
                        .tabular(true)
                        .color(SystemColor::SecondaryLabel)
                        .align(LabelAlign::Center),
                ),
            )
            .child(
                div()
                    .w(px(SIDE_W))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .when(slider, |row| {
                        row.child(
                            div().w(px(96.)).child(
                                native_slider(
                                    "ng.finder.zoom",
                                    size_slider(self.icon_options().size),
                                )
                                .range(0., SIZE_STEPS)
                                .on_change(cx.listener(
                                    |this, value: &f64, _, cx| {
                                        let size = slider_size(*value);
                                        if size != this.icon_options().size {
                                            this.change_icon_options(
                                                |options| options.icon_size = Some(size),
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }
}
