//! Every file operation the window starts: what it does when it lands, and what it says when the
//! file system refuses.

pub(crate) mod clipboard;
pub(crate) mod conflicts;
pub(crate) mod jobs;
pub(crate) mod undo;

use std::path::PathBuf;

use finder_fs::ops::{Conflict, Op};
use gpui::{Context, SharedString};

use crate::app::FinderDemo;
use crate::data::{intern_path, split_key};

/// What a finished operation leaves behind it.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum After {
    Nothing,
    Select,
    Rename,
}

/// The folders an operation changes, which are read again once it lands.
pub(crate) fn touched(op: &Op) -> Vec<PathBuf> {
    let parents = |items: &[PathBuf]| {
        items
            .iter()
            .filter_map(|item| item.parent().map(PathBuf::from))
            .collect::<Vec<_>>()
    };
    match op {
        Op::Move { items, into } | Op::Copy { items, into } => {
            let mut dirs = parents(items);
            dirs.push(into.clone());
            dirs
        }
        Op::Trash(items) | Op::DeleteImmediately(items) | Op::Duplicate(items) => parents(items),
        Op::Rename { path, .. } => parents(std::slice::from_ref(path)),
        Op::NewFolder { into, .. } | Op::NewFile { into, .. } => vec![into.clone()],
    }
}

impl FinderDemo {
    /// Every file operation runs through the queue; a move or copy that would collide with an
    /// existing name shows Finder's conflict sheet first.
    pub(crate) fn run_op(&mut self, op: Op, after: After, cx: &mut Context<Self>) {
        match conflicts::split(op, after) {
            conflicts::Split::Clean(op) => self.submit_op(op, Conflict::KeepBoth, after, cx),
            conflicts::Split::Conflicted { clean, sheet } => {
                if let Some(clean) = clean {
                    self.submit_op(clean, Conflict::KeepBoth, After::Nothing, cx);
                }
                self.conflict = Some(sheet);
                cx.notify();
            }
        }
    }

    pub(crate) fn settled(
        &mut self,
        touched: Vec<PathBuf>,
        results: Vec<PathBuf>,
        after: After,
        cx: &mut Context<Self>,
    ) {
        let landed = results.iter().filter_map(|path| path.parent());
        for dir in touched.iter().map(PathBuf::as_path).chain(landed) {
            let id = intern_path(dir);
            self.read_folder(id, cx);
        }
        if after == After::Nothing {
            return;
        }
        let keys: Vec<SharedString> = results
            .iter()
            .map(|path| SharedString::from(path.to_string_lossy().into_owned()))
            .collect();
        self.selected = keys.iter().cloned().collect();
        self.cursor = keys.last().cloned();
        self.anchor = self.cursor.clone();
        self.reveal_next = self.cursor.clone();
        if after == After::Rename {
            self.renaming = self.cursor.clone();
        }
        cx.notify();
    }

    /// Finder's error sheet: what the file system said, on the window it happened in.
    pub(crate) fn complain(&self, text: &str) {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        let alert = objc2_app_kit::NSAlert::new(mtm);
        alert.setMessageText(&objc2_foundation::NSString::from_str(text));
        match &self.host {
            Some(window) => alert.beginSheetModalForWindow_completionHandler(window, None),
            None => {
                alert.runModal();
            }
        }
    }

    /// The rename lands on disk; the row comes back with the folder's next read.
    pub(crate) fn rename(&mut self, key: &SharedString, name: &str, cx: &mut Context<Self>) {
        let Some((_, old)) = split_key(key) else {
            return;
        };
        let name = name.trim();
        if name.is_empty() || name == old {
            return;
        }
        self.tags.remove(key);
        self.run_op(
            Op::Rename {
                path: PathBuf::from(key.as_ref()),
                name: name.to_owned(),
            },
            After::Select,
            cx,
        );
    }
}
