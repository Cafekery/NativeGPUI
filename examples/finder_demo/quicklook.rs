use std::path::PathBuf;

use gpui::{
    AnyWindowHandle, Bounds, KeyDownEvent, Keystroke, Modifiers, Pixels, SharedString, Window,
    prelude::*,
};
use native_gpui::{
    QuickLookKey, quick_look, quick_look_close, quick_look_full_screen, quick_look_update,
    quick_look_visible,
};

use crate::app::FinderDemo;
use crate::data::Line;
use crate::remote::Then;
use crate::view::LIST_VIEW;

type Shown = (SharedString, Option<Bounds<Pixels>>);

/// Quick Look of cached copies of server files, once they download.
pub(crate) struct Look {
    pub(crate) window: AnyWindowHandle,
    items: Vec<PathBuf>,
    current: usize,
    shown: Shown,
    /// The panel is up and follows the selection.
    update: bool,
}

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
        if crate::remote::is_remote(&shown) {
            return self.look_remote(items, current, (shown, source), false, window, cx);
        }
        self.ql_shown = Some((shown, source));
        quick_look(window, items, current, source, key_forwarder(window, cx));
    }

    fn look_remote(
        &mut self,
        items: Vec<PathBuf>,
        current: usize,
        shown: Shown,
        update: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let window = window.window_handle();
        let look = |items| Look {
            window,
            items,
            current,
            shown,
            update,
        };
        // Moving on before a file lands leaves its download hogging the next one's bandwidth.
        if let Some(job) = self.ql_fetch.take() {
            self.remote_jobs.cancel(job);
        }
        self.ql_fetch = self.fetch(items, |items| Then::QuickLook(look(items)), cx);
    }

    pub(crate) fn looked(&mut self, look: Look, window: &mut Window, cx: &mut Context<Self>) {
        let Look {
            items,
            current,
            shown,
            update,
            ..
        } = look;
        if !update && !quick_look_visible() {
            self.ql_shown = Some(shown.clone());
            quick_look(window, items, current, shown.1, key_forwarder(window, cx));
        } else if update && quick_look_visible() {
            if let Some(now) = self.ql_shown.as_ref().filter(|now| now.0 == shown.0) {
                quick_look_update(window, items, current, now.1);
            }
        }
    }

    pub(crate) fn slideshow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((items, current)) = self
            .quick_look_subject()
            .filter(|(shown, _)| !crate::remote::is_remote(shown))
            .and_then(|(shown, _)| self.quick_look_items(&shown))
        else {
            return;
        };
        self.ql_shown = None;
        quick_look_full_screen(window, items, current, key_forwarder(window, cx));
    }

    fn quick_look_subject(&self) -> Option<(SharedString, Option<Bounds<Pixels>>)> {
        if self.selected.is_empty() {
            return Some((self.here().into(), None));
        }
        Some((self.cursor.clone()?, self.ql_icon.bounds()))
    }

    fn quick_look_items(&self, shown: &SharedString) -> Option<(Vec<PathBuf>, usize)> {
        let remote = crate::remote::is_remote(shown);
        if self.selected.is_empty() {
            return (!remote).then(|| (vec![PathBuf::from(shown.as_ref())], 0));
        }
        let lines = self.lines(self.view == LIST_VIEW);
        let mut items: Vec<&Line> = lines
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .collect();
        if !items.iter().any(|line| line.key == *shown) {
            items = lines.iter().filter(|line| line.key == *shown).collect();
        }
        items.retain(|line| !remote || !line.row.is_folder());
        let current = items.iter().position(|line| line.key == *shown)?;
        let paths = items
            .iter()
            .map(|line| PathBuf::from(line.row.path.as_ref()))
            .collect();
        Some((paths, current))
    }

    pub(crate) fn sync_quick_look(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !quick_look_visible() {
            self.ql_shown = None;
            return;
        }
        let Some(subject) = self.quick_look_subject() else {
            return;
        };
        let remote = crate::remote::is_remote(&subject.0);
        let settled = match self.ql_shown.as_mut() {
            // A server file is fetched once; its icon moving only moves the zoom anchor.
            Some(shown) if remote && shown.0 == subject.0 => {
                shown.1 = subject.1;
                true
            }
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
        self.ql_shown = Some(subject.clone());
        if remote {
            cx.defer_in(window, move |this, window, cx| {
                this.look_remote(items, current, subject, true, window, cx)
            });
            return;
        }
        quick_look_update(window, items, current, subject.1);
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
