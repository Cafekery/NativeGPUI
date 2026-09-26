use gpui::{AnyElement, Context, Window, div, prelude::*, px};
use native_gpui::tokens::{self, SystemColor};
use native_gpui::{LabelAlign, native_label, native_slider};

use crate::app::FinderDemo;
use crate::chrome::path_bar::{BarPaint, EDGE_ROW, edge_row};
use crate::data::{available_bytes, byte_string};
use crate::frame::ns_window;
use crate::prefs;
use crate::view::{GALLERY_VIEW, ICON_VIEW};

pub(crate) const BAR_H: f32 = 28.;

/// Finder centres the count in the bar less this much at its trailing end.
const TEXT_TRAIL: f32 = 12.;

impl FinderDemo {
    /// Finder grows the window by the bar, keeping its top edge, rather than shortening the list.
    pub(crate) fn toggle_status_bar(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.status_bar_shown = !self.status_bar_shown;
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

    pub(crate) fn status_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.lines(false).len();
        let chosen = self.selected.len();
        let available = byte_string(available_bytes(self.here()));
        let size = self
            .stats
            .as_ref()
            .filter(|stats| stats.key == self.stats_key())
            .map(|stats| byte_string(stats.flat_bytes));
        let text = match (chosen, size) {
            (0, _) => format!("{count} items, {available} available"),
            (chosen, None) => format!("{chosen} of {count} selected, {available} available"),
            (chosen, Some(size)) => {
                format!("{chosen} of {count} selected, {size}, {available} available")
            }
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
            .child(div().w(px(120.)).flex_none())
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
                    .w(px(120.))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .when(slider, |row| {
                        row.child(
                            div().w(px(96.)).child(
                                native_slider("ng.finder.zoom", self.icon_size)
                                    .range(32., 128.)
                                    .on_change(cx.listener(|this, value: &f64, _, cx| {
                                        this.icon_size = (value / 8.).round() * 8.;
                                        prefs::set_icon_size(this.icon_size as f32);
                                        cx.notify();
                                    })),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }
}
