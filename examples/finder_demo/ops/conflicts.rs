//! Finder's conflict sheet: Merge between folders, Apply to All when more than one name goes.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use finder_fs::ops::{Conflict, Op};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use native_gpui::tokens::{self, FileType};
use native_gpui::{CheckState, Material, material, native_button, native_checkbox};

use super::After;
use crate::app::FinderDemo;
use crate::data::byte_string;
use crate::palette::Palette;

const SHEET_W: f32 = 460.;
const ICON: f32 = 64.;
const ITEM_ICON: f32 = 32.;

/// The sheet's own choice, `Stop` never reaches `ops::Conflict`.
#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum Choice {
    KeepBoth,
    Replace,
    Merge,
    Stop,
}

impl Choice {
    fn conflict(self) -> Option<Conflict> {
        match self {
            Choice::KeepBoth => Some(Conflict::KeepBoth),
            Choice::Replace => Some(Conflict::Replace),
            Choice::Merge => Some(Conflict::Merge),
            Choice::Stop => None,
        }
    }
}

/// One item still needing a decision, and what to do once every conflict is resolved.
pub(crate) struct Sheet {
    copy: bool,
    into: PathBuf,
    after: After,
    current: PathBuf,
    dest: PathBuf,
    remaining: VecDeque<PathBuf>,
    total: usize,
    pub(crate) apply_to_all: bool,
}

impl Sheet {
    fn dest_of(into: &Path, item: &Path) -> PathBuf {
        into.join(item.file_name().unwrap_or_default())
    }

    fn build_op(&self, items: Vec<PathBuf>) -> Op {
        let into = self.into.clone();
        if self.copy {
            Op::Copy { items, into }
        } else {
            Op::Move { items, into }
        }
    }

    pub(crate) fn name(&self) -> String {
        self.current
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }

    pub(crate) fn both_folders(&self) -> bool {
        self.current.is_dir() && self.dest.is_dir()
    }

    pub(crate) fn shows_apply_to_all(&self) -> bool {
        self.total > 1
    }
}

/// `op` split into the items that land cleanly and the ones a name collides on.
pub(crate) enum Split {
    Clean(Op),
    Conflicted { clean: Option<Op>, sheet: Sheet },
}

pub(crate) fn split(op: Op, after: After) -> Split {
    let (items, into, copy) = match op {
        Op::Move { items, into } => (items, into, false),
        Op::Copy { items, into } => (items, into, true),
        other => return Split::Clean(other),
    };
    let mut clean = Vec::new();
    let mut conflicted = VecDeque::new();
    for item in items {
        if Sheet::dest_of(&into, &item).exists() {
            conflicted.push_back(item);
        } else {
            clean.push(item);
        }
    }
    if conflicted.is_empty() {
        let op = if copy {
            Op::Copy { items: clean, into }
        } else {
            Op::Move { items: clean, into }
        };
        return Split::Clean(op);
    }
    let clean_op = (!clean.is_empty()).then(|| {
        if copy {
            Op::Copy {
                items: clean,
                into: into.clone(),
            }
        } else {
            Op::Move {
                items: clean,
                into: into.clone(),
            }
        }
    });
    let total = conflicted.len();
    let current = conflicted.pop_front().unwrap();
    let dest = Sheet::dest_of(&into, &current);
    let sheet = Sheet {
        copy,
        into,
        after,
        current,
        dest,
        remaining: conflicted,
        total,
        apply_to_all: false,
    };
    Split::Conflicted {
        clean: clean_op,
        sheet,
    }
}

fn modified_seconds(path: &Path) -> f64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|since_epoch| since_epoch.as_secs_f64() - 978_307_200.)
        .unwrap_or(0.)
}

pub(super) fn item_icon(path: &Path, size: f32) -> AnyElement {
    let text = path.to_string_lossy().into_owned();
    tokens::file_icon(FileType::Path(&text), px(size))
        .map(|symbol| symbol.img().into_any_element())
        .unwrap_or_else(|| div().into_any_element())
}

/// One side of Finder's "which one do you want to keep" comparison.
fn compare_row(path: &Path, caption: &str, palette: &Palette) -> AnyElement {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let meta = std::fs::metadata(path).ok();
    let size = meta
        .as_ref()
        .filter(|meta| meta.is_file())
        .map(|meta| byte_string(meta.len()))
        .unwrap_or_else(|| "--".into());
    let modified = crate::data::day_string(modified_seconds(path));
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.))
        .child(item_icon(path, ITEM_ICON))
        .child(
            div()
                .text_size(px(11.))
                .text_color(palette.label)
                .text_ellipsis()
                .child(SharedString::from(name)),
        )
        .child(
            div()
                .text_size(px(10.))
                .text_color(palette.secondary)
                .child(SharedString::from(format!("{caption}: {size}, {modified}"))),
        )
        .into_any_element()
}

