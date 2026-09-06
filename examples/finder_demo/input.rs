//! The keys the list itself handles: everything the menu bar's commands do not claim.

use gpui::{Context, KeyDownEvent, ScrollStrategy, Window, point, px};
use native_gpui::{quick_look_close, quick_look_visible};

use crate::{COLUMN_VIEW, FinderDemo, GALLERY_VIEW, ICON_VIEW, LIST_VIEW};

impl FinderDemo {
    pub(crate) fn on_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        if key == "escape" && quick_look_visible() {
            quick_look_close();
            return;
        }
        if key == "escape" && self.type_filter.is_some() {
            self.clear_type_filter();
            cx.notify();
            return;
        }
        if key == "escape" && !self.cut.is_empty() {
            self.clear_cut(cx);
            return;
        }
        if key == "space" && self.cursor_line().is_some() {
            self.toggle_quick_look(window, cx);
            return;
        }
        let lines = self.focused_lines();
        if lines.is_empty()
            || (self.view == COLUMN_VIEW && self.column_key(key, modifiers, window, cx))
        {
            return;
        }
        let current = self
            .cursor
            .as_ref()
            .and_then(|key| self.line_at(&lines, key));
        let last = lines.len() - 1;
        let tree = self.view == LIST_VIEW;
        match key {
            "up" if modifiers.platform => self.up(window, cx),
            "down" | "o" if modifiers.platform => self.open_selected(window, cx),
            "up" | "down" | "left" | "right" if self.view == ICON_VIEW => {
                let columns = self.columns(window);
                let next = match current {
                    Some(at) => match key {
                        "up" if at >= columns => at - columns,
                        "down" if at + columns <= last => at + columns,
                        "left" if at % columns > 0 => at - 1,
                        "right" if at % columns + 1 < columns && at < last => at + 1,
                        _ => at,
                    },
                    None if matches!(key, "down" | "right") => 0,
                    None => last,
                };
                let line = &lines[next];
                if modifiers.shift {
                    self.selected.insert(line.key.clone());
                    self.cursor = Some(line.key.clone());
                } else {
                    self.pick(line, false, false);
                }
                let visible = self.list_room - self.pane_w();
                self.icons
                    .reveal(&line.key, next, columns, self.icon_size as f32, visible);
            }
            "home" | "end" if self.view == ICON_VIEW => self.icons.scroll_to_end(key == "end"),
            "left" | "right" if self.view == GALLERY_VIEW => {
                let next = match current {
                    Some(at) if key == "left" => at.saturating_sub(1),
                    Some(at) => (at + 1).min(last),
                    None => 0,
                };
                self.pick(&lines[next], modifiers.shift, false);
                self.strip_reveal(next);
            }
            "up" | "down" if self.view == GALLERY_VIEW => {}
            "home" | "end" if self.view == GALLERY_VIEW => {
                let x = if key == "end" {
                    -self.strip.max_offset().x
                } else {
                    px(0.)
                };
                self.strip.set_offset(point(x, px(0.)));
            }
            "up" | "down" => {
                let next = match current {
                    Some(at) if key == "up" => at.saturating_sub(1),
                    Some(at) => (at + 1).min(last),
                    None if key == "down" => 0,
                    None => last,
                };
                self.pick(&lines[next], modifiers.shift, false);
                self.reveal(&lines[next].key);
            }
            "right" | "left" if tree => self.disclose_selected(key == "right", modifiers.alt),
            "enter" => self.renaming = current.map(|at| lines[at].key.clone()),
            "tab" if self.view != COLUMN_VIEW => self.tab(modifiers.shift),
            "home" | "end" | "pageup" | "pagedown" if tree => {
                let (pads, _) = self.list_inset();
                let page = (f32::from(window.viewport_size().height) / self.row_h()) as usize;
                let top = self.scroll.0.borrow().base_handle.logical_scroll_top().0;
                let index = match key {
                    "home" => 0,
                    "end" => pads + last,
                    "pageup" => top.saturating_sub(page),
                    _ => (top + page).min(pads + last),
                };
                self.scroll.scroll_to_item(index, ScrollStrategy::Top);
            }
            _ if !modifiers.platform && !modifiers.control && !modifiers.alt => {
                let Some(typed) = event
                    .keystroke
                    .key_char
                    .as_deref()
                    .filter(|typed| typed.chars().all(|c| !c.is_control()))
                else {
                    return;
                };
                self.type_select(typed, cx);
            }
            _ => return,
        }
        if self.view == COLUMN_VIEW {
            self.column_arrived(window, cx);
        } else {
            cx.notify();
        }
    }
}
