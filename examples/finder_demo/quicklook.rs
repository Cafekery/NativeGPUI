use std::path::PathBuf;

use gpui::{
    Bounds, Context, KeyDownEvent, Keystroke, Modifiers, Pixels, SharedString, Window, prelude::*,
};
use native_gpui::{
    QuickLookKey, quick_look, quick_look_close, quick_look_full_screen, quick_look_update,
    quick_look_visible,
};

use crate::app::FinderDemo;
use crate::data::Line;
use crate::view::LIST_VIEW;

fn key_forwarder(window: &Window, cx: &mut Context<FinderDemo>) -> impl Fn(QuickLookKey) + 'static {
    let entity = cx.entity().downgrade();
    let handle = window.window_handle();
    let app = cx.to_async();
    move |key| {
        let mut app = app.clone();
        let _ = app.update_window(handle, |_, window, cx| {
            if let Some(entity) = entity.upgrade() {
                entity.update(cx, |this, cx| this.quick_look_key(key, window, cx));
            }
        });
    }
}

impl FinderDemo {
    pub(crate) fn toggle_quick_look(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if quick_look_visible() {
            quick_look_close();
            return;
        }
        let Some((shown, source)) = self.quick_look_subject() else {
            return;
        };
        let Some((items, current)) = self.quick_look_items(&shown) else {
            return;
        };
        self.ql_shown = Some((shown, source));
        quick_look(window, items, current, source, key_forwarder(window, cx));
    }

    pub(crate) fn slideshow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((items, current)) = self
            .quick_look_subject()
            .and_then(|(shown, _)| self.quick_look_items(&shown))
        else {
            return;
        };
        self.ql_shown = None;
        quick_look_full_screen(window, items, current, key_forwarder(window, cx));
    }

    fn quick_look_subject(&self) -> Option<(SharedString, Option<Bounds<Pixels>>)> {
        match self.selected.is_empty() {
            true => Some((self.here().into(), None)),
            false => Some((self.cursor.clone()?, self.ql_icon.bounds())),
        }
    }

    fn quick_look_items(&self, shown: &SharedString) -> Option<(Vec<PathBuf>, usize)> {
        if self.selected.is_empty() {
            return Some((vec![PathBuf::from(shown.as_ref())], 0));
        }
        let lines = self.lines(self.view == LIST_VIEW);
        let mut items: Vec<&Line> = lines
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .collect();
        if !items.iter().any(|line| line.key == *shown) {
            items = lines.iter().filter(|line| line.key == *shown).collect();
        }
        let current = items.iter().position(|line| line.key == *shown)?;
        let paths = items
            .iter()
            .map(|line| PathBuf::from(line.row.path.as_ref()))
            .collect();
        Some((paths, current))
    }

    pub(crate) fn sync_quick_look(&mut self, window: &Window) {
        if !quick_look_visible() {
            self.ql_shown = None;
            return;
        }
        let Some(subject) = self.quick_look_subject() else {
            return;
        };
        let settled = match &self.ql_shown {
            Some(shown) => *shown == subject,
            // A window that did not open the panel never hands it its folder.
            None => self.selected.is_empty(),
        };
        if settled {
            return;
        }
        let Some((items, current)) = self.quick_look_items(&subject.0) else {
            return;
        };
        quick_look_update(window, items, current, subject.1);
        self.ql_shown = Some(subject);
    }

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
