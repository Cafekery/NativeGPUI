use std::path::{Path, PathBuf};

use finder_fs::ops::{Conflict, Done, Op};
use finder_fs::queue::{Event, JobId};
use gpui::{AnyElement, App, ClickEvent, Context, Entity, Window, div, prelude::*, px};
use native_gpui::tokens::{self, FileType, SystemColor};
use native_gpui::{
    Blending, Layer, Material, material, native_button, native_image, native_label,
    native_progress, native_raster,
};

use super::After;
use crate::app::FinderDemo;
use crate::data::byte_string;
use crate::sound::Sound;

const CANCEL_W: f32 = 40.;
pub(crate) const ROW_H: f32 = 60.;
const ICON: f32 = 32.;
const STOP: f32 = 13.;

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
    pub(crate) sound: Option<Sound>,
    pub(crate) label: String,
    pub(crate) done_bytes: u64,
    pub(crate) total_bytes: u64,
    pub(crate) bytes_per_sec: f64,
    pub(crate) paused: bool,
}

impl JobRow {
    pub(crate) fn new(
        id: JobId,
        op: Op,
        after: After,
        record: Record,
        sound: Option<Sound>,
    ) -> Self {
        let label = describe(&op);
        Self {
            id,
            op,
            after,
            record,
            sound,
            label,
            done_bytes: 0,
            total_bytes: 0,
            bytes_per_sec: 0.,
            paused: false,
        }
    }
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn named(items: &[PathBuf]) -> String {
    match items {
        [only] => format!("\u{201c}{}\u{201d}", name_of(only)),
        _ => format!("{} items", items.len()),
    }
}

fn describe(op: &Op) -> String {
    let to = |verb: &str, items: &[PathBuf], into: &Path| {
        format!(
            "{verb} {} to \u{201c}{}\u{201d}",
            named(items),
            name_of(into)
        )
    };
    match op {
        Op::Move { items, into } => to("Moving", items, into),
        Op::Copy { items, into } | Op::CopyExactly { items, into } => to("Copying", items, into),
        Op::Trash(items) => to("Moving", items, Path::new("Trash")),
        Op::Duplicate(items) => match items.first().and_then(|first| first.parent()) {
            Some(into) => to("Duplicating", items, into),
            None => format!("Duplicating {}", named(items)),
        },
        Op::MakeAlias(items) => format!("Making Aliases of {}", named(items)),
        Op::Compress(items) => format!("Compressing {}", named(items)),
        Op::DeleteImmediately(items) => format!("Deleting {}", named(items)),
        Op::Rename { name, .. } => format!("Renaming to \u{201c}{name}\u{201d}"),
        Op::NewFolder { name, .. } | Op::NewFile { name, .. } => {
            format!("Creating \u{201c}{name}\u{201d}")
        }
        Op::PutBack(items) => {
            let back: Vec<PathBuf> = items.iter().map(|item| item.to.clone()).collect();
            format!("Putting Back {}", named(&back))
        }
        Op::Quick(quick) => format!("{} {}", quick.label(), named(quick.items())),
        Op::Batch(ops) => ops.first().map(describe).unwrap_or_default(),
    }
}

fn subject(op: &Op) -> Option<&Path> {
    match op {
        Op::Move { items, .. }
        | Op::Copy { items, .. }
        | Op::CopyExactly { items, .. }
        | Op::Trash(items)
        | Op::DeleteImmediately(items)
        | Op::Duplicate(items)
        | Op::MakeAlias(items)
        | Op::Compress(items) => items.first().map(PathBuf::as_path),
        Op::Rename { path, .. } => Some(path),
        Op::NewFolder { into, .. } | Op::NewFile { into, .. } => Some(into),
        Op::PutBack(items) => items.first().map(|item| item.item.as_path()),
        Op::Quick(quick) => quick.items().first().map(PathBuf::as_path),
        Op::Batch(ops) => ops.first().and_then(subject),
    }
}

fn caption(job: &JobRow) -> String {
    if job.total_bytes == 0 {
        return "Preparing\u{2026}".to_string();
    }
    let left = match job.bytes_per_sec {
        rate if rate > 0. => {
            remaining(job.total_bytes.saturating_sub(job.done_bytes) as f64 / rate)
        }
        _ => "Estimating time remaining\u{2026}".to_string(),
    };
    format!(
        "{} of {} \u{2013} {left}",
        byte_string(job.done_bytes),
        byte_string(job.total_bytes)
    )
}

fn remaining(secs: f64) -> String {
    let about = |unit: f64| (secs / unit).round() as u64;
    match secs {
        s if s <= 55. => format!("About {} seconds", (s / 5.).ceil().max(1.) as u64 * 5),
        s if s < 90. => "About a minute".to_string(),
        s if s < 3_300. => format!("About {} minutes", about(60.)),
        s if s < 5_400. => "About an hour".to_string(),
        s if s < 82_800. => format!("About {} hours", about(3_600.)),
        s if s < 129_600. => "About a day".to_string(),
        _ => format!("About {} days", about(86_400.)),
    }
}

impl FinderDemo {
    pub(crate) fn progress_shown(&self) -> bool {
        !self.jobs.is_empty() || self.jobs_open
    }

    pub(crate) fn jobs_popover(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<AnyElement> = self
            .jobs
            .iter()
            .map(|job| job_row(job, cx.entity()))
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
                    // All native and Above: the material covers everything GPUI paints.
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
        let sound = Sound::of(&op, false);
        self.submit(op, conflict, after, Record::New, sound, cx);
    }

