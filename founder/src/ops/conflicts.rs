use std::cmp::Ordering;
use std::collections::VecDeque;
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use finder_fs::ops::{Conflict, Op, locked, same_item, taken};
use gpui::{AnyElement, App, Context, Entity, FontWeight, div, prelude::*, px};
use native_gpui::tokens::{FileType, Symbol};
use native_gpui::{CheckState, ControlSize, native_button, native_checkbox, native_label};

use super::After;
use crate::app::FinderDemo;
use crate::form::text_height;

const ICON: f32 = 34.;
const ICON_X: f32 = 17.;
const BADGE: f32 = 18.;
const TOP: f32 = 2.;
const TEXT_X: f32 = 60.;
const TEXT_W: f32 = 332.;
const TEXT_SIZE: f32 = 11.;
const GAP: f32 = 12.;
const BUTTON_W: f32 = 88.;
const BUTTON_H: f32 = 22.;
const BUTTON_GAP: f32 = 6.;
const CHECKBOX_H: f32 = 16.;
const EDGE: f32 = 11.;
const BOTTOM: f32 = 10.;

#[derive(Copy, Clone, PartialEq, Eq)]
enum Choice {
    KeepBoth,
    Replace,
    Merge,
    Skip,
    Stop,
}

impl Choice {
    fn conflict(self) -> Option<Conflict> {
        match self {
            Choice::KeepBoth => Some(Conflict::KeepBoth),
            Choice::Replace => Some(Conflict::Replace),
            Choice::Merge => Some(Conflict::Merge),
            Choice::Skip => Some(Conflict::Skip),
            Choice::Stop => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Choice::KeepBoth => "Keep Both",
            Choice::Replace => "Replace",
            Choice::Merge => "Merge",
            Choice::Skip => "Skip",
            Choice::Stop => "Stop",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Move,
    Copy,
    CopyExactly,
}

impl Kind {
    fn op(self, items: Vec<PathBuf>, into: PathBuf) -> Op {
        match self {
            Kind::Move => Op::Move { items, into },
            Kind::Copy => Op::Copy { items, into },
            Kind::CopyExactly => Op::CopyExactly { items, into },
        }
    }
}

/// What the question says about the pair, read once and off the main thread.
pub(crate) struct Facts {
    folder: bool,
    /// Both are plain folders, so they could merge.
    folders: bool,
    /// The existing item's date against the moved one's.
    age: Ordering,
    /// What a merge keeps from the folder being copied.
    newer: Vec<String>,
}

impl Facts {
    fn of(current: &Path, dest: &Path, kind: Kind) -> Facts {
        let (here, there) = (
            fs::symlink_metadata(current).ok(),
            fs::symlink_metadata(dest).ok(),
        );
        let folder = |path: &Path, meta: &Option<Metadata>| {
            meta.as_ref().is_some_and(Metadata::is_dir) && !is_package(path)
        };
        let folders = folder(current, &here) && folder(dest, &there);
        let modified = |meta: &Option<Metadata>| {
            let modified = meta.as_ref()?.modified().ok()?;
            Some(modified.duration_since(UNIX_EPOCH).ok()?.as_millis())
        };
        Facts {
            folder: here.as_ref().is_some_and(Metadata::is_dir),
            folders,
            age: modified(&there).cmp(&modified(&here)),
            newer: if folders && kind != Kind::Move {
                newer_items(current, dest)
            } else {
                Vec::new()
            },
        }
    }

    fn merges(&self, kind: Kind) -> bool {
        self.folders && kind != Kind::Move
    }
}

fn newer_items(from: &Path, into: &Path) -> Vec<String> {
    let modified = |path: &Path| {
        fs::symlink_metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
    };
    let Ok(entries) = fs::read_dir(from) else {
        return Vec::new();
    };
    let mut newer: Vec<String> = entries
        .flatten()
        .filter(|entry| {
            let theirs = modified(&into.join(entry.file_name()));
            theirs.is_none() || modified(&entry.path()) > theirs
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect();
    newer.sort();
    newer
}

pub(crate) struct Sheet {
    kind: Kind,
    into: PathBuf,
    after: After,
    current: PathBuf,
    dest: PathBuf,
    remaining: VecDeque<PathBuf>,
    total: usize,
    apply_to_all: bool,
    trashless: bool,
    facts: Option<Facts>,
}

impl Sheet {
    fn dest_of(into: &Path, item: &Path) -> PathBuf {
        into.join(item.file_name().unwrap_or_default())
    }
}

fn is_package(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| finder_fs::kind::is_package_name(&name.to_string_lossy()))
}

/// A merge would mix a package's files with the old one's, so a package is replaced.
fn by_conflict(items: Vec<PathBuf>, conflict: Conflict) -> Vec<(Vec<PathBuf>, Conflict)> {
    if conflict != Conflict::Merge {
        return vec![(items, conflict)];
    }
    let (packages, others): (Vec<_>, Vec<_>) = items.into_iter().partition(|item| is_package(item));
    [(others, Conflict::Merge), (packages, Conflict::Replace)]
        .into_iter()
        .filter(|(items, _)| !items.is_empty())
        .collect()
}

pub(crate) enum Split {
    Clean(Op),
    Conflicted { clean: Option<Op>, sheet: Sheet },
    Refused(String),
    Locked { op: Op, locked: Vec<PathBuf> },
}

pub(crate) fn split(op: Op, after: After) -> Split {
    let items = match &op {
        Op::Trash(items) | Op::DeleteImmediately(items) | Op::Move { items, .. } => &items[..],
        Op::Rename { path, .. } => std::slice::from_ref(path),
        _ => &[],
    };
    let held: Vec<PathBuf> = items.iter().filter(|item| locked(item)).cloned().collect();
    if !held.is_empty() {
        return Split::Locked { op, locked: held };
    }
    unlocked(op, after)
}

pub(crate) fn unlocked(op: Op, after: After) -> Split {
    let (items, into, kind) = match op {
        Op::Move { items, into } => (items, into, Kind::Move),
        Op::Copy { items, into } => (items, into, Kind::Copy),
        Op::CopyExactly { items, into } => (items, into, Kind::CopyExactly),
        Op::Rename { path, name } => return rename_split(path, name),
        other => return Split::Clean(other),
    };
    let mut clean = Vec::new();
    let mut conflicted = VecDeque::new();
    for item in items {
        let dest = Sheet::dest_of(&into, &item);
        if taken(&dest) && !same_item(&dest, &item) {
            conflicted.push_back(item);
        } else {
            clean.push(item);
        }
    }
    if conflicted.is_empty() {
        return Split::Clean(kind.op(clean, into));
    }
    let clean_op = (!clean.is_empty()).then(|| kind.op(clean, into.clone()));
    let total = conflicted.len();
    let current = conflicted.pop_front().unwrap();
    let dest = Sheet::dest_of(&into, &current);
    let facts = Some(Facts::of(&current, &dest, kind));
    let sheet = Sheet {
        kind,
        trashless: !finder_fs::volumes::has_trash(&into),
        into,
        after,
        current,
        dest,
        remaining: conflicted,
        total,
        apply_to_all: false,
        facts,
    };
    Split::Conflicted {
        clean: clean_op,
        sheet,
    }
}

fn rename_split(path: PathBuf, name: String) -> Split {
    let dest = path.with_file_name(&name);
    if !taken(&dest) || same_item(&dest, &path) {
        return Split::Clean(Op::Rename { path, name });
    }
    Split::Refused(name_taken(&name))
}

pub(crate) fn name_taken(name: &str) -> String {
    let named = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
            format!("\u{201c}{stem}\u{201d} with extension \u{201c}.{ext}\u{201d}")
        }
        _ => format!("\u{201c}{name}\u{201d}"),
    };
    format!("The name {named} is already taken. Please choose a different name.")
}

fn question(sheet: &Sheet, facts: &Facts) -> String {
    let merges = facts.merges(sheet.kind);
    let name = sheet
        .current
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let noun = if merges { "folder" } else { "item" };
    let (named, one) = match facts.age {
        Ordering::Greater => (format!("A newer {noun}"), "the older one"),
        Ordering::Less => (format!("An older {noun}"), "the newer one"),
        Ordering::Equal if merges => ("A folder".to_owned(), "the one"),
        Ordering::Equal => ("An item".to_owned(), "the one"),
    };
    let ask = if merges {
        "merge or replace it"
    } else {
        "replace it"
    };
    let verb = if sheet.kind == Kind::Move {
        "moving"
    } else {
        "copying"
    };
    let mut text = format!(
        "{named} named \u{201c}{name}\u{201d} already exists in this location. Do you want to {ask} with {one} you\u{2019}re {verb}?"
    );
    if sheet.trashless {
        text +=
            " Replacing deletes the existing item immediately. You can\u{2019}t undo this action.";
    }
    if merges {
        text += "\n\nMerge will combine the two folders, preserving all the newer items";
        match facts.newer.as_slice() {
            [] => {}
            [first] => text += &format!(" such as \u{201c}{first}\u{201d}"),
            [first, rest @ ..] => {
                let s = if rest.len() == 1 { "" } else { "s" };
                text += &format!(
                    " such as \u{201c}{first}\u{201d} and {} other item{s}",
                    rest.len()
                );
            }
        }
        text.push('.');
    }
    text
}

/// The conflict as Finder asks it, inside the Copy window, and its height.
pub(crate) fn conflict_row(demo: &Entity<FinderDemo>, cx: &App) -> Option<(AnyElement, f32)> {
    let sheet = demo.read(cx).conflict.as_ref()?;
    let facts = sheet.facts.as_ref()?;
    let id = demo.entity_id().as_u64();
    let text = question(sheet, facts);
    let text_h = text_height(&text, TEXT_SIZE, FontWeight::NORMAL, TEXT_W);
    let buttons_y = TOP + text_h + GAP;
    let choices = if facts.merges(sheet.kind) {
        [Choice::Stop, Choice::Replace, Choice::Merge]
    } else if facts.folders {
        [Choice::Skip, Choice::Stop, Choice::Replace]
    } else {
        [Choice::KeepBoth, Choice::Stop, Choice::Replace]
    };
    let (kind, stand_in) = match (sheet.total, facts.folder) {
        (2.., _) => (FileType::Named("NSMultipleDocuments"), FileType::Folder),
        (_, true) => (
            FileType::Path(crate::data::intern_path(&sheet.current)),
            FileType::Folder,
        ),
        (_, false) => (
            FileType::Path(crate::data::intern_path(&sheet.current)),
            FileType::Uti("public.data"),
        ),
    };
    let art = |symbol: Option<Symbol>, size: f32| {
        crate::art::square(size).children(symbol.map(|symbol| symbol.img()))
    };
    let icon = crate::icons::icon(kind, ICON).or_else(|| crate::icons::icon(stand_in, ICON));
    let row = div()
        .relative()
        .flex_none()
        .h(px(buttons_y + BUTTON_H + BOTTOM))
        .child(
            div()
                .absolute()
                .left(px(ICON_X))
                .top(px(TOP))
                .child(art(icon, ICON)),
        )
        .child(
            div()
                .absolute()
                .left(px(ICON_X + ICON - BADGE))
                .top(px(TOP + ICON - BADGE + 2.))
                .child(art(
                    crate::icons::icon(FileType::Named("NSCaution"), BADGE),
                    BADGE,
                )),
        )
        .child(
            div()
                .absolute()
                .left(px(TEXT_X))
                .top(px(TOP))
                .w(px(TEXT_W))
                .h(px(text_h))
                .child(
                    native_label(("ng.finder.conflict.text", id), &text)
                        .font_size(px(TEXT_SIZE))
                        .wraps(true),
                ),
        )
        .when(sheet.total > 1, |row| {
            let demo = demo.clone();
            row.child(
                div()
                    .absolute()
                    .left(px(EDGE))
                    .top(px(buttons_y + (BUTTON_H - CHECKBOX_H) / 2.))
                    .child(
                        native_checkbox(("ng.finder.conflict.all", id), sheet.apply_to_all)
                            .title("Apply to All")
                            .control_size(ControlSize::Small)
                            .on_change(move |state: &CheckState, _, cx| {
                                let on = *state == CheckState::On;
                                demo.update(cx, |this, cx| this.set_apply_to_all(on, cx));
                            }),
                    ),
            )
        })
        .child(
            div()
                .absolute()
                .right(px(EDGE))
                .top(px(buttons_y))
                .flex()
                .gap(px(BUTTON_GAP))
                .children(choices.map(|choice| {
                    let demo = demo.clone();
                    native_button(
                        ("ng.finder.conflict", id * 8 + choice as u64),
                        choice.label(),
                    )
                    .width(px(BUTTON_W))
                    .control_size(ControlSize::Small)
                    .on_click(move |_, _, cx| {
                        demo.update(cx, |this, cx| this.resolve_conflict(choice, cx));
                    })
                })),
        );
    Some((row.into_any_element(), buttons_y + BUTTON_H + BOTTOM))
}

impl FinderDemo {
    fn read_conflict(&mut self, cx: &mut Context<Self>) {
        let Some(sheet) = self.conflict.as_ref().filter(|sheet| sheet.facts.is_none()) else {
            return;
        };
        let (current, dest, kind) = (sheet.current.clone(), sheet.dest.clone(), sheet.kind);
        let read = (current.clone(), dest.clone());
        let facts =
            (cx.background_executor()).spawn(async move { Facts::of(&read.0, &read.1, kind) });
        cx.spawn(async move |this, cx| {
            let facts = facts.await;
            this.update(cx, |this, cx| {
                let shown = (this.conflict.as_mut())
                    .filter(|sheet| sheet.current == current && sheet.dest == dest);
                if let Some(sheet) = shown {
                    sheet.facts = Some(facts);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn show_conflict(&mut self, sheet: Sheet, cx: &mut Context<Self>) {
        if self.conflict.is_some() {
            self.conflicts_waiting.push_back(sheet);
            return;
        }
        self.conflict = Some(sheet);
        self.read_conflict(cx);
        crate::progress_window::present(cx);
        cx.notify();
    }

    pub(crate) fn stop_conflict(&mut self, cx: &mut Context<Self>) {
        self.resolve_conflict(Choice::Stop, cx);
    }

    fn set_apply_to_all(&mut self, value: bool, cx: &mut Context<Self>) {
        if let Some(sheet) = &mut self.conflict {
            sheet.apply_to_all = value;
        }
        cx.notify();
    }

    fn resolve_conflict(&mut self, choice: Choice, cx: &mut Context<Self>) {
        let Some(mut sheet) = self.conflict.take() else {
            return;
        };
        if let Some(conflict) = choice.conflict() {
            let mut items = vec![std::mem::take(&mut sheet.current)];
            if sheet.apply_to_all {
                items.extend(sheet.remaining.drain(..));
            }
            for (items, conflict) in by_conflict(items, conflict) {
                let op = sheet.kind.op(items, sheet.into.clone());
                self.submit_op(op, conflict, sheet.after, cx);
            }
            if let Some(next) = sheet.remaining.pop_front() {
                sheet.dest = Sheet::dest_of(&sheet.into, &next);
                sheet.current = next;
                sheet.facts = None;
                self.conflict = Some(sheet);
            }
        }
        if self.conflict.is_none() {
            self.conflict = self.conflicts_waiting.pop_front();
        }
        self.read_conflict(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(kind: Kind, folders: bool, age: Ordering, newer: &[&str]) -> String {
        let facts = Facts {
            folder: folders,
            folders,
            age,
            newer: newer.iter().map(|name| name.to_string()).collect(),
        };
        let sheet = Sheet {
            kind,
            into: PathBuf::from("/b"),
            after: After::Nothing,
            current: PathBuf::from("/a/Folder"),
            dest: PathBuf::from("/b/Folder"),
            remaining: VecDeque::new(),
            total: 1,
            apply_to_all: false,
            trashless: false,
            facts: None,
        };
        question(&sheet, &facts)
    }

    #[test]
    fn the_question_reads_like_finders() {
        assert_eq!(
            asked(Kind::Move, false, Ordering::Greater, &[]),
            "A newer item named \u{201c}Folder\u{201d} already exists in this location. Do you want to replace it with the older one you\u{2019}re moving?"
        );
        assert_eq!(
            asked(Kind::Move, true, Ordering::Equal, &["a.txt"]),
            "An item named \u{201c}Folder\u{201d} already exists in this location. Do you want to replace it with the one you\u{2019}re moving?"
        );
        assert_eq!(
            asked(Kind::Copy, true, Ordering::Equal, &["a.txt"]),
            "A folder named \u{201c}Folder\u{201d} already exists in this location. Do you want to merge or replace it with the one you\u{2019}re copying?\n\nMerge will combine the two folders, preserving all the newer items such as \u{201c}a.txt\u{201d}."
        );
        assert!(
            asked(Kind::Copy, true, Ordering::Less, &["a", "b", "c"]).ends_with(
                "An older folder named \u{201c}Folder\u{201d} already exists in this location. Do you want to merge or replace it with the newer one you\u{2019}re copying?\n\nMerge will combine the two folders, preserving all the newer items such as \u{201c}a\u{201d} and 2 other items."
            )
        );
    }

    #[test]
    fn a_merge_never_reaches_a_package() {
        let items: Vec<PathBuf> = ["/a/Folder", "/a/Tool.app", "/a/notes.txt"]
            .map(PathBuf::from)
            .into();
        let groups = by_conflict(items.clone(), Conflict::Merge);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].1, Conflict::Merge);
        assert_eq!(groups[0].0, [items[0].clone(), items[2].clone()]);
        assert_eq!(groups[1], (vec![items[1].clone()], Conflict::Replace));
        let all = by_conflict(items, Conflict::KeepBoth);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].1, Conflict::KeepBoth);
    }
}
