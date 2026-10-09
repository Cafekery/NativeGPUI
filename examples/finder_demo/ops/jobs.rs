use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use finder_fs::ops::transfer::{End, Failure, Kind, Phase, Progress};
use finder_fs::ops::{Carried, Conflict, Done, Op};
use finder_fs::queue::{Event, JobId, Queue};
use gpui::{AnyElement, App, ClickEvent, Context, Entity, FontWeight, Window, div, prelude::*, px};
use native_gpui::tokens::{self, FileType, SystemColor};
use native_gpui::{
    ControlSize, native_button, native_image, native_label, native_progress, native_raster,
    native_symbol,
};

use super::After;
use crate::app::FinderDemo;
use crate::data::{byte_string, count};
use crate::sound::Sound;

/// A job that ends sooner never shows its progress.
const SHOW_AFTER: Duration = Duration::from_millis(500);
pub(crate) const ROW_H: f32 = 75.;
const ICON: f32 = 32.;
const GLYPH: f32 = 11.;
const GLYPH_GAP: f32 = 9.;
const EDGE: f32 = 13.;
const TEXT_X: f32 = 62.;
const DASH: &str = " \u{2013} ";

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
    began: Instant,
    progress: Option<Progress>,
    failed: Option<Failed>,
}

/// What an ended job could not carry, kept until it is retried or dismissed.
struct Failed {
    failures: Vec<Failure>,
    retry: Option<Op>,
}

impl Failed {
    /// A full or read-only volume turns down every item still left, not one file.
    fn refusal(&self) -> Option<&Failure> {
        self.failures.iter().find(|failure| {
            matches!(
                failure.error.kind(),
                ErrorKind::StorageFull | ErrorKind::QuotaExceeded | ErrorKind::ReadOnlyFilesystem
            )
        })
    }

    fn reason(&self) -> Option<String> {
        let first = self.refusal().or(self.failures.first());
        first.map(|failure| failure.error.to_string())
    }
}

impl JobRow {
    fn new(id: JobId, op: Op, after: After, record: Record, sound: Option<Sound>) -> Self {
        Self {
            id,
            op,
            after,
            record,
            sound,
            began: Instant::now(),
            progress: None,
            failed: None,
        }
    }

    /// Once it has run a moment, and for as long as its failures wait for an answer.
    pub(crate) fn shown(&self) -> bool {
        self.failed.is_some() || self.began.elapsed() >= SHOW_AFTER
    }

    pub(crate) fn running(&self) -> bool {
        self.failed.is_none()
    }

