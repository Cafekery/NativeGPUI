pub(crate) mod clipboard;
pub(crate) mod conflicts;
pub(crate) mod jobs;
pub(crate) mod undo;

use std::path::{Path, PathBuf};

use finder_fs::ops::{Conflict, Op, locked};
use gpui::{Context, SharedString};

use crate::app::FinderDemo;
use crate::data::{intern_path, split_key};

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum After {
    Nothing,
    Select,
    Rename,
}

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
        Op::Trash(items)
        | Op::DeleteImmediately(items)
        | Op::Duplicate(items)
        | Op::MakeAlias(items)
        | Op::Compress(items) => parents(items),
        Op::Rename { path, .. } => parents(std::slice::from_ref(path)),
        Op::NewFolder { into, .. } | Op::NewFile { into, .. } => vec![into.clone()],
        Op::PutBack(items) => items
            .iter()
            .flat_map(|restore| [&restore.item, &restore.to])
            .filter_map(|path| path.parent().map(PathBuf::from))
            .collect(),
        Op::Batch(ops) => ops.iter().flat_map(touched).collect(),
    }
}

impl FinderDemo {
    pub(crate) fn run_op(&mut self, op: Op, after: After, cx: &mut Context<Self>) {
        let Some(op) = self.confirm_unlock(op) else {
            return;
        };
        match conflicts::split(op, after) {
            conflicts::Split::Clean(op) => self.submit_op(op, Conflict::KeepBoth, after, cx),
            conflicts::Split::Conflicted { clean, sheet } => {
                if let Some(clean) = clean {
                    self.submit_op(clean, Conflict::KeepBoth, After::Nothing, cx);
                }
                self.conflict = Some(sheet);
                cx.notify();
            }
            conflicts::Split::Refused(message) => self.complain(&message),
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
        let shown = |dir: &Path| {
            let id = intern_path(dir);
            id == self.here() || id == self.target() || self.expanded.contains(id)
        };
        let visible = results.iter().any(|path| path.parent().is_some_and(shown));
        if after == After::Nothing || !visible {
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

    /// Finder asks before it clears a lock: one question per locked item, with Apply to All once
    /// more than one of them is locked.
    fn confirm_unlock(&self, op: Op) -> Option<Op> {
        let (items, deleting) = match &op {
            Op::Trash(items) => (items.clone(), false),
            Op::DeleteImmediately(items) => (items.clone(), true),
            _ => return Some(op),
        };
        let total = items.iter().filter(|item| locked(item)).count();
        if total == 0 {
            return Some(op);
        }
        let mut kept = Vec::with_capacity(items.len());
        let mut for_all = None;
        for item in items {
            if !locked(&item) {
                kept.push(item);
                continue;
            }
            let (yes, all) = match for_all {
                Some(yes) => (yes, true),
                None => self.ask_unlock(&item, deleting, total > 1),
            };
            if all {
                for_all = Some(yes);
            }
            if yes {
                kept.push(item);
            }
        }
        if kept.is_empty() {
            return None;
        }
        Some(match deleting {
            true => Op::DeleteImmediately(kept),
            false => Op::Trash(kept),
        })
    }

    /// Finder's locked-item question, and whether the answer stands for every other locked item.
    fn ask_unlock(&self, item: &Path, deleting: bool, offer_all: bool) -> (bool, bool) {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return (false, false);
        };
        let name = item.file_name().unwrap_or_default().to_string_lossy();
        let (question, action) = match deleting {
            true => ("delete it", "Delete"),
            false => ("move it to the Trash", "Move to Trash"),
        };
        let alert = objc2_app_kit::NSAlert::new(mtm);
        alert.setMessageText(&objc2_foundation::NSString::from_str(&format!(
            "The item \u{201c}{name}\u{201d} is locked. Are you sure you want to {question}?"
        )));
        alert.addButtonWithTitle(&objc2_foundation::NSString::from_str(action));
        alert.addButtonWithTitle(&objc2_foundation::NSString::from_str("Cancel"));
        if offer_all {
            alert.setShowsSuppressionButton(true);
            if let Some(button) = alert.suppressionButton() {
                button.setTitle(&objc2_foundation::NSString::from_str("Apply to All"));
            }
        }
        let answer = alert.runModal();
        let all = offer_all
            && alert
                .suppressionButton()
                .is_some_and(|button| button.state() == objc2_app_kit::NSControlStateValueOn);
        (answer == objc2_app_kit::NSAlertFirstButtonReturn, all)
    }

    pub(crate) fn confirm(
        &self,
        text: &str,
        detail: &str,
        button: &str,
        destructive: bool,
        cx: &mut Context<Self>,
        run: impl Fn(&gpui::AsyncApp) + 'static,
    ) {
        let (Some(mtm), Some(window)) = (objc2::MainThreadMarker::new(), self.host.clone()) else {
            return;
        };
        let alert = objc2_app_kit::NSAlert::new(mtm);
        alert.setMessageText(&objc2_foundation::NSString::from_str(text));
        alert.setInformativeText(&objc2_foundation::NSString::from_str(detail));
        let act = alert.addButtonWithTitle(&objc2_foundation::NSString::from_str(button));
        alert.addButtonWithTitle(&objc2_foundation::NSString::from_str("Cancel"));
        // Finder's delete question ignores Return: its red button takes a click.
        if destructive {
            act.setHasDestructiveAction(true);
            act.setKeyEquivalent(&objc2_foundation::NSString::from_str(""));
        }
        let async_cx = cx.to_async();
        // `RcBlock`: AppKit holds the handler until the sheet ends, long after this returns.
        let handler = block2::RcBlock::new(move |response: objc2_app_kit::NSModalResponse| {
            if response == objc2_app_kit::NSAlertFirstButtonReturn {
                run(&async_cx);
            }
        });
        alert.beginSheetModalForWindow_completionHandler(&window, Some(&handler));
    }

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

    /// A typed `/` is stored as `:`, the byte Finder draws as a slash.
    pub(crate) fn rename(&mut self, key: &SharedString, name: &str, cx: &mut Context<Self>) {
        let Some((_, old)) = split_key(key) else {
            return;
        };
        let name = name.trim().replace('/', ":");
        if name.is_empty() || name == old {
            return;
        }
        self.tags.remove(key);
        self.run_op(
            Op::Rename {
                path: PathBuf::from(key.as_ref()),
                name,
            },
            After::Select,
            cx,
        );
    }
}
