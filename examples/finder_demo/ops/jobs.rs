use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use finder_fs::ops::transfer::{End, Failure, Kind, Phase, Progress};
use finder_fs::ops::{Carried, Conflict, Done, Op};
use finder_fs::queue::{Event, JobId, Queue};
use gpui::{AnyElement, App, ClickEvent, Context, Entity, FontWeight, Window, div, prelude::*, px};
use native_gpui::tokens::{self, FileType, SystemColor};
use native_gpui::{
    ControlSize, NativeImage, native_button, native_label, native_progress, native_raster,
    native_symbol,
};

use super::After;
use crate::app::FinderDemo;
use crate::data::{byte_string, count};
use crate::form::{text_height, text_width_at};
use crate::remote::{Ended, Jobs, Then};
use crate::sound::Sound;

const SHOW_AFTER: Duration = Duration::from_millis(500);
pub(crate) const WIDTH: f32 = 404.;
pub(crate) const ROW_H: f32 = 61.;
pub(crate) const LAST_ROW_H: f32 = 56.;
pub(crate) const HAIRLINE_Y: f32 = 54.;
const ICON: f32 = 32.;
const ICON_X: f32 = 18.;
const ICON_Y: f32 = 8.;
const TEXT_X: f32 = 60.;
const BAR_X: f32 = 62.;
const TITLE_Y: f32 = 2.;
const BAR_Y: f32 = 16.;
const BAR_H: f32 = 16.;
const REASON_Y: f32 = 18.;
const CAPTION_Y: f32 = 33.;
const BUTTONS_H: f32 = 20.;
/// A circle symbol is as wide as its point size.
const GLYPH: f32 = 11.;
const GLYPH_HIT: f32 = 20.;
const GLYPH_GAP: f32 = 9.;
const EDGE: f32 = 13.;
const DASH: &str = " \u{2013} ";

#[derive(Clone, Copy, PartialEq)]
enum Id {
    Local(JobId),
    Remote(u64),
}