    fn carries_bytes(&self) -> bool {
        matches!(
            self.op,
            Op::Move { .. }
                | Op::Copy { .. }
                | Op::CopyExactly { .. }
                | Op::Duplicate(_)
                | Op::PutBack(_)
                | Op::Retry(_)
        )
    }
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// Finder counts what is left to carry, so the number falls as items land.
fn named(items: &[PathBuf], left: Option<u64>) -> String {
    match (items, left) {
        (_, Some(left)) if left > 1 => format!("{} items", count(left as usize)),
        ([only], _) => format!("\u{201c}{}\u{201d}", name_of(only)),
        _ => format!("{} items", count(items.len())),
    }
}

fn describe(op: &Op, left: Option<u64>) -> String {
    let to = |verb: &str, items: &[PathBuf], into: &Path| {
        format!(
            "{verb} {} to \u{201c}{}\u{201d}",
            named(items, left),
            name_of(into)
        )
    };
    match op {
        Op::Move { items, into } => to("Moving", items, into),
        Op::Copy { items, into } | Op::CopyExactly { items, into } => to("Copying", items, into),
        Op::Retry(request) => {
            let sources: Vec<PathBuf> = request.items().map(|(item, _)| item.into()).collect();
            let into = request.items().next().and_then(|(_, dest)| dest.parent());
            let verb = match request.kind() {
                Kind::Move => "Moving",
                Kind::Copy | Kind::CopyExactly => "Copying",
            };
            to(verb, &sources, into.unwrap_or(Path::new("/")))
        }
        Op::Trash(items) => to("Moving", items, Path::new("Trash")),
        Op::Duplicate(items) => match items.first().and_then(|first| first.parent()) {
            Some(into) => to("Duplicating", items, into),
            None => format!("Duplicating {}", named(items, left)),
        },
        Op::MakeAlias(items) => format!("Making Aliases of {}", named(items, left)),
        Op::Compress(items) => format!("Compressing {}", named(items, left)),
        Op::DeleteImmediately(items) => format!("Deleting {}", named(items, left)),
        Op::Rename { name, .. } => format!("Renaming to \u{201c}{name}\u{201d}"),
        Op::NewFolder { name, .. } | Op::NewFile { name, .. } => {
            format!("Creating \u{201c}{name}\u{201d}")
        }
        Op::PutBack(items) => {
            let back: Vec<PathBuf> = items.iter().map(|item| item.to.clone()).collect();
            format!("Putting Back {}", named(&back, left))
        }
        Op::Quick(quick) => format!("{} {}", quick.label(), named(quick.items(), left)),
        Op::Batch(ops) => ops.first().map(|op| describe(op, left)).unwrap_or_default(),
    }
}

fn failed_title(op: &Op, failed: &Failed) -> String {
    let verb = match op {
        Op::Move { .. } | Op::Trash(_) => "moved",
        Op::Retry(request) if request.kind() == Kind::Move => "moved",
        Op::PutBack(_) => "put back",
        Op::Duplicate(_) => "duplicated",
        _ => "copied",
    };
    if failed.refusal().is_some() {
        let left = sources(failed.retry.as_ref().unwrap_or(op));
        let left: Vec<PathBuf> = left.into_iter().map(Path::to_path_buf).collect();
        return format!("{} couldn't be {verb}", named(&left, None));
    }
    match failed.failures.as_slice() {
        [only] => format!(
            "\u{201c}{}\u{201d} couldn't be {verb}",
            name_of(&only.source)
        ),
        failures => format!("{} items couldn't be {verb}", count(failures.len())),
    }
}

fn sources(op: &Op) -> Vec<&Path> {
    match op {
        Op::Move { items, .. }
        | Op::Copy { items, .. }
        | Op::CopyExactly { items, .. }
        | Op::Duplicate(items) => items.iter().map(PathBuf::as_path).collect(),
        Op::Retry(request) => request.items().map(|(item, _)| item).collect(),
        Op::PutBack(items) => items.iter().map(|item| item.item.as_path()).collect(),
        _ => subject(op).into_iter().collect(),
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
        Op::Retry(request) => request.items().next().map(|(item, _)| item),
        Op::Rename { path, .. } => Some(path),
        Op::NewFolder { into, .. } | Op::NewFile { into, .. } => Some(into),
        Op::PutBack(items) => items.first().map(|item| item.item.as_path()),
        Op::Quick(quick) => quick.items().first().map(PathBuf::as_path),
        Op::Batch(ops) => ops.first().and_then(subject),
    }
}

/// Finder's own wording, with the speed this one also shows; the time left waits for the engine
/// to trust it.
fn caption(progress: Option<&Progress>) -> String {
    let Some(progress) = progress else {
        return "Preparing\u{2026}".to_string();
    };
    if progress.phase == Phase::Finishing {
        return "Finishing\u{2026}".to_string();
    }
    if progress.bytes_total == 0 {
        return match progress.items_total {
            0 => "Preparing\u{2026}".to_string(),
            total => format!(
                "{} of {} items",
                count(progress.items_done as usize),
                count(total as usize)
            ),
        };
    }
    let amount = format!(
        "{} of {}",
        byte_string(progress.bytes_done),
        byte_string(progress.bytes_total)
    );
    if progress.paused {
        return format!("{amount}{DASH}Paused");
    }
    let mut parts = vec![amount];
    if progress.bytes_per_sec >= 1. {
        parts.push(format!("{}/s", byte_string(progress.bytes_per_sec as u64)));
    }
    match progress.eta {
        Some(eta) => parts.push(remaining(eta.as_secs_f64())),
        None if parts.len() == 1 => parts.push("Estimating time remaining\u{2026}".to_string()),
        None => {}
    }
    parts.join(DASH)
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

/// The fraction done, or `None` while there is nothing yet to measure it by.
fn fraction(progress: Option<&Progress>) -> Option<f64> {
    let progress = progress?;
    if progress.phase == Phase::Finishing {
        return Some(1.);
    }
    if progress.counting {
        return None;
    }
    match (progress.bytes_total, progress.items_total) {
        (0, 0) => None,
        (0, items) => Some(progress.items_done as f64 / items as f64),
        (bytes, _) => Some(progress.bytes_done as f64 / bytes as f64),
    }
}

impl FinderDemo {
    pub(crate) fn progress_shown(&self) -> bool {
        self.jobs.iter().any(JobRow::shown)
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
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SHOW_AFTER).await;
            this.update(cx, |this, cx| {
                if this.jobs.iter().any(|row| row.id == id) {
                    cx.defer(crate::progress_window::reveal);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Reads each running job's progress; true while any job still runs.
    pub(crate) fn poll_jobs(&mut self) -> bool {
        let mut running = false;
        for row in self.jobs.iter_mut().filter(|row| row.running()) {
            if let Some(progress) = self.queue.progress(row.id) {
                row.progress = Some(progress);
            }
            running = true;
        }
        running
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
                Event::Carried(job, carried) => self.carried(job, carried, cx),
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
            }
        }
        cx.notify();
    }

    /// Items that failed keep their row, with what to show and what to try again.
    fn carried(&mut self, job: JobId, carried: Carried, cx: &mut Context<Self>) {
        let Some(mut row) = self.take_job(job) else {
            return;
        };
        let Carried {
            done,
            failures,
            end,
            left,
        } = carried;
        let clean = failures.is_empty() && end == End::Finished;
        if let Some(sound) = row.sound.filter(|_| clean && !done.results.is_empty()) {
            sound.play();
        }
        if let Some(left) = left.clone().filter(|_| !failures.is_empty()) {
            self.undo_left(&row, left);
        }
        self.finish(&row, done, cx);
        if failures.is_empty() || end == End::Cancelled {
            return;
        }
        row.failed = Some(Failed {
            failures,
            retry: left,
        });
        self.jobs.push(row);
        cx.defer(crate::progress_window::reveal);
    }

    fn take_job(&mut self, job: JobId) -> Option<JobRow> {
        let at = self.jobs.iter().position(|row| row.id == job)?;
        Some(self.jobs.remove(at))
    }

    fn finish(&mut self, row: &JobRow, done: Done, cx: &mut Context<Self>) {
        self.note(row, &done);
        self.settled(super::touched(&row.op), done.results, row.after, cx);
    }

    fn retry_job(&mut self, job: JobId, cx: &mut Context<Self>) {
        let Some(row) = self.take_job(job) else {
            return;
        };
        match row.failed.and_then(|failed| failed.retry) {
            Some(retry) => self.submit(
                retry,
                Conflict::Merge,
                row.after,
                Record::New,
                row.sound,
                cx,
            ),
            None => cx.notify(),
        }
    }

    fn show_failures(&self, job: JobId, cx: &mut Context<Self>) {
        let Some((op, failed)) = (self.jobs.iter())
            .find(|row| row.id == job)
            .and_then(|row| Some((&row.op, row.failed.as_ref()?)))
        else {
            return;
        };
        let items: Vec<PathBuf> = match failed.refusal() {
            Some(_) => sources(failed.retry.as_ref().unwrap_or(op))
                .into_iter()
                .filter(|path| path.exists())
                .map(Path::to_path_buf)
                .collect(),
            None => (failed.failures.iter())
                .flat_map(|failure| [&failure.source, &failure.dest])
                .find(|path| path.exists())
                .into_iter()
                .chain(failed.failures.iter().map(|failure| &failure.source))
                .filter(|path| path.exists())
                .cloned()
                .collect(),
        };
        cx.defer(move |cx| crate::navigate::reveal(&items, cx));
    }
}

fn act(
    demo: &Entity<FinderDemo>,
    job: JobId,
    act: impl Fn(&mut FinderDemo, JobId, &mut Context<FinderDemo>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let demo = demo.clone();
    move |_, _, cx| demo.update(cx, |this, cx| act(this, job, cx))
}

fn on_queue(
    demo: &Entity<FinderDemo>,
    job: JobId,
    run: fn(&Queue, JobId),
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    act(demo, job, move |this, job, cx| {
        run(&this.queue, job);
        this.poll_jobs();
        cx.notify();
    })
}

fn glyph(
    id: (&'static str, u64),
    right: f32,
    image: impl IntoElement,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .absolute()
        .right(px(right))
        .top(px(23.5))
        .size(px(GLYPH))
        .flex()
        .items_center()
        .justify_center()
        .child(image)
        .on_click(on_click)
        .into_any_element()
}

fn text(
    id: (&'static str, u64),
    text: &str,
    top: f32,
    right: f32,
    size: gpui::Pixels,
    color: SystemColor,
) -> AnyElement {
    div()
        .absolute()
        .left(px(TEXT_X))
        .right(px(right))
        .top(px(top))
        .child(native_label(id, text).font_size(size).color(color))
        .into_any_element()
}

/// One job, laid out as Finder lays out its copy window's rows.
pub(crate) fn progress_row(job: &JobRow, demo: &Entity<FinderDemo>) -> AnyElement {
    let id = job.id.raw();
    let small = tokens::small_system_font_size();
    let icon = subject(&job.op)
        .and_then(|path| tokens::file_icon(FileType::Path(&path.to_string_lossy()), px(ICON)));
    let stop = native_image(
        ("ng.finder.progress.stop.glyph", id),
        "NSStopProgressFreestandingTemplate",
    )
    .tint(SystemColor::SecondaryLabel);
    let row = div()
        .relative()
        .flex_none()
        .h(px(ROW_H))
        .when_some(icon, |row, icon| {
            row.child(
                div()
                    .absolute()
                    .left(px(17.))
                    .top(px(13.))
                    .child(native_raster(("ng.finder.progress.icon", id), icon)),
            )
        });
    if let Some(failed) = &job.failed {
        let reason = failed.reason();
        let button = |key: &'static str, title: &str| {
            native_button((key, id), title.to_owned()).control_size(ControlSize::Small)
        };
        return row
            .child(text(
                ("ng.finder.progress.label", id),
                &failed_title(&job.op, failed),
                5.,
                EDGE + GLYPH + GLYPH_GAP,
                tokens::system_font_size(),
                SystemColor::Label,
            ))
            .child(glyph(
                ("ng.finder.progress.dismiss", id),
                EDGE,
                stop,
                act(demo, job.id, |this, job, cx| {
                    this.take_job(job);
                    cx.notify();
                }),
            ))
            .child(text(
                ("ng.finder.progress.reason", id),
                reason.as_deref().unwrap_or_default(),
                22.,
                EDGE + GLYPH + GLYPH_GAP,
                small,
                SystemColor::SecondaryLabel,
            ))
            .child(
                div()
                    .absolute()
                    .right(px(EDGE))
                    .top(px(44.))
                    .flex()
                    .gap(px(8.))
                    .child(button("ng.finder.progress.show", "Show").on_click({
                        let demo = demo.clone();
                        let job = job.id;
                        move |_, _, cx| {
                            demo.update(cx, |this, cx| this.show_failures(job, cx));
                        }
                    }))
                    .when(failed.retry.is_some(), |buttons| {
                        buttons.child(button("ng.finder.progress.retry", "Retry").on_click({
                            let demo = demo.clone();
                            let job = job.id;
                            move |_, _, cx| demo.update(cx, |this, cx| this.retry_job(job, cx))
                        }))
                    }),
            )
            .into_any_element();
    }
    let progress = job.progress.as_ref();
    let left = progress
        .filter(|progress| progress.items_total > 1)
        .map(|progress| progress.items_total.saturating_sub(progress.items_done));
    let paused = progress.is_some_and(|progress| progress.paused);
    let pausable = job.carries_bytes();
    let bar_right = EDGE + GLYPH + GLYPH_GAP + if pausable { GLYPH + GLYPH_GAP } else { 0. };
    let bar = match fraction(progress) {
        Some(done) => native_progress(("ng.finder.progress.bar", id), done),
        None => native_progress(("ng.finder.progress.bar", id), 0.).indeterminate(true),
    };
    let current = progress
        .and_then(|progress| progress.current.as_ref())
        .filter(|_| progress.is_some_and(|progress| progress.phase == Phase::Copying))
        .map(|current| name_of(&current.path))
        .unwrap_or_default();
    row.child(text(
        ("ng.finder.progress.label", id),
        &describe(&job.op, left),
        5.,
        EDGE,
        tokens::system_font_size(),
        SystemColor::Label,
    ))
    .child(
        div()
            .absolute()
            .left(px(TEXT_X))
            .right(px(bar_right))
            .top(px(21.))
            .h(px(16.))
            .flex()
            .items_center()
            .child(bar),
    )
    .when(pausable, |row| {
        let symbol = if paused {
            "play.circle.fill"
        } else {
            "pause.circle.fill"
        };
        let run: fn(&Queue, JobId) = if paused { Queue::resume } else { Queue::pause };
        row.child(glyph(
            ("ng.finder.progress.pause", id),
            EDGE + GLYPH + GLYPH_GAP,
            native_symbol(
                ("ng.finder.progress.pause.glyph", id),
                symbol,
                GLYPH + 2.,
                FontWeight::NORMAL,
            )
            .tint(SystemColor::SecondaryLabel),
            on_queue(demo, job.id, run),
        ))
    })
    .child(glyph(
        ("ng.finder.progress.stop", id),
        EDGE,
        stop,
        on_queue(demo, job.id, Queue::cancel),
    ))
    .child(text(
        ("ng.finder.progress.caption", id),
        &caption(progress),
        38.,
        EDGE,
        small,
        SystemColor::SecondaryLabel,
    ))
    .child(text(
        ("ng.finder.progress.current", id),
        &current,
        54.,
        EDGE,
        px(10.),
        SystemColor::TertiaryLabel,
    ))
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(bytes_done: u64, bytes_total: u64) -> Progress {
        Progress {
            bytes_done,
            bytes_total,
            items_done: 1,
            items_total: 12,
            ..Progress::default()
        }
    }

    #[test]
    fn a_full_volume_names_what_is_left_rather_than_the_first_item() {
        use finder_fs::ops::transfer::{OnError, Request};
        let items = vec![PathBuf::from("/a/big4.bin"), PathBuf::from("/a/big.bin")];
        let into = PathBuf::from("/Volumes/Storage");
        let op = Op::Copy {
            items: items.clone(),
            into: into.clone(),
        };
        let why = "There isn't enough space on \u{201c}Storage\u{201d} to copy these items.";
        let left = items
            .iter()
            .map(|item| (item.clone(), into.join(name_of(item))));
        let failed = Failed {
            failures: vec![Failure {
                source: items[0].clone(),
                dest: into.join("big4.bin"),
                error: std::io::Error::new(ErrorKind::StorageFull, why),
            }],
            retry: Some(Op::Retry(Request::each(
                Kind::Copy,
                left,
                Conflict::Merge,
                OnError::Stop,
            ))),
        };
        assert_eq!(failed_title(&op, &failed), "2 items couldn't be copied");
        assert_eq!(failed.reason().as_deref(), Some(why));
    }

    #[test]
    fn the_caption_waits_for_a_speed_and_a_settled_time() {
        let mut shown = progress(0, 4_600_000_000);
        assert_eq!(
            caption(Some(&shown)),
            "Zero bytes of 4.6 GB \u{2013} Estimating time remaining\u{2026}"
        );
        shown.bytes_done = 1_200_000_000;
        shown.bytes_per_sec = 84_000_000.;
        assert_eq!(caption(Some(&shown)), "1.2 GB of 4.6 GB \u{2013} 84 MB/s");
        shown.eta = Some(Duration::from_secs(40));
        assert_eq!(
            caption(Some(&shown)),
            "1.2 GB of 4.6 GB \u{2013} 84 MB/s \u{2013} About 40 seconds"
        );
        shown.paused = true;
        assert_eq!(caption(Some(&shown)), "1.2 GB of 4.6 GB \u{2013} Paused");
        shown.phase = Phase::Finishing;
        assert_eq!(caption(Some(&shown)), "Finishing\u{2026}");
    }

    #[test]
    fn a_count_names_what_is_left() {
        let items = vec![PathBuf::from("/a/one"), PathBuf::from("/a/two")];
        let into = PathBuf::from("/Volumes/Storage");
        let op = Op::Copy { items, into };
        assert_eq!(
            describe(&op, None),
            "Copying 2 items to \u{201c}Storage\u{201d}"
        );
        assert_eq!(
            describe(&op, Some(3_035)),
            "Copying 3,035 items to \u{201c}Storage\u{201d}"
        );
        let one = Op::Move {
            items: vec![PathBuf::from("/a/big.bin")],
            into: PathBuf::from("/b"),
        };
        assert_eq!(
            describe(&one, Some(1)),
            "Moving \u{201c}big.bin\u{201d} to \u{201c}b\u{201d}"
        );
    }
}
