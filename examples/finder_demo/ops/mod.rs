pub(crate) mod clipboard;
pub(crate) mod conflicts;
pub(crate) mod jobs;
pub(crate) mod undo;

use std::path::{Path, PathBuf};

use block2::RcBlock;
use finder_fs::ops::{Conflict, Op};
use gpui::{Context, SharedString};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSAlertStyle,
    NSControlStateValueOn, NSModalResponse,
};
use objc2_foundation::NSString;

use crate::app::FinderDemo;
use crate::data::{intern_path, split_key};

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum After {
    Nothing,
    Select,
    Rename,
    Reveal,
}

pub(crate) fn touched(op: &Op) -> Vec<PathBuf> {
    let parents = |items: &[PathBuf]| {
        items
            .iter()
            .filter_map(|item| item.parent().map(PathBuf::from))
            .collect::<Vec<_>>()
    };
    match op {
        Op::Move { items, into } | Op::Copy { items, into } | Op::CopyExactly { items, into } => {
            let mut dirs = parents(items);
            dirs.push(into.clone());
            dirs
        }
        Op::Trash(items)
        | Op::DeleteImmediately(items)
        | Op::Duplicate(items)
        | Op::MakeAlias(items)
        | Op::Compress(items) => parents(items),
        Op::Quick(quick) => parents(quick.items()),
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
        let checked = cx
            .background_executor()
            .spawn(async move { conflicts::split(op, after) });
        cx.spawn(async move |this, cx| {
            let split = checked.await;
            this.update(cx, |this, cx| this.checked(split, after, cx))
                .ok();
        })
        .detach();
    }

    fn checked(&mut self, split: conflicts::Split, after: After, cx: &mut Context<Self>) {
        match split {
            conflicts::Split::Clean(op) => self.submit_op(op, Conflict::KeepBoth, after, cx),
            conflicts::Split::Locked { op, locked } => {
                if let Some(op) = self.confirm_unlock(op, &locked) {
                    self.submit_op(op, Conflict::KeepBoth, after, cx);
                }
            }
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
        if after == After::Reveal {
            cx.defer(move |cx| crate::navigate::reveal(&results, cx));
            return;
        }
        let shown = |dir: &Path| {
            let id = intern_path(dir);
            id == self.here() || id == self.target() || self.expanded.contains(id)
        };
        let visible = results.iter().any(|path| path.parent().is_some_and(shown));
        if after == After::Nothing || !visible {
            return;
        }
        self.choose(results.iter().map(PathBuf::as_path).map(key_of));
        if after == After::Rename {
            self.renaming = self.cursor.clone();
        }
        cx.notify();
    }

    fn confirm_unlock(&self, op: Op, locked: &[PathBuf]) -> Option<Op> {
        let (items, deleting) = match op {
            Op::Trash(items) => (items, false),
            Op::DeleteImmediately(items) => (items, true),
            op => return Some(op),
        };
        let mut kept = Vec::with_capacity(items.len());
        let mut for_all = None;
        for item in items {
            if !locked.contains(&item) {
                kept.push(item);
                continue;
            }
            let (yes, all) = match for_all {
                Some(yes) => (yes, true),
                None => ask_unlock(&item, deleting, locked.len() > 1),
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

    pub(crate) fn confirm(
        &self,
        text: &str,
        detail: &str,
        button: &str,
        destructive: bool,
        cx: &mut Context<Self>,
        run: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(text));
        alert.setInformativeText(&NSString::from_str(detail));
        let act = alert.addButtonWithTitle(&NSString::from_str(button));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        // Finder's delete question ignores Return: its red button takes a click.
        if destructive {
            act.setHasDestructiveAction(true);
            act.setKeyEquivalent(&NSString::from_str(""));
        }
        self.sheet(&alert, NSAlertFirstButtonReturn, cx, run);
    }

    fn sheet(
        &self,
        alert: &NSAlert,
        accept: NSModalResponse,
        cx: &mut Context<Self>,
        run: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) {
        let Some(window) = &self.host else {
            return;
        };
        let this = cx.entity().downgrade();
        let async_cx = cx.to_async();
        // `RcBlock`: AppKit holds the handler until the sheet ends, long after this returns.
        let handler = RcBlock::new(move |response: NSModalResponse| {
            if response == accept {
                let mut cx = async_cx.clone();
                this.update(&mut cx, |this, cx| run(this, cx)).ok();
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&handler));
    }

    pub(crate) fn complain(&self, text: &str) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(text));
        match &self.host {
            Some(window) => alert.beginSheetModalForWindow_completionHandler(window, None),
            None => {
                alert.runModal();
            }
        }
    }

    pub(crate) fn refuse(&self, text: String, cx: &mut Context<Self>) {
        // From a task, outside GPUI's borrow of the app, which the modal loop would re-enter.
        cx.spawn(async move |_, _| {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let alert = NSAlert::new(mtm);
            alert.setAlertStyle(NSAlertStyle::Critical);
            alert.setMessageText(&NSString::from_str(&text));
            alert.runModal();
        })
        .detach();
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
        let extension = |name: &str| {
            let (stem, extension) = name.rsplit_once('.')?;
            (!stem.is_empty() && !extension.is_empty()).then(|| extension.to_owned())
        };
        let (was, now) = (extension(old), extension(&name));
        match was {
            Some(was)
                if now.as_ref() != Some(&was)
                    && !Path::new(key.as_ref()).is_dir()
                    && crate::prefs::setting_on("FXEnableExtensionChangeWarning", true) =>
            {
                self.ask_extension(key.clone(), name, was, now, cx)
            }
            _ => self.renamed(key, name, cx),
        }
    }

    fn renamed(&mut self, key: &SharedString, name: String, cx: &mut Context<Self>) {
        if let Some((_, old)) = split_key(key) {
            self.icons.renamed(old, &name);
        }
        self.run_op(
            Op::Rename {
                path: PathBuf::from(key.as_ref()),
                name,
            },
            After::Select,
            cx,
        );
    }

    fn ask_extension(
        &self,
        key: SharedString,
        name: String,
        was: String,
        now: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let (text, change) = match &now {
            Some(now) => (
                format!(
                    "Are you sure you want to change the extension from \u{201c}.{was}\u{201d} to \u{201c}.{now}\u{201d}?"
                ),
                format!("Use .{now}"),
            ),
            None => (
                format!("Are you sure you want to remove the extension \u{201c}.{was}\u{201d}?"),
                "Remove".to_owned(),
            ),
        };
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&text));
        alert.setInformativeText(&NSString::from_str(
            "If you make this change, your document may open in a different application.",
        ));
        alert.addButtonWithTitle(&NSString::from_str(&format!("Keep .{was}")));
        alert.addButtonWithTitle(&NSString::from_str(&change));
        self.sheet(&alert, NSAlertSecondButtonReturn, cx, move |this, cx| {
            this.renamed(&key, name.clone(), cx)
        });
    }
}

/// Returns the answer and whether it stands for every other locked item.
fn ask_unlock(item: &Path, deleting: bool, offer_all: bool) -> (bool, bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return (false, false);
    };
    let name = item.file_name().unwrap_or_default().to_string_lossy();
    let (question, action) = match deleting {
        true => ("delete it", "Delete"),
        false => ("move it to the Trash", "Move to Trash"),
    };
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&format!(
        "The item \u{201c}{name}\u{201d} is locked. Are you sure you want to {question}?"
    )));
    alert.addButtonWithTitle(&NSString::from_str(action));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    if offer_all {
        alert.setShowsSuppressionButton(true);
        if let Some(button) = alert.suppressionButton() {
            button.setTitle(&NSString::from_str("Apply to All"));
        }
    }
    let answer = alert.runModal();
    let all = offer_all
        && alert
            .suppressionButton()
            .is_some_and(|button| button.state() == NSControlStateValueOn);
    (answer == NSAlertFirstButtonReturn, all)
}

fn key_of(path: &Path) -> SharedString {
    SharedString::from(path.to_string_lossy().into_owned())
}