impl Id {
    fn raw(self) -> u64 {
        match self {
            Id::Local(job) => job.raw(),
            Id::Remote(job) => job,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Record {
    New,
    Undo(usize),
    Redo(usize),
    Retry(u64),
}

pub(crate) struct JobRow {
    id: Id,
    op: Op,
    conflict: Conflict,
    after: After,
    record: Record,
    sound: Option<Sound>,
    began: Instant,
    progress: Option<Progress>,
    failed: Option<Failed>,
    artwork: Option<FileType<'static>>,
}

struct Failed {
    failures: Vec<Failure>,
    retry: Option<Op>,
    entry: Option<u64>,
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
    fn new(
        id: Id,
        op: Op,
        conflict: Conflict,
        after: After,
        record: Record,
        sound: Option<Sound>,
    ) -> Self {
        Self {
            id,
            op,
            conflict,
            after,
            record,
            sound,
            began: Instant::now(),
            progress: None,
            failed: None,
            artwork: None,
        }
    }

    pub(crate) fn shown(&self) -> bool {
        self.failed.is_some() || self.began.elapsed() >= SHOW_AFTER
    }

    pub(crate) fn running(&self) -> bool {
        self.failed.is_none()
    }

    pub(crate) fn height(&self) -> f32 {
        let Some(failed) = &self.failed else {
            return ROW_H;
        };
        let line = reason_height("");
        ROW_H + reason_height(&failed.reason().unwrap_or_default()) - line + BUTTONS_H - line
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
    if let Some(id) = path.to_str().filter(|id| crate::remote::is_remote(id)) {
        return crate::remote::title(id).to_owned();
    }
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn named<P: AsRef<Path>>(items: &[P], left: Option<u64>) -> String {
    match (items, left) {
        (_, Some(left)) if left > 1 => format!("{} items", count(left as usize)),
        ([only], _) => format!("\u{201c}{}\u{201d}", name_of(only.as_ref())),
        _ => format!("{} items", count(items.len())),
    }
}

fn describe(op: &Op, left: Option<u64>) -> String {
    let to = |verb: &str, what: String, into: &Path| {
        format!("{verb} {what} to \u{201c}{}\u{201d}", name_of(into))
    };
    match op {
        Op::Move { items, into } => to("Moving", named(items, left), into),
        Op::Copy { items, into } | Op::CopyExactly { items, into } => {
            to("Copying", named(items, left), into)
        }
        Op::Retry(request) => {
            let into = request.items().next().and_then(|(_, dest)| dest.parent());
            let verb = match request.kind() {
                Kind::Move => "Moving",
                Kind::Copy | Kind::CopyExactly => "Copying",
            };
            to(
                verb,
                named(&sources(op), left),
                into.unwrap_or(Path::new("/")),
            )
        }
        Op::Trash(items) => to("Moving", named(items, left), Path::new("Trash")),
        Op::Duplicate(items) => match items.first().and_then(|first| first.parent()) {
            Some(into) => to("Duplicating", named(items, left), into),
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
            let back: Vec<&Path> = items.iter().map(|item| item.to.as_path()).collect();
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
        let id = Id::Local(self.queue.submit(op.clone(), conflict));
        self.push_job(JobRow::new(id, op, conflict, after, record, sound), cx);
    }

    /// `fetch` pairs remote files with the cache paths they download to.
    pub(crate) fn submit_remote(
        &mut self,
        op: Op,
        fetch: Vec<(PathBuf, PathBuf)>,
        then: Option<Then>,
        after: After,
        cx: &mut Context<Self>,
    ) {
        let artwork = subject(&op).and_then(|path| self.remote_artwork(path));
        let sound = Sound::of(&op, false).filter(|_| fetch.is_empty());
        let id = Id::Remote(self.remote_jobs.start(op.clone(), fetch, then));
        let mut row = JobRow::new(id, op, Conflict::KeepBoth, after, Record::New, sound);
        row.artwork = artwork;
        self.push_job(row, cx);
    }

    fn push_job(&mut self, row: JobRow, cx: &mut Context<Self>) {
        let id = row.id;
        self.jobs.push(row);
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

    pub(crate) fn poll_jobs(&mut self) -> bool {
        let mut running = false;
        for row in self.jobs.iter_mut().filter(|row| row.running()) {
            let progress = match row.id {
                Id::Local(job) => self.queue.progress(job),
                Id::Remote(job) => self.remote_jobs.progress(job),
            };
            if let Some(progress) = progress {
                row.progress = Some(progress);
            }
            running = true;
        }
        running
    }

    fn note(&mut self, row: &JobRow, done: &Done) -> Option<u64> {
        match row.record {
            Record::New => self.journal.record(done, &row.op, row.conflict),
            Record::Redo(at) => self.journal.redone(at, done),
            Record::Retry(entry) if self.journal.retried(entry, done) => Some(entry),
            Record::Retry(_) => self.journal.record(done, &row.op, row.conflict),
            Record::Undo(_) => None,
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
                    let Some(row) = self.take_job(Id::Local(job)) else {
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
                    let Some(row) = self.take_job(Id::Local(job)) else {
                        continue;
                    };
                    if let Some(left) = left {
                        self.undo_left(&row, left);
                    }
                    self.finish(&row, done, cx);
                    self.complain(&error.to_string());
                }
                Event::Cancelled { job, done } => {
                    if let Some(row) = self.take_job(Id::Local(job)) {
                        self.finish(&row, done, cx);
                    }
                }
            }
        }
        cx.notify();
    }

    fn carried(&mut self, job: JobId, carried: Carried, cx: &mut Context<Self>) {
        let Some(mut row) = self.take_job(Id::Local(job)) else {
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
        if let Some(left) = left.as_ref().filter(|_| !failures.is_empty()) {
            self.undo_left(&row, left.clone());
        }
        let entry = self.finish(&row, done, cx);
        if failures.is_empty() || end == End::Cancelled {
            return;
        }
        row.failed = Some(Failed {
            failures,
            retry: left,
            entry,
        });
        self.jobs.push(row);
        cx.defer(crate::progress_window::reveal);
    }

    /// No journal entry: a server change is not undone.
    pub(crate) fn handle_remote_events(&mut self, cx: &mut Context<Self>) {
        let ended = self.remote_jobs.ended();
        if ended.is_empty() {
            return;
        }
        for (job, ended, then) in ended {
            let Ended {
                results,
                failures,
                end,
                taken,
                dropped,
            } = ended;
            let clean = failures.is_empty() && end == End::Finished;
            let row = self.take_job(Id::Remote(job));
            match (then, &row) {
                (Some(Then::MoveIn(items)), Some(row)) => {
                    self.settled(super::touched(&row.op), results.clone(), row.after, cx);
                    if let Some(folder) = results.into_iter().next() {
                        self.run_op(
                            Op::Move {
                                items,
                                into: folder,
                            },
                            After::Nothing,
                            cx,
                        );
                    }
                }
                (Some(then), _) => self.then(then, clean, cx),
                (None, Some(row)) => {
                    if let Some(sound) = row.sound.filter(|_| clean && !results.is_empty()) {
                        sound.play();
                    }
                    self.settled(super::touched(&row.op), results, row.after, cx);
                }
                (None, None) => {}
            }
            if let Some(name) = taken {
                self.complain(&super::conflicts::name_taken(&name));
            }
            if let Some(server) = dropped {
                self.eject_remote(server, cx);
            }
            let Some(mut row) = row.filter(|_| !failures.is_empty() && end != End::Cancelled)
            else {
                continue;
            };
            row.failed = Some(Failed {
                failures,
                retry: None,
                entry: None,
            });
            self.jobs.push(row);
            cx.defer(crate::progress_window::reveal);
        }
        cx.notify();
    }

    fn take_job(&mut self, job: Id) -> Option<JobRow> {
        let at = self.jobs.iter().position(|row| row.id == job)?;
        Some(self.jobs.remove(at))
    }

    fn finish(&mut self, row: &JobRow, done: Done, cx: &mut Context<Self>) -> Option<u64> {
        let entry = self.note(row, &done);
        self.settled(super::touched(&row.op), done.results, row.after, cx);
        entry
    }

    fn retry_job(&mut self, job: Id, cx: &mut Context<Self>) {
        let Some(row) = self.take_job(job) else {
            return;
        };
        let Some(Failed {
            retry: Some(retry),
            entry,
            ..
        }) = row.failed
        else {
            cx.notify();
            return;
        };
        let record = entry.map_or(Record::New, Record::Retry);
        self.submit(retry, Conflict::Merge, row.after, record, row.sound, cx);
    }

    fn show_failures(&self, job: Id, cx: &mut Context<Self>) {
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

fn act<E: 'static>(
    demo: &Entity<FinderDemo>,
    job: Id,
    act: impl Fn(&mut FinderDemo, Id, &mut Context<FinderDemo>) + 'static,
) -> impl Fn(&E, &mut Window, &mut App) + 'static {
    let demo = demo.clone();
    move |_, _, cx| demo.update(cx, |this, cx| act(this, job, cx))
}

fn on_queue(
    demo: &Entity<FinderDemo>,
    job: Id,
    (run, remote): (fn(&Queue, JobId), fn(&Jobs, u64)),
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    act(demo, job, move |this, job, cx| {
        match job {
            Id::Local(job) => run(&this.queue, job),
            Id::Remote(job) => remote(&this.remote_jobs, job),
        }
        this.poll_jobs();
        cx.notify();
    })
}

fn glyph(
    id: (&'static str, u64),
    right: f32,
    image: NativeImage,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let pad = (GLYPH_HIT - GLYPH) / 2.;
    div()
        .id(id)
        .absolute()
        .right(px(right - pad))
        .top(px(BAR_Y + (BAR_H - GLYPH_HIT) / 2.))
        .size(px(GLYPH_HIT))
        .flex()
        .items_center()
        .justify_center()
        .child(image)
        .on_click(on_click)
        .into_any_element()
}

fn symbol(id: (&'static str, u64), name: &str) -> NativeImage {
    native_symbol(id, name, GLYPH, FontWeight::NORMAL).tint(SystemColor::SecondaryLabel)
}

fn text(
    id: (&'static str, u64),
    text: &str,
    left: f32,
    top: f32,
    right: f32,
    color: SystemColor,
) -> AnyElement {
    div()
        .absolute()
        .left(px(left))
        .right(px(right))
        .top(px(top))
        .child(
            native_label(id, text)
                .font_size(tokens::small_system_font_size())
                .color(color),
        )
        .into_any_element()
}

fn reason_height(reason: &str) -> f32 {
    let width = WIDTH - TEXT_X - (EDGE + GLYPH + GLYPH_GAP);
    let small = f32::from(tokens::small_system_font_size());
    let line = text_height("Ag", small, FontWeight::NORMAL, width);
    text_height(reason, small, FontWeight::NORMAL, width).max(line)
}

fn current_file(job: &JobRow, progress: &Progress, left: Option<u64>) -> Option<String> {
    let current = progress.current.as_ref()?;
    let named = left.is_none_or(|left| left <= 1) && sources(&job.op).len() == 1;
    let sizable = current.bytes_total.saturating_mul(20) >= progress.bytes_total;
    (progress.phase == Phase::Copying && !named && sizable).then(|| name_of(&current.path))
}

pub(crate) fn progress_row(job: &JobRow, demo: &Entity<FinderDemo>) -> AnyElement {
    let id = job.id.raw();
    let small = tokens::small_system_font_size();
    let icon = match sources(&job.op).as_slice() {
        [_, _, ..] => tokens::file_icon(FileType::Named("NSMultipleDocuments"), px(ICON)),
        _ => match job.artwork {
            Some(artwork) => tokens::file_icon(artwork, px(ICON)),
            None => subject(&job.op).and_then(|path| {
                tokens::file_icon(FileType::Path(&path.to_string_lossy()), px(ICON))
            }),
        },
    };
    let stop = symbol(("ng.finder.progress.stop.glyph", id), "xmark.circle.fill");
    let row = div()
        .relative()
        .flex_none()
        .h(px(job.height()))
        .when_some(icon, |row, icon| {
            row.child(
                div()
                    .absolute()
                    .left(px(ICON_X))
                    .top(px(ICON_Y))
                    .child(native_raster(("ng.finder.progress.icon", id), icon)),
            )
        });
    let beside_glyph = EDGE + GLYPH + GLYPH_GAP;
    if let Some(failed) = &job.failed {
        let reason = failed.reason().unwrap_or_default();
        let reason_h = reason_height(&reason);
        let button = |key: &'static str, title: &str| {
            native_button((key, id), title.to_owned()).control_size(ControlSize::Small)
        };
        return row
            .child(text(
                ("ng.finder.progress.label", id),
                &failed_title(&job.op, failed),
                TEXT_X,
                TITLE_Y,
                beside_glyph,
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
            .child(
                div()
                    .absolute()
                    .left(px(TEXT_X))
                    .right(px(beside_glyph))
                    .top(px(REASON_Y))
                    .h(px(reason_h))
                    .child(
                        native_label(("ng.finder.progress.reason", id), &reason)
                            .font_size(small)
                            .color(SystemColor::SecondaryLabel)
                            .wraps(true),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .right(px(EDGE))
                    .top(px(CAPTION_Y + reason_h - reason_height("")))
                    .flex()
                    .gap(px(8.))
                    .child(button("ng.finder.progress.show", "Show").on_click(act(
                        demo,
                        job.id,
                        |this, job, cx| this.show_failures(job, cx),
                    )))
                    .when(failed.retry.is_some(), |buttons| {
                        buttons.child(button("ng.finder.progress.retry", "Retry").on_click(act(
                            demo,
                            job.id,
                            FinderDemo::retry_job,
                        )))
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
    let bar_right = beside_glyph + if pausable { GLYPH + GLYPH_GAP } else { 0. };
    let bar = match fraction(progress) {
        Some(done) => native_progress(("ng.finder.progress.bar", id), done),
        None => native_progress(("ng.finder.progress.bar", id), 0.).indeterminate(true),
    };
    let caption = caption(progress);
    let current_x = TEXT_X + text_width_at(&caption, f32::from(small)) - 1.;
    let current = progress
        .and_then(|progress| current_file(job, progress, left))
        .filter(|_| WIDTH - EDGE - current_x >= 48.);
    row.child(text(
        ("ng.finder.progress.label", id),
        &describe(&job.op, left),
        TEXT_X,
        TITLE_Y,
        EDGE,
        SystemColor::Label,
    ))
    .child(
        div()
            .absolute()
            .left(px(BAR_X))
            .right(px(bar_right))
            .top(px(BAR_Y))
            .h(px(BAR_H))
            .flex()
            .items_center()
            .child(bar),
    )
    .when(pausable, |row| {
        let (name, run): (_, (fn(&Queue, JobId), fn(&Jobs, u64))) = if paused {
            ("play.circle.fill", (Queue::resume, Jobs::resume))
        } else {
            ("pause.circle.fill", (Queue::pause, Jobs::pause))
        };
        row.child(glyph(
            ("ng.finder.progress.pause", id),
            beside_glyph,
            symbol(("ng.finder.progress.pause.glyph", id), name),
            on_queue(demo, job.id, run),
        ))
    })
    .child(glyph(
        ("ng.finder.progress.stop", id),
        EDGE,
        stop,
        on_queue(demo, job.id, (Queue::cancel, Jobs::cancel)),
    ))
    .child(text(
        ("ng.finder.progress.caption", id),
        &caption,
        TEXT_X,
        CAPTION_Y,
        EDGE,
        SystemColor::SecondaryLabel,
    ))
    .when_some(current, |row, name| {
        row.child(text(
            ("ng.finder.progress.current", id),
            &format!("\u{2013} {name}"),
            current_x,
            CAPTION_Y,
            EDGE,
            SystemColor::TertiaryLabel,
        ))
    })
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
        let into = PathBuf::from("/Volumes/Archive");
        let op = Op::Copy {
            items: items.clone(),
            into: into.clone(),
        };
        let why = "There isn't enough space on \u{201c}Archive\u{201d} to copy these items.";
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
            entry: None,
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
        let into = PathBuf::from("/Volumes/Archive");
        let op = Op::Copy { items, into };
        assert_eq!(
            describe(&op, None),
            "Copying 2 items to \u{201c}Archive\u{201d}"
        );
        assert_eq!(
            describe(&op, Some(3_035)),
            "Copying 3,035 items to \u{201c}Archive\u{201d}"
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
