//! The conflict sheet (Merge always offered, Apply to All), Finder-worded Undo/Redo off
//! `queue::Journal`, and the toolbar's queued-operations progress popover.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use finder_fs::ops::{Conflict, Op};
use finder_fs::queue::{Event, JobId, Journal};
use gpui::{
    Action, AnyElement, App, Context, Div, InteractiveElement, ParentElement, SharedString, Styled,
    actions, div, prelude::*, px,
};
use native_gpui::tokens::{self, FileType};
use native_gpui::{
    CheckState, Material, material, native_button, native_checkbox, native_progress,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};

use crate::app::FinderDemo;
use crate::data::byte_string;
use crate::ops::After;
use crate::palette::Palette;
use crate::path_bar::PATH_H;

const SHEET_W: f32 = 460.;
const ICON: f32 = 64.;
const ITEM_ICON: f32 = 32.;

actions!(finder_demo, [UndoOp, RedoOp]);

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

/// One item still needing a decision, plus what to do once every conflict in this operation is
/// resolved.
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

/// Splits `op` into the items that land cleanly and the ones a name collides on; `None` means
/// nothing collided and the whole op can run as it is.
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

/// What a completed job in the queue does when it lands: record it in the undo journal, or not,
/// when the job it finished was itself running the journal's own undo or redo.
pub(crate) struct JobRow {
    pub(crate) id: JobId,
    pub(crate) op: Op,
    pub(crate) after: After,
    pub(crate) record: bool,
    pub(crate) label: String,
    pub(crate) done_bytes: u64,
    pub(crate) total_bytes: u64,
    pub(crate) bytes_per_sec: f64,
    pub(crate) paused: bool,
}

impl JobRow {
    pub(crate) fn new(id: JobId, op: Op, after: After, record: bool) -> Self {
        let label = describe(&op);
        Self {
            id,
            op,
            after,
            record,
            label,
            done_bytes: 0,
            total_bytes: 0,
            bytes_per_sec: 0.,
            paused: false,
        }
    }
}

