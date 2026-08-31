//! The icon (grid) view.
//!
//! Owns `FinderDemo::icon_body` and the tile metrics. The grid's column count is
//! `FinderDemo::columns`, which lives in `main` because the keyboard needs it too.
//!
//! Deliberately not here: the gallery view, which is a different arrangement of the same rows and
//! lives in `gallery`.

use gpui::{AnyElement, ClickEvent, Window, div, prelude::*, px, uniform_list};
use native_gpui::tokens;

use crate::FinderDemo;
use crate::palette::Palette;
use crate::ui::{glyph, tag_dot};

/// Icon-view tile: how much wider than the icon the cell is, and the gap between cells. The pitch
/// is the sum, and leaving the gap out of it is how the last column ends up clipped.
pub(crate) const TILE_MARGIN: f32 = 56.;
pub(crate) const TILE_GAP: f32 = 8.;

impl FinderDemo {
    pub(crate) fn icon_body(
        &mut self,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lines = self.lines(false);
        let columns = self.columns(window, cx);
        let rows = lines.len().div_ceil(columns).max(1);
        let this = cx.entity().downgrade();
        let selected = self.selected.clone();
        let tags = self.tags.clone();
        let size = self.icon_size as f32;
        let label = palette.label;
        let selection = palette.selection;
        let selection_text = palette.selection_text;

        uniform_list("ng.finder.icons", rows, move |range, _, _| {
            range
                .map(|band| {
                    let mut strip = div()
                        .h(px(size + 48.))
                        .flex()
                        .items_start()
                        .gap(px(TILE_GAP))
                        .px(px(12.))
                        .pt(px(8.));
                    for offset in 0..columns {
                        let index = band * columns + offset;
                        let Some(line) = lines.get(index) else { break };
                        let is_selected = selected.contains(&line.key);
                        let this = this.clone();
                        let clicked = line.clone();
                        let mut caption = div()
                            .px(px(5.))
                            .rounded(px(4.))
                            .max_w(px(size + TILE_MARGIN - 4.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(tokens::small_system_font_size())
                            .text_color(if is_selected { selection_text } else { label })
                            .child(line.row.name.clone());
                        if is_selected {
                            caption = caption.bg(selection);
                        }
                        let mut tile = div()
                            .id(("ng.finder.tile", index))
                            .w(px(size + TILE_MARGIN))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(4.))
                            .child(glyph(line.row.face, size))
                            .child(caption);
                        if let Some(tag) = tags.get(&line.key) {
                            tile = tile.child(tag_dot(*tag));
                        }
                        strip =
                            strip.child(tile.on_click(move |event: &ClickEvent, window, cx| {
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
                            }));
                    }
                    strip
                })
                .collect::<Vec<_>>()
        })
        .flex_1()
        .track_scroll(self.scroll.clone())
        .into_any_element()
    }
}