    pub(crate) fn submit(
        &mut self,
        op: Op,
        conflict: Conflict,
        after: After,
        record: Record,
        sound: Option<Sound>,
        cx: &mut Context<Self>,
    ) {
        let id = self.queue.submit(op.clone(), conflict);
        self.jobs.push(JobRow::new(id, op, after, record, sound));
        cx.notify();
    }

    fn note(&mut self, row: &JobRow, done: &Done) {
        match row.record {
            Record::New => self.journal.record(done, &row.op),
            Record::Redo(at) => self.journal.redone(at, done),
            Record::Undo(_) => {}
        }
    }

    fn undo_left(&mut self, row: &JobRow, left: Op) {
        if let Record::Undo(at) = row.record {
            self.journal.undo_left(at, left);
        }
    }

    pub(crate) fn handle_queue_events(&mut self, events: Vec<Event>, cx: &mut Context<Self>) {
        if events.is_empty() {
            return;
        }
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
                    let Some(row) = self.take_job(job) else {
                        continue;
                    };
                    match result {
                        Ok(done) => {
                            if let Some(sound) = row.sound.filter(|_| !done.results.is_empty()) {
                                sound.play();
                            }
                            self.finish(&row, done, cx);
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
                    let Some(row) = self.take_job(job) else {
                        continue;
                    };
                    if let Some(left) = left {
                        self.undo_left(&row, left);
                    }
                    self.finish(&row, done, cx);
                    self.complain(&error.to_string());
                }
                Event::Cancelled { job, done } => {
                    if let Some(row) = self.take_job(job) {
                        self.finish(&row, done, cx);
                    }
                }
                Event::Paused | Event::Resumed => {
                    let paused = matches!(event, Event::Paused);
                    for row in &mut self.jobs {
                        row.paused = paused;
                    }
                }
            }
        }
        cx.notify();
    }

    fn take_job(&mut self, job: JobId) -> Option<JobRow> {
        let at = self.jobs.iter().position(|row| row.id == job)?;
        Some(self.jobs.remove(at))
    }

    fn finish(&mut self, row: &JobRow, done: Done, cx: &mut Context<Self>) {
        self.note(row, &done);
        self.settled(super::touched(&row.op), done.results, row.after, cx);
    }
}

fn fraction(job: &JobRow) -> f64 {
    match job.total_bytes {
        0 => 0.,
        total => job.done_bytes as f64 / total as f64,
    }
}

fn cancel(demo: Entity<FinderDemo>, job: JobId) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |_, _, cx| {
        demo.update(cx, |this, cx| {
            this.queue.cancel(job);
            cx.notify();
        })
    }
}

pub(crate) fn progress_row(job: &JobRow, demo: Entity<FinderDemo>) -> AnyElement {
    let id = job.id.raw();
    let small = tokens::small_system_font_size();
    let icon = subject(&job.op)
        .and_then(|path| tokens::file_icon(FileType::Path(&path.to_string_lossy()), px(ICON)));
    div()
        .relative()
        .flex_none()
        .h(px(ROW_H))
        .when_some(icon, |row, icon| {
            row.child(
                div()
                    .absolute()
                    .left(px(18.))
                    .top(px(13.))
                    .child(native_raster(("ng.finder.progress.icon", id), icon)),
            )
        })
        .child(
            div()
                .absolute()
                .left(px(60.))
                .right(px(31.))
                .top(px(8.))
                .child(native_label(("ng.finder.progress.label", id), &job.label).font_size(small)),
        )
        .child(
            div()
                .absolute()
                .left(px(62.))
                .right(px(33.))
                .top(px(19.))
                .h(px(20.))
                .flex()
                .items_center()
                .child(native_progress(
                    ("ng.finder.progress.bar", id),
                    fraction(job),
                )),
        )
        .child(
            div()
                .id(("ng.finder.progress.stop", id))
                .absolute()
                .right(px(12.))
                .top(px(22.5))
                .size(px(STOP))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    native_image(
                        ("ng.finder.progress.stop.glyph", id),
                        "NSStopProgressFreestandingTemplate",
                    )
                    .tint(SystemColor::SecondaryLabel),
                )
                .on_click(cancel(demo, job.id)),
        )
        .child(
            div()
                .absolute()
                .left(px(60.))
                .right(px(31.))
                .top(px(37.))
                .child(
                    native_label(("ng.finder.progress.caption", id), &caption(job))
                        .font_size(small)
                        .color(SystemColor::SecondaryLabel),
                ),
        )
        .into_any_element()
}

pub(crate) fn job_row(job: &JobRow, demo: Entity<FinderDemo>) -> AnyElement {
    let id = job.id.raw();
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .gap(px(8.))
                .child(div().flex_1().min_w(px(0.)).child(
                    native_label(("ng.finder.jobs.label", id), &job.label).font_size(px(11.)),
                ))
                .child(
                    div()
                        .id(("ng.finder.jobs.cancel", id))
                        .flex_none()
                        .w(px(CANCEL_W))
                        .child(
                            native_label(("ng.finder.jobs.cancel.label", id), "Cancel")
                                .font_size(px(11.))
                                .color(SystemColor::SecondaryLabel),
                        )
                        .on_click(cancel(demo, job.id)),
                ),
        )
        .child(native_progress(("ng.finder.jobs.bar", id), fraction(job)))
        .child(
            native_label(("ng.finder.jobs.caption", id), &caption(job))
                .font_size(px(10.))
                .color(SystemColor::SecondaryLabel),
        )
        .into_any_element()
}
