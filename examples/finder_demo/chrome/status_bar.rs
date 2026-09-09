use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use native_gpui::native_slider;
use native_gpui::tokens;

use crate::app::FinderDemo;
use crate::chrome::surface::bar_tint;
use crate::data::{available_bytes, byte_string};
use crate::palette::Palette;
use crate::prefs;
use crate::view::{GALLERY_VIEW, ICON_VIEW};

pub(crate) const BAR_H: f32 = 26.;

impl FinderDemo {
    pub(crate) fn status_bar(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
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

        div()
            .h(px(BAR_H))
            .flex_none()
            .flex()
            .items_center()
            .when_some(bar_tint(), |bar, tint| bar.bg(tint))
            .px(px(12.))
            .border_t_1()
            .border_color(palette.separator)
            .text_size(tokens::small_system_font_size())
            .text_color(palette.secondary)
            .child(div().w(px(120.)).flex_none())
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .whitespace_nowrap()
                    .child(SharedString::from(text)),
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
