use std::path::PathBuf;

use finder_fs::ops::{Conflict, Done, Op};
use finder_fs::queue::{Event, JobId};
use gpui::{AnyElement, App, Context, div, prelude::*, px};
use native_gpui::tokens::SystemColor;
use native_gpui::{
    Blending, Layer, Material, material, native_button, native_label, native_progress,
};

use super::After;
use crate::app::FinderDemo;
use crate::data::byte_string;

const CANCEL_W: f32 = 40.;

/// A new operation goes in the journal, a redo updates the entry it came from, and an undo is
/// already accounted for.
#[derive(Clone, Copy)]
pub(crate) enum Record {
    New,
    Undo(usize),
    Redo(usize),
}

pub(crate) struct JobRow {
    pub(crate) id: JobId,
    pub(crate) op: Op,
    pub(crate) after: After,
    pub(crate) record: Record,
    pub(crate) label: String,
    pub(crate) done_bytes: u64,
    pub(crate) total_bytes: u64,
    pub(crate) bytes_per_sec: f64,
    pub(crate) paused: bool,
}

impl JobRow {
    pub(crate) fn new(id: JobId, op: Op, after: After, record: Record) -> Self {
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
        Op::MakeAlias(items) => verb_items("Making Aliases of", items),
        Op::Compress(items) => verb_items("Compressing", items),
        Op::Trash(items) => verb_items("Moving", items),
        Op::DeleteImmediately(items) => verb_items("Deleting", items),
        Op::Rename { name, .. } => format!("Renaming to \"{name}\""),
        Op::NewFolder { name, .. } => format!("Creating \"{name}\""),
        Op::NewFile { name, .. } => format!("Creating \"{name}\""),
        Op::PutBack(items) => verb_items(
            "Putting Back",
            &items
                .iter()
                .map(|restore| restore.to.clone())
                .collect::<Vec<_>>(),
        ),
        Op::Batch(ops) => ops.first().map(describe).unwrap_or_default(),
    }
}

impl FinderDemo {
    /// Finder's toolbar has no progress item; ours stands there only while it has something to report.
    pub(crate) fn progress_shown(&self) -> bool {
        !self.jobs.is_empty() || self.jobs_open
    }

    pub(crate) fn jobs_popover(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<AnyElement> = self
            .jobs
            .iter()
            .map(|job| {
                let (demo, id) = (cx.entity(), job.id);
                job_row(job, move |cx| {
                    demo.update(cx, |this, cx| {
                        this.queue.cancel(id);
                        cx.notify();
                    })
                })
            })
            .collect();
        let paused = self.jobs.iter().any(|job| job.paused);
        div()
            .absolute()
            .top(self.metrics().top + px(6.))
            .right(px(12.))
            .child(
                div()
                    .relative()
                    .w(px(320.))
                    .rounded(px(10.))
                    .overflow_hidden()
                    .block_mouse_except_scroll()
                    // Native and Above, all of it: the band's material covers everything GPUI paints.
                    .child(
                        material("ng.finder.jobs.glass", Material::Popover)
                            .layer(Layer::Above)
                            .blending(Blending::WithinWindow)
                            .corner_radius(px(10.))
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
                            .when(rows.is_empty(), |panel| {
                                panel.child(
                                    native_label("ng.finder.jobs.none", "No operations in progress")
                                        .font_size(px(12.))
                                        .color(SystemColor::SecondaryLabel),
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

    pub(crate) fn submit_op(
        &mut self,
        op: Op,
        conflict: Conflict,
        after: After,
        cx: &mut Context<Self>,
    ) {
        self.submit(op, conflict, after, Record::New, cx);
    }

    pub(crate) fn submit(
        &mut self,
        op: Op,
        conflict: Conflict,
        after: After,
        record: Record,
        cx: &mut Context<Self>,
    ) {
        let id = self.queue.submit(op.clone(), conflict);
        self.jobs.push(JobRow::new(id, op, after, record));
        cx.notify();
    }

    fn note(&mut self, row: &JobRow, done: &Done) -> bool {
        match row.record {
            Record::New => self.journal.record(done, &row.op),
            Record::Redo(at) => self.journal.redone(at, done),
            Record::Undo(_) => return false,
        }
        true
    }

    fn undo_left(&mut self, row: &JobRow, left: Op) {
        if let Record::Undo(at) = row.record {
            self.journal.undo_left(at, left);
            super::undo::refresh_edit_menu(&self.journal);
        }
    }

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
                            landed |= self.note(&row, &done);
                            let touched = crate::ops::touched(&row.op);
                            self.settled(touched, done.results, row.after, cx);
                        }
                        Err(err) => {
                            self.undo_left(&row, row.op.clone());
                            self.complain(&err.to_string());
                        }
                    }
                }
                Event::Failed {
                    job,
                    done,
                    error,
                    left,
                } => {
                    let Some(pos) = self.jobs.iter().position(|row| row.id == job) else {
                        continue;
                    };
                    let row = self.jobs.remove(pos);
                    landed |= self.note(&row, &done);
                    if let Some(left) = left {
                        self.undo_left(&row, left);
                    }
                    let touched = crate::ops::touched(&row.op);
                    self.settled(touched, done.results, row.after, cx);
                    self.complain(&error.to_string());
                }
                Event::Cancelled { job, done } => {
                    let Some(pos) = self.jobs.iter().position(|row| row.id == job) else {
                        continue;
                    };
                    let row = self.jobs.remove(pos);
                    landed |= self.note(&row, &done);
                    let touched = crate::ops::touched(&row.op);
                    self.settled(touched, done.results, row.after, cx);
                }
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
            super::undo::refresh_edit_menu(&self.journal);
        }
        cx.notify();
    }
}

/// One operation's row: what it is doing, how far it has got, and a Cancel that `on_cancel` carries
/// to whichever window owns it.
pub(crate) fn job_row(job: &JobRow, on_cancel: impl Fn(&mut App) + 'static) -> AnyElement {
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
                .gap(px(8.))
                .child(div().flex_1().min_w(px(0.)).child(
                    native_label(("ng.finder.jobs.label", id.raw()), &job.label).font_size(px(11.)),
                ))
                .child(
                    div()
                        .id(("ng.finder.jobs.cancel", id.raw()))
                        .flex_none()
                        .w(px(CANCEL_W))
                        .child(
                            native_label(("ng.finder.jobs.cancel.label", id.raw()), "Cancel")
                                .font_size(px(11.))
                                .color(SystemColor::SecondaryLabel),
                        )
                        .on_click(move |_, _, cx| on_cancel(cx)),
                ),
        )
        .child(native_progress(("ng.finder.jobs.bar", id.raw()), fraction))
        .child(
            native_label(("ng.finder.jobs.caption", id.raw()), &caption)
                .font_size(px(10.))
                .color(SystemColor::SecondaryLabel),
        )
        .into_any_element()
}