impl FinderDemo {
    /// Finder's "An item named…" sheet: Keep Both / Stop / Replace, and Merge between folders.
    pub(crate) fn conflict_sheet(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(sheet) = &self.conflict else {
            return div().into_any_element();
        };
        let name = sheet.name();
        let question = if sheet.both_folders() {
            format!(
                "An item named \u{201c}{name}\u{201d} already exists in this location. Do you want to merge it with the folder you're moving?"
            )
        } else {
            format!(
                "An item named \u{201c}{name}\u{201d} already exists in this location. Do you want to replace it with the one you're moving?"
            )
        };
        let both_folders = sheet.both_folders();
        let shows_all = sheet.shows_apply_to_all();
        let apply_to_all = sheet.apply_to_all;
        let current = sheet.current.clone();
        let dest = sheet.dest.clone();

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w(px(SHEET_W))
                    .rounded(px(12.))
                    .overflow_hidden()
                    .child(
                        material("ng.finder.conflict.glass", Material::Popover)
                            .absolute()
                            .inset_0(),
                    )
                    .child(
                        div()
                            .relative()
                            .p(px(20.))
                            .flex()
                            .flex_col()
                            .gap(px(14.))
                            .font_family(tokens::system_font_family())
                            .text_color(palette.label)
                            .child(
                                div()
                                    .flex()
                                    .gap(px(14.))
                                    .items_start()
                                    .child(item_icon(&current, ICON))
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(13.))
                                            .child(SharedString::from(question)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(16.))
                                    .pl(px(ICON + 14.))
                                    .child(compare_row(&current, "Moved item", palette))
                                    .child(compare_row(&dest, "Existing item", palette)),
                            )
                            .when(shows_all, |sheet| {
                                sheet.child(
                                    div().pl(px(ICON + 14.)).child(
                                        native_checkbox("ng.finder.conflict.all", apply_to_all)
                                            .title("Apply to All")
                                            .on_change(cx.listener(
                                                |this, state: &CheckState, _, cx| {
                                                    this.set_apply_to_all(
                                                        *state == CheckState::On,
                                                        cx,
                                                    );
                                                },
                                            )),
                                    ),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(px(8.))
                                    .child(
                                        native_button("ng.finder.conflict.stop", "Stop").on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.resolve_conflict(Choice::Stop, cx)
                                            }),
                                        ),
                                    )
                                    .child(
                                        native_button("ng.finder.conflict.keep", "Keep Both")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.resolve_conflict(Choice::KeepBoth, cx)
                                            })),
                                    )
                                    .when(both_folders, |row| {
                                        row.child(
                                            native_button("ng.finder.conflict.merge", "Merge")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.resolve_conflict(Choice::Merge, cx)
                                                })),
                                        )
                                    })
                                    .child(
                                        native_button("ng.finder.conflict.replace", "Replace")
                                            .default_button(true)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.resolve_conflict(Choice::Replace, cx)
                                            })),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn set_apply_to_all(&mut self, value: bool, cx: &mut Context<Self>) {
        if let Some(sheet) = &mut self.conflict {
            sheet.apply_to_all = value;
        }
        cx.notify();
    }

    /// Runs the sheet's choice: `Stop` drops every conflict still pending in this operation.
    pub(crate) fn resolve_conflict(&mut self, choice: Choice, cx: &mut Context<Self>) {
        let Some(mut sheet) = self.conflict.take() else {
            return;
        };
        let Some(conflict) = choice.conflict() else {
            cx.notify();
            return;
        };
        if sheet.apply_to_all {
            let mut items = vec![sheet.current.clone()];
            items.extend(sheet.remaining.drain(..));
            let op = sheet.build_op(items);
            self.submit_op(op, conflict, sheet.after, cx);
        } else {
            let op = sheet.build_op(vec![sheet.current.clone()]);
            self.submit_op(op, conflict, sheet.after, cx);
            if let Some(next) = sheet.remaining.pop_front() {
                sheet.dest = Sheet::dest_of(&sheet.into, &next);
                sheet.current = next;
                self.conflict = Some(sheet);
            }
        }
        cx.notify();
    }
}