fn describe(op: &Op) -> String {
    let verb_items = |verb: &str, items: &[PathBuf]| match items {
        [only] => format!(
            "{verb} \"{}\"",
            only.file_name().unwrap_or_default().to_string_lossy()
        ),
        _ => format!("{verb} {} Items", items.len()),
    };
    match op {
        Op::Move { items, .. } => verb_items("Moving", items),
        Op::Copy { items, .. } => verb_items("Copying", items),
        Op::Duplicate(items) => verb_items("Duplicating", items),
        Op::Trash(items) => verb_items("Moving", items),
        Op::DeleteImmediately(items) => verb_items("Deleting", items),
        Op::Rename { name, .. } => format!("Renaming to \"{name}\""),
        Op::NewFolder { name, .. } => format!("Creating \"{name}\""),
        Op::NewFile { name, .. } => format!("Creating \"{name}\""),
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

fn item_icon(path: &Path, size: f32) -> AnyElement {
    let text = path.to_string_lossy().into_owned();
    tokens::file_icon(FileType::Path(&text), px(size))
        .map(|symbol| symbol.img().into_any_element())
        .unwrap_or_else(|| div().into_any_element())
}

/// One side of the "which one do you want to keep" comparison Finder shows below the question.
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
    /// Finder's "An item named…" sheet: Keep Both / Stop / Replace, Merge whenever both sides
    /// are folders, and Apply to All once more than one conflict is pending.
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

    /// Runs the sheet's choice: `Stop` drops every conflict still pending in this operation,
    /// everything else submits the current item (or, with Apply to All, every item still
    /// pending) with that `ops::Conflict` and shows the next conflict if one is left.
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

    /// The toolbar's progress popover: every queued job with a determinate bar, throughput and
    /// pause/resume/cancel.
    pub(crate) fn jobs_popover(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<AnyElement> = self
            .jobs
            .iter()
            .map(|job| self.job_row(job, palette, cx))
            .collect();
        let paused = self.jobs.iter().any(|job| job.paused);
        div()
            .absolute()
            .top(px(PATH_H + 6.))
            .right(px(12.))
            .child(
                div()
                    .relative()
                    .w(px(320.))
                    .rounded(px(10.))
                    .overflow_hidden()
                    .child(
                        material("ng.finder.jobs.glass", Material::Popover)
                            .absolute()
                            .inset_0(),
                    )
                    .child(
                        div()
                            .relative()
                            .p(px(12.))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .font_family(tokens::system_font_family())
                            .text_color(palette.label)
                            .when(rows.is_empty(), |panel| {
                                panel.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(palette.secondary)
                                        .child("No operations in progress"),
                                )
                            })
                            .children(rows)
                            .when(!self.jobs.is_empty(), |panel| {
                                panel.child(
                                    div().flex().justify_end().child(
                                        native_button(
                                            "ng.finder.jobs.pause",
                                            if paused { "Resume" } else { "Pause" },
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                if paused {
                                                    this.queue.resume();
                                                } else {
                                                    this.queue.pause();
                                                }
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn job_row(&self, job: &JobRow, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let fraction = if job.total_bytes > 0 {
            job.done_bytes as f64 / job.total_bytes as f64
        } else {
            0.
        };
        let caption = if job.total_bytes > 0 {
            format!(
                "{} of {} \u{2014} {}/s",
                byte_string(job.done_bytes),
                byte_string(job.total_bytes),
                byte_string(job.bytes_per_sec as u64)
            )
        } else {
            "Preparing\u{2026}".to_string()
        };
        let id = job.id;
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_size(px(11.))
                    .child(SharedString::from(job.label.clone()))
                    .child(
                        div()
                            .id(("ng.finder.jobs.cancel", id.raw()))
                            .text_color(palette.secondary)
                            .child("Cancel")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.queue.cancel(id);
                                cx.notify();
                            })),
                    ),
            )
            .child(native_progress(("ng.finder.jobs.bar", id.raw()), fraction))
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(palette.secondary)
                    .child(SharedString::from(caption)),
            )
            .into_any_element()
    }

    /// Submits `op` to the operation queue and tracks it for the progress popover; `record`
    /// controls whether landing it adds a fresh entry to the undo journal, which is false only
    /// while the journal's own undo/redo is what is running.
    pub(crate) fn submit_op(
        &mut self,
        op: Op,
        conflict: Conflict,
        after: After,
        cx: &mut Context<Self>,
    ) {
        self.submit(op, conflict, after, true, cx);
    }

    pub(crate) fn submit(
        &mut self,
        op: Op,
        conflict: Conflict,
        after: After,
        record: bool,
        cx: &mut Context<Self>,
    ) {
        let id = self.queue.submit(op.clone(), conflict);
        self.jobs.push(JobRow::new(id, op, after, record));
        cx.notify();
    }

    pub(crate) fn perform_undo(&mut self, cx: &mut Context<Self>) {
        let Some(undoable) = self.journal.pop_undo() else {
            return;
        };
        refresh_edit_menu(&self.journal);
        self.submit(undoable.undo, Conflict::Replace, After::Select, false, cx);
    }

    pub(crate) fn perform_redo(&mut self, cx: &mut Context<Self>) {
        let Some(undoable) = self.journal.pop_redo() else {
            return;
        };
        refresh_edit_menu(&self.journal);
        self.submit(undoable.redo, Conflict::Replace, After::Select, false, cx);
    }

    /// Wires ⌘Z / ⇧⌘Z's actions (fired by the Edit menu's retargeted rows) into this window.
    pub(crate) fn wire_undo_redo(&self, element: Div, cx: &mut Context<Self>) -> Div {
        element
            .on_action(cx.listener(|this, _: &UndoOp, _, cx| this.perform_undo(cx)))
            .on_action(cx.listener(|this, _: &RedoOp, _, cx| this.perform_redo(cx)))
    }

    /// Drains the operation queue's events: progress updates the popover, a finished job settles
    /// the view and records it in the undo journal (unless it was the journal's own undo/redo).
    pub(crate) fn handle_queue_events(&mut self, events: Vec<Event>, cx: &mut Context<Self>) {
        if events.is_empty() {
            return;
        }
        let mut landed = false;
        for event in events {
            match event {
                Event::Started(_) => {}
                Event::Progress {
                    job,
                    done_bytes,
                    total_bytes,
                    bytes_per_sec,
                    ..
                } => {
                    if let Some(row) = self.jobs.iter_mut().find(|row| row.id == job) {
                        row.done_bytes = done_bytes;
                        row.total_bytes = total_bytes;
                        row.bytes_per_sec = bytes_per_sec;
                    }
                }
                Event::Finished(job, result) => {
                    let Some(pos) = self.jobs.iter().position(|row| row.id == job) else {
                        continue;
                    };
                    let row = self.jobs.remove(pos);
                    match result {
                        Ok(done) => {
                            if row.record {
                                self.journal.record(&done, &row.op);
                                landed = true;
                            }
                            let touched = crate::ops::touched(&row.op);
                            self.settled(touched, done.results, row.after, cx);
                        }
                        Err(err) => self.complain(&err.to_string()),
                    }
                }
                Event::Cancelled(job) => self.jobs.retain(|row| row.id != job),
                Event::Paused => {
                    for row in &mut self.jobs {
                        row.paused = true;
                    }
                }
                Event::Resumed => {
                    for row in &mut self.jobs {
                        row.paused = false;
                    }
                }
            }
        }
        if landed {
            refresh_edit_menu(&self.journal);
        }
        cx.notify();
    }
}

fn find_submenu(bar: &NSMenu, title: &str) -> Option<Retained<NSMenu>> {
    bar.itemArray()
        .iter()
        .find(|item| item.title().to_string() == title)
        .and_then(|item| item.submenu())
}

fn find_item(menu: &NSMenu, title: &str) -> Option<Retained<NSMenuItem>> {
    menu.itemArray()
        .iter()
        .find(|item| item.title().to_string() == title)
}

struct SinkIvars {
    cx: gpui::AsyncApp,
}

define_class!(
    /// Retargets the Edit menu's stock Undo/Redo rows, which nothing in this app's responder
    /// chain otherwise answers.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGUndoRedoSink"]
    #[ivars = SinkIvars]
    struct UndoRedoSink;

    impl UndoRedoSink {
        #[unsafe(method(ngUndo:))]
        fn ng_undo(&self, _sender: *mut AnyObject) {
            self.dispatch(UndoOp);
        }

        #[unsafe(method(ngRedo:))]
        fn ng_redo(&self, _sender: *mut AnyObject) {
            self.dispatch(RedoOp);
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate(&self, item: *mut NSMenuItem) -> bool {
            let action = unsafe { item.as_ref() }.and_then(|item| item.action());
            STATE.with(|state| {
                let (can_undo, can_redo) = state.get();
                if action == Some(sel!(ngUndo:)) {
                    can_undo
                } else if action == Some(sel!(ngRedo:)) {
                    can_redo
                } else {
                    true
                }
            })
        }
    }
);

impl UndoRedoSink {
    fn dispatch(&self, action: impl Action) {
        let boxed: Box<dyn Action> = Box::new(action);
        self.ivars()
            .cx
            .clone()
            .spawn(async move |cx| {
                cx.update(|cx| cx.dispatch_action(&*boxed));
            })
            .detach();
    }
}

thread_local! {
    static STATE: Cell<(bool, bool)> = const { Cell::new((false, false)) };
    static ITEMS: RefCell<Option<(Retained<NSMenuItem>, Retained<NSMenuItem>)>> =
        const { RefCell::new(None) };
}

/// Retargets the Edit menu's Undo/Redo rows once, right after `install_menus`. A second call is
/// a no-op: the rows no longer carry their original titles to search for.
pub(crate) fn install_undo_redo(cx: &mut App) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(bar) = NSApplication::sharedApplication(mtm).mainMenu() else {
        return;
    };
    let Some(edit) = find_submenu(&bar, "Edit") else {
        return;
    };
    let (Some(undo_item), Some(redo_item)) = (find_item(&edit, "Undo"), find_item(&edit, "Redo"))
    else {
        return;
    };
    let sink = UndoRedoSink::alloc(mtm).set_ivars(SinkIvars { cx: cx.to_async() });
    let sink: Retained<UndoRedoSink> = unsafe { msg_send![super(sink), init] };
    unsafe {
        undo_item.setTarget(Some(&sink));
        undo_item.setAction(Some(sel!(ngUndo:)));
        redo_item.setTarget(Some(&sink));
        redo_item.setAction(Some(sel!(ngRedo:)));
    }
    SINK.with(|shared| *shared.borrow_mut() = Some(sink));
    ITEMS.with(|shared| *shared.borrow_mut() = Some((undo_item, redo_item)));
}

thread_local! {
    static SINK: RefCell<Option<Retained<UndoRedoSink>>> = const { RefCell::new(None) };
}

/// Sets the Edit menu's Undo/Redo titles to Finder's own wording — "Undo Move of \"X\"" — and
/// their enabled state, off the journal's current stacks.
pub(crate) fn refresh_edit_menu(journal: &Journal) {
    ITEMS.with(|shared| {
        let items = shared.borrow();
        let Some((undo_item, redo_item)) = items.as_ref() else {
            return;
        };
        let (undo_title, can_undo) = match journal.undo_label() {
            Some(label) => (format!("Undo {label}"), true),
            None => ("Undo".to_string(), false),
        };
        let (redo_title, can_redo) = match journal.redo_label() {
            Some(label) => (format!("Redo {label}"), true),
            None => ("Redo".to_string(), false),
        };
        undo_item.setTitle(&NSString::from_str(&undo_title));
        redo_item.setTitle(&NSString::from_str(&redo_title));
        STATE.with(|state| state.set((can_undo, can_redo)));
    });
}
