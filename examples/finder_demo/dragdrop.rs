//! What a drag carries and where a drop lands: Finder's move, Option-copy and spring-loading.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use finder_fs::ops::Op;
use gpui::{Context, DragMoveEvent, IntoElement, Render, SharedString, Window, div};

use crate::app::FinderDemo;
use crate::ops::After;

/// What a row drag carries: the row the pointer grabbed, standing for the whole selection.
#[derive(Clone)]
pub(crate) struct RowDrag {
    pub(crate) key: SharedString,
}

/// Whether a drag carries this row, and so never targets it: Finder drops nothing onto itself.
pub(crate) fn carries(
    selected: &HashSet<SharedString>,
    grab: &RowDrag,
    key: &SharedString,
) -> bool {
    grab.key == *key || (selected.contains(&grab.key) && selected.contains(key))
}

pub(crate) struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl FinderDemo {
    /// Finder's spring-loading: a folder held under a drag opens on its own.
    pub(crate) fn hover_folder(
        &mut self,
        into: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        match self.spring {
            Some((key, since)) if key == into => {
                if now.duration_since(since) > Duration::from_millis(700) {
                    self.spring = None;
                    self.go(into, window, cx);
                }
            }
            _ => self.spring = Some((into, now)),
        }
    }

    /// A row drag reaching the sidebar becomes an AppKit file drag: the sidebar is an NSView.
    pub(crate) fn drag_to_sidebar(
        &mut self,
        event: &DragMoveEvent<RowDrag>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.event.position.x >= self.metrics().left {
            return;
        }
        let paths: Vec<PathBuf> = self
            .drag_keys(&event.drag(cx).key)
            .iter()
            .map(|key| PathBuf::from(key.as_ref()))
            .collect();
        if native_gpui::sidebar::drag_files(window, &paths) {
            cx.stop_active_drag(window);
        }
    }

    /// The rows a drag carries: the whole selection when the grabbed row is in it.
    pub(crate) fn drag_keys(&self, grabbed: &SharedString) -> Vec<SharedString> {
        match self.selected.contains(grabbed) {
            true => self.selected.iter().cloned().collect(),
            false => vec![grabbed.clone()],
        }
    }

    /// Finder's drop: the dragged rows move into `into`, or copy there while Option is held.
    pub(crate) fn drop_into(
        &mut self,
        into: &'static str,
        keys: &[SharedString],
        copy: bool,
        cx: &mut Context<Self>,
    ) {
        let items: Vec<PathBuf> = keys
            .iter()
            .map(|key| PathBuf::from(key.as_ref()))
            .filter(|path| path.parent() != Some(Path::new(into)))
            .collect();
        if items.is_empty() {
            return;
        }
        let into = PathBuf::from(into);
        let op = match copy {
            true => Op::Copy { items, into },
            false => Op::Move { items, into },
        };
        self.run_op(op, After::Select, cx);
    }
}
