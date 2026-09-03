//! The gallery view: the selection large, every item in a strip along the bottom.

use gpui::{AnyElement, ClickEvent, Window, div, point, prelude::*, px};

use crate::FinderDemo;
use crate::data::Line;
use crate::palette::Palette;
use crate::ui::artwork;

/// Measured on Finder's strip: 54-point plates 5 apart, 5 in from the left, 6 off the bottom.
const STRIP_CELL: f32 = 54.;
const STRIP_GAP: f32 = 5.;
const STRIP_INSET: f32 = 5.;
const STRIP_BOTTOM: f32 = 6.;
const STRIP_ICON: f32 = 48.;
const STRIP_RADIUS: f32 = 8.;

/// The hero fills the room above the strip to within this of its shorter side.
const HERO_INSET: f32 = 61.;
const HERO_MAX: f32 = 512.;

pub(crate) const STRIP_H: f32 = STRIP_CELL + 2. * STRIP_BOTTOM;

impl FinderDemo {
    pub(crate) fn gallery_body(
        &mut self,
        palette: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lines = self.lines(false);
        // Finder's gallery always shows something: with nothing chosen, the first item is.
        let has_cursor = self
            .cursor
            .as_ref()
            .is_some_and(|key| lines.iter().any(|line| &line.key == key));
        if let (false, Some(first)) = (has_cursor, lines.first().cloned()) {
            self.pick(&first, false, false);
            // The pane draws from the same cursor and may have drawn already this frame.
            cx.notify();
        }
        let chosen: Vec<Line> = lines
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .cloned()
            .collect();
        let cursor = self.cursor_line();

        let room_w = self.list_room - self.pane_w();
        let room_h =
            f32::from(window.viewport_size().height) - f32::from(self.metrics().top) - STRIP_H;
        let size = (room_w.min(room_h) - 2. * HERO_INSET).clamp(64., HERO_MAX);
        let hero = if chosen.len() > 1 {
            Some(self.stack_hero(&chosen, size))
        } else {
            cursor.as_ref().map(|line| artwork(&line.row, size))
        };

        let this = cx.entity().downgrade();
        let plate = palette.tertiary;
        let strip = div()
            .id("ng.finder.filmstrip")
            .h(px(STRIP_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(STRIP_GAP))
            .px(px(STRIP_INSET))
            .overflow_x_scroll()
            .track_scroll(&self.strip)
            .children(lines.into_iter().enumerate().map(|(index, line)| {
                let is_selected = self.selected.contains(&line.key);
                let this = this.clone();
                let clicked = line.clone();
                div()
                    .id(("ng.finder.frame", index))
                    .w(px(STRIP_CELL))
                    .h(px(STRIP_CELL))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(STRIP_RADIUS))
                    .when(is_selected, |cell| cell.bg(plate))
                    .child(artwork(&line.row, STRIP_ICON))
                    .on_click(move |event: &ClickEvent, window, cx| {
                        let (extend, toggle, twice) = match event {
                            ClickEvent::Mouse(mouse) => (
                                mouse.up.modifiers.shift,
                                mouse.up.modifiers.platform,
                                mouse.up.click_count >= 2,
                            ),
                            _ => (false, false, false),
                        };
                        let line = clicked.clone();
                        this.update(cx, |this: &mut FinderDemo, cx| {
                            this.pick(&line, extend, toggle);
                            if twice {
                                this.open(&line, window, cx);
                            }
                            cx.notify();
                        })
                        .ok();
                    })
            }));

        div()
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .pr(px(self.pane_w()))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(hero),
            )
            .child(strip)
            .into_any_element()
    }

    /// Scroll the strip so this frame shows whole.
    pub(crate) fn strip_reveal(&self, index: usize) {
        let start = STRIP_INSET + index as f32 * (STRIP_CELL + STRIP_GAP);
        let seen = -f32::from(self.strip.offset().x);
        let visible = f32::from(self.strip.bounds().size.width);
        let next = if start < seen {
            start - STRIP_INSET
        } else if start + STRIP_CELL > seen + visible {
            start + STRIP_CELL + STRIP_INSET - visible
        } else {
            return;
        };
        self.strip.set_offset(point(px(-next.max(0.)), px(0.)));
    }
}
