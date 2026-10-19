pub(crate) mod clipboard;
pub(crate) mod conflicts;
pub(crate) mod jobs;
pub(crate) mod put_back;
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

/// A server id's parent is not where `Path::parent` puts it at the server root.
fn parent(path: &Path) -> Option<&Path> {
    match path.to_str() {
        Some(id) if crate::remote::is_remote(id) => Some(Path::new(crate::remote::parent(id))),
        _ => path.parent(),
    }
}

fn parents<P: AsRef<Path>>(paths: impl IntoIterator<Item = P>) -> Vec<PathBuf> {
    (paths.into_iter())
        .filter_map(|path| parent(path.as_ref()).map(PathBuf::from))
        .collect()
}

pub(crate) fn touched(op: &Op) -> Vec<PathBuf> {
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
        Op::Rename { path, .. } => parents([path]),
        Op::NewFolder { into, .. } | Op::NewFile { into, .. } => vec![into.clone()],
        Op::PutBack(items) => parents(
            items
                .iter()
                .flat_map(|restore| [&restore.item, &restore.to]),
        ),
        Op::Retry(request) => parents(request.items().flat_map(|(item, dest)| [item, dest])),
        Op::Batch(ops) => ops.iter().flat_map(touched).collect(),
    }
}

impl FinderDemo {
    pub(crate) fn run_op(&mut self, op: Op, after: After, cx: &mut Context<Self>) {
        if touched(&op)
            .iter()
            .any(|dir| crate::remote::is_remote(&dir.to_string_lossy()))
        {
            self.submit_remote(op, Vec::new(), None, after, cx);
            return;
        }
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
                    self.checked(conflicts::unlocked(op, after), after, cx);
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
        let landed = results.iter().filter_map(|path| parent(path));
        for dir in touched.iter().map(PathBuf::as_path).chain(landed) {
            self.read_folder(intern_path(dir), cx);
        }
        if after == After::Reveal {
            cx.defer(move |cx| crate::navigate::reveal(&results, cx));
            return;
        }
        let shown = |dir: &Path| {
            let id = intern_path(dir);
            id == self.here() || id == self.target() || self.expanded.contains(id)
        };
        let visible = results.iter().any(|path| parent(path).is_some_and(shown));
        if after == After::Nothing || !visible {
            return;
        }
        self.choose(
            (results.iter()).map(|path| SharedString::from(path.to_string_lossy().into_owned())),
        );
        if after == After::Rename {
            self.renaming = self.cursor.clone();
        }
        cx.notify();
    }

    fn confirm_unlock(&self, op: Op, locked: &[PathBuf]) -> Option<Op> {
        let (items, question, action) = match &op {
            Op::Trash(items) => (items.clone(), "move it to the Trash", "Move to Trash"),
            Op::DeleteImmediately(items) => (items.clone(), "delete it", "Delete"),
            Op::Move { items, .. } => (items.clone(), "move it", "Move"),
            Op::Rename { path, .. } => (vec![path.clone()], "rename it", "Rename"),
            _ => return Some(op),
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
                None => ask_unlock(&item, question, action, locked.len() > 1),
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
        Some(match op {
            Op::Trash(_) => Op::Trash(kept),
            Op::DeleteImmediately(_) => Op::DeleteImmediately(kept),
            Op::Move { into, .. } => Op::Move { items: kept, into },
            op => op,
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

    pub(crate) fn sheet(
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
        let Some((dir, old)) = split_key(key) else {
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
        let folder = || {
            if crate::remote::is_remote(key) {
                self.remote_row(dir, key).is_some_and(|row| row.is_folder())
            } else {
                Path::new(key.as_ref()).is_dir()
            }
        };
        match was {
            Some(was)
                if now.as_ref() != Some(&was)
                    && !folder()
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

fn ask_unlock(item: &Path, question: &str, action: &str, offer_all: bool) -> (bool, bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return (false, false);
    };
    let name = item.file_name().unwrap_or_default().to_string_lossy();
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
