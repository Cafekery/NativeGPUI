use std::path::PathBuf;

use finder_fs::ops::{Conflict, Op};
use finder_fs::queue::{Event, JobId};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use native_gpui::tokens;
use native_gpui::{Material, material, native_button, native_progress};

use super::After;
use crate::app::FinderDemo;
use crate::chrome::path_bar::PATH_H;
use crate::data::byte_string;
use crate::palette::Palette;

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
        Op::MakeAlias(items) => verb_items("Making Aliases of", items),
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

    /// `record` is false only while the journal's own undo or redo is running.
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
                Event::Failed { job, done, error } => {
                    let Some(pos) = self.jobs.iter().position(|row| row.id == job) else {
                        continue;
                    };
                    let row = self.jobs.remove(pos);
                    if row.record && done.undo.is_some() {
                        self.journal.record(&done, &row.op);
                        landed = true;
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
                    if row.record && done.undo.is_some() {
                        self.journal.record(&done, &row.op);
                        landed = true;
                    }
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
