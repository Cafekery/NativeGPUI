//! The system Quick Look panel: opened over the selection, zooming out of the cursor's icon, and
//! kept in step with the window while it is up.

use std::path::PathBuf;

use gpui::{Context, KeyDownEvent, Keystroke, Modifiers, Window, prelude::*};
use native_gpui::{
    QuickLookKey, quick_look, quick_look_close, quick_look_update, quick_look_visible,
};

use crate::data::Line;
use crate::{FinderDemo, LIST_VIEW};

impl FinderDemo {
    /// Space: the system Quick Look panel over the selection, zooming out of the cursor's icon.
    pub(crate) fn toggle_quick_look(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if quick_look_visible() {
            quick_look_close();
            return;
        }
        let Some((items, current)) = self.quick_look_items() else {
            return;
        };
        let source = self.ql_icon.get();
        self.ql_shown = self.cursor.clone().map(|key| (key, source));
        let entity = cx.entity().downgrade();
        let handle = gpui::Window::window_handle(window);
        let app = cx.to_async();
        quick_look(window, items, current, source, move |key| {
            let entity = entity.clone();
            let mut app = app.clone();
            let _ = app.update_window(handle, |_, window, cx| {
                if let Some(entity) = entity.upgrade() {
                    entity.update(cx, |this, cx| this.quick_look_key(key, window, cx));
                }
            });
        });
    }

    /// Finder previews the whole selection with the cursor's item showing, or just the cursor.
    fn quick_look_items(&self) -> Option<(Vec<PathBuf>, usize)> {
        let cursor = self.cursor.clone()?;
        let lines = self.lines(self.view == LIST_VIEW);
        let mut items: Vec<&Line> = lines
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .collect();
        if !items.iter().any(|line| line.key == cursor) {
            items = lines.iter().filter(|line| line.key == cursor).collect();
        }
        let current = items.iter().position(|line| line.key == cursor)?;
        let paths = items
            .iter()
            .map(|line| PathBuf::from(line.row.path.as_ref()))
            .collect();
        Some((paths, current))
    }

    /// The panel follows the selection while it is up.
    pub(crate) fn sync_quick_look(&mut self, window: &Window) {
        if !quick_look_visible() {
            self.ql_shown = None;
            return;
        }
        let Some(cursor) = self.cursor.clone() else {
            return;
        };
        let source = self.ql_icon.get();
        if self
            .ql_shown
            .as_ref()
            .is_some_and(|(key, at)| *key == cursor && *at == source)
        {
            return;
        }
        let Some((items, current)) = self.quick_look_items() else {
            return;
        };
        quick_look_update(window, items, current, source);
        self.ql_shown = Some((cursor, source));
    }

    /// The panel's arrow keys move the selection the way the same keys do in the window.
    fn quick_look_key(&mut self, key: QuickLookKey, window: &mut Window, cx: &mut Context<Self>) {
        let key = match key {
            QuickLookKey::Up => "up",
            QuickLookKey::Down => "down",
            QuickLookKey::Left => "left",
            QuickLookKey::Right => "right",
        };
        let event = KeyDownEvent {
            keystroke: Keystroke {
                key: key.into(),
                modifiers: Modifiers::default(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        };
        self.on_key(&event, window, cx);
    }
}
