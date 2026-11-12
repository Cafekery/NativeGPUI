use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use objc2_foundation::NSFileManager;
use parking_lot::{Condvar, Mutex};

use super::{
    Conflict, Done, Placed, Refusal, Restore, deep, drop_slot, exchange, free_path, name_of,
    next_free, refusal_message, refuse_into_itself, taken, trash_slot, undo_of_transfer,
};
use meter::{Left, Meter};
use volume::Volume;

mod copy;
mod meter;
mod plan;
mod volume;

#[cfg(test)]
pub(crate) mod tests;

const TICK: Duration = Duration::from_millis(250);
const SPACE_EVERY: Duration = Duration::from_secs(1);
const PLAN_FOR: Duration = Duration::from_millis(500);
const RELEASE_BYTES: u64 = 256 << 20;
const RELEASE_ITEMS: usize = 1000;
const RELEASE_EVERY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Copy,
    Move,
    CopyExactly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnError {
    Continue,
    Stop,
}

#[derive(Clone, Debug)]
pub struct Request {
    kind: Kind,
    items: Vec<Item>,
    conflict: Conflict,
    on_error: OnError,
}

#[derive(Clone, Debug)]
struct Item {
    source: PathBuf,
    dest: PathBuf,
}

impl Request {
    pub fn new(
        kind: Kind,
        sources: Vec<PathBuf>,
        into: &Path,
        conflict: Conflict,
        on_error: OnError,
    ) -> Request {
        let pairs = sources.into_iter().map(|source| {
            let dest = into.join(source.file_name().unwrap_or_default());
            (source, dest)
        });
        Request::each(kind, pairs, conflict, on_error)
    }

    pub fn each(
        kind: Kind,
        pairs: impl IntoIterator<Item = (PathBuf, PathBuf)>,
        conflict: Conflict,
        on_error: OnError,
    ) -> Request {
        let items = pairs
            .into_iter()
            .map(|(source, dest)| Item { source, dest })
            .collect();
        Request {
            kind,
            items,
            conflict,
            on_error,
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn conflict(&self) -> Conflict {
        self.conflict
    }

    pub fn on_error(&self) -> OnError {
        self.on_error
    }

    pub fn items(&self) -> impl ExactSizeIterator<Item = (&Path, &Path)> {
        self.items
            .iter()
            .map(|item| (item.source.as_path(), item.dest.as_path()))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Copying,
    Finishing,
    Done,
}

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub phase: Phase,
    pub counting: bool,
    pub paused: bool,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub items_done: u64,
    pub items_total: u64,
    pub failures: u64,
    /// The largest file in flight.
    pub current: Option<Current>,
    pub bytes_per_sec: f64,
    pub eta: Option<Duration>,
}

#[derive(Clone, Debug)]
pub struct Current {
    pub path: PathBuf,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Finished,
    Cancelled,
    Stopped,
}

#[derive(Debug)]
pub struct Failure {
    pub source: PathBuf,
    pub dest: PathBuf,
    pub error: io::Error,
}

#[derive(Debug)]
pub struct Outcome {
    pub done: Done,
    pub failures: Vec<Failure>,
    pub end: End,
    retry: Option<Request>,
}

impl Outcome {
    pub fn retry(&self) -> Option<Request> {
        self.retry.clone()
    }
}

#[derive(Clone, Default)]
pub struct Transfer {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    paused: Mutex<bool>,
    unpaused: Condvar,
    cancelled: AtomicBool,
    stopped: AtomicBool,
    counts: Counts,
    status: Mutex<Status>,
}

#[derive(Default)]
struct Counts {
    bytes_done: AtomicU64,
    bytes_total: AtomicU64,
    items_done: AtomicU64,
    items_total: AtomicU64,
    failures: AtomicU64,
}

#[derive(Default)]
struct Status {
    phase: Phase,
    counting: bool,
    bytes_per_sec: f64,
    eta: Option<Duration>,
    slots: Vec<Arc<Slot>>,
}

#[derive(Default)]
struct Slot {
    file: Mutex<Option<(PathBuf, u64)>>,
    done: AtomicU64,
}

impl Transfer {
    pub fn progress(&self) -> Progress {
        let shared = &self.shared;
        let status = shared.status.lock();
        let current = status
            .slots
            .iter()
            .filter_map(|slot| {
                let file = slot.file.lock();
                let (path, total) = file.as_ref()?;
                Some(Current {
                    path: path.clone(),
                    bytes_done: slot.done.load(Ordering::Relaxed).min(*total),
                    bytes_total: *total,
                })
            })
            .max_by_key(|current| current.bytes_total);
        let counts = &shared.counts;
        Progress {
            phase: status.phase,
            counting: status.counting,
            paused: *shared.paused.lock(),
            bytes_done: counts.bytes_done.load(Ordering::Relaxed),
            bytes_total: counts.bytes_total.load(Ordering::Relaxed),
            items_done: counts.items_done.load(Ordering::Relaxed),
            items_total: counts.items_total.load(Ordering::Relaxed),
            failures: counts.failures.load(Ordering::Relaxed),
            current,
            bytes_per_sec: status.bytes_per_sec,
            eta: status.eta,
        }
    }

    pub fn pause(&self) {
        *self.shared.paused.lock() = true;
    }

    pub fn resume(&self) {
        *self.shared.paused.lock() = false;
        self.shared.unpaused.notify_all();
    }

    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::SeqCst);
        let _paused = self.shared.paused.lock();
        self.shared.unpaused.notify_all();
    }

    pub(crate) fn begin_steps(&self, items: u64) {
        self.shared.begin();
        self.shared
            .counts
            .items_total
            .store(items, Ordering::Relaxed);
        self.shared.status.lock().counting = false;
    }

    pub(crate) fn next_step(&self) -> bool {
        self.shared.wait_while_paused();
        !self.shared.halted()
    }

    pub(crate) fn step_done(&self) {
        self.shared
            .counts
            .items_done
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn run(&self, request: Request) -> Outcome {
        let shared = &*self.shared;
        shared.begin();
        let is_move = request.kind == Kind::Move;
        for item in &request.items {
            let checked = match (item.dest.parent(), item.source.file_name()) {
                (Some(into), Some(_)) => refuse_into_itself(&item.source, into, is_move),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "item has no file name",
                )),
            };
            if let Err(error) = checked {
                return refused(shared, item, error);
            }
        }
        let Some(first) = request.items.first() else {
            shared.enter(Phase::Done);
            return Outcome {
                done: Done::default(),
                failures: Vec::new(),
                end: End::Finished,
                retry: None,
            };
        };
        let into = first.dest.parent().unwrap_or(Path::new("/"));
        let volume = match Volume::of(into) {
            Ok(volume) => volume,
            Err(error) => return refused(shared, first, error),
        };
        let job = Job::new(shared, &request, volume);
        if job.volume.read_only {
            let error = io::Error::from_raw_os_error(libc::EROFS);
            let error = copy::worded(error, &first.source, &first.dest, &job);
            return refused(shared, first, error);
        }
        let slots: Vec<Arc<Slot>> = (0..job.workers()).map(|_| Arc::default()).collect();
        shared.status.lock().slots = slots.clone();
        let (tasks, queue) = mpsc::channel();
        let queue = Mutex::new(queue);
        let alive = AtomicUsize::new(slots.len() + 1);
        let waiter = thread::current();
        let leave = || {
            alive.fetch_sub(1, Ordering::SeqCst);
            waiter.unpark();
        };
        thread::scope(|scope| {
            scope.spawn(|| {
                plan::walk(&job, &request.items, tasks);
                if !job.halted() {
                    shared.status.lock().counting = false;
                }
                leave();
            });
            for slot in &slots {
                scope.spawn(|| {
                    copy::work(&job, &queue, slot);
                    leave();
                });
            }
            let mut meter = Meter::default();
            let mut space_checked = None;
            while alive.load(Ordering::SeqCst) > 0 {
                thread::park_timeout(TICK);
                job.tick(&mut meter, &mut space_checked);
            }
        });
        shared.status.lock().slots.clear();
        job.finish()
    }
}

fn refused(shared: &Shared, item: &Item, error: io::Error) -> Outcome {
    shared.counts.failures.fetch_add(1, Ordering::Relaxed);
    shared.stopped.store(true, Ordering::SeqCst);
    shared.enter(Phase::Done);
    Outcome {
        done: Done::default(),
        failures: vec![Failure {
            source: item.source.clone(),
            dest: item.dest.clone(),
            error,
        }],
        end: End::Stopped,
        retry: None,
    }
}

impl Shared {
    fn begin(&self) {
        // Not `cancelled`: a cancel sent before `run` must hold.
        self.stopped.store(false, Ordering::SeqCst);
        let counts = &self.counts;
        for count in [
            &counts.bytes_done,
            &counts.bytes_total,
            &counts.items_done,
            &counts.items_total,
            &counts.failures,
        ] {
            count.store(0, Ordering::Relaxed);
        }
        *self.status.lock() = Status {
            counting: true,
            ..Status::default()
        };
    }

    fn enter(&self, phase: Phase) {
        let mut status = self.status.lock();
        status.phase = phase;
        status.counting = false;
        status.eta = None;
    }

    fn halted(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst) || self.stopped.load(Ordering::SeqCst)
    }

    fn halt(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _paused = self.paused.lock();
        self.unpaused.notify_all();
    }

    fn wait_while_paused(&self) {
        let mut paused = self.paused.lock();
        while *paused && !self.halted() {
            self.unpaused.wait(&mut paused);
        }
    }
}

struct Gate {
    made: Mutex<Option<bool>>,
    settled: Condvar,
    gid: AtomicU32,
}

impl Gate {
    fn open(gid: u32) -> Arc<Gate> {
        Arc::new(Gate {
            made: Mutex::new(Some(true)),
            settled: Condvar::new(),
            gid: AtomicU32::new(gid),
        })
    }

    fn pending() -> Arc<Gate> {
        Arc::new(Gate {
            made: Mutex::new(None),
            settled: Condvar::new(),
            gid: AtomicU32::new(u32::MAX),
        })
    }

    fn settle(&self, made: bool, gid: u32) {
        self.gid.store(gid, Ordering::Relaxed);
        *self.made.lock() = Some(made);
        self.settled.notify_all();
    }

    fn gid(&self) -> u32 {
        self.gid.load(Ordering::Relaxed)
    }

    fn wait(&self, shared: &Shared) -> bool {
        let mut made = self.made.lock();
        loop {
            if let Some(made) = *made {
                return made;
            }
            if shared.halted() {
                return false;
            }
            self.settled.wait_for(&mut made, TICK);
        }
    }
}

enum What {
    Dir(Arc<Gate>),
    File,
    Link,
}

struct Task {
    what: What,
    root: usize,
    source: PathBuf,
    dest: PathBuf,
    parent: Arc<Gate>,
    size: u64,
    over: bool,
    is_root: bool,
    /// Lands where nothing of this transfer was before it, so an undo takes it away.
    top: bool,
    clone: bool,
}

struct Swap {
    root: usize,
    source: PathBuf,
    staged: PathBuf,
    dest: PathBuf,
}

struct Emptied {
    dir: PathBuf,
    junk: Vec<PathBuf>,
}

#[derive(Default)]
struct Record {
    resolved: Vec<Option<PathBuf>>,
    results: Vec<Option<PathBuf>>,
    landed: Vec<Restore>,
    displaced: Vec<Restore>,
    made: Vec<(PathBuf, PathBuf)>,
    premade: Vec<(usize, PathBuf)>,
    swaps: Vec<Swap>,
    emptied: Vec<Emptied>,
    held: Vec<Freed>,
    /// Folders whose source could not be read, left writable for a retry.
    unsettled: Vec<PathBuf>,
}

/// A source whose copy landed, deleted once the copy's volume is flushed.
struct Freed {
    source: PathBuf,
    dest: PathBuf,
    stamp: copy::Stamp,
}

#[derive(Default)]
struct Release {
    pairs: Vec<Freed>,
    bytes: u64,
    since: Option<Instant>,
}

struct Job<'a> {
    shared: &'a Shared,
    items: &'a [Item],
    kind: Kind,
    conflict: Conflict,
    on_error: OnError,
    volume: Volume,
    record: Mutex<Record>,
    failures: Mutex<Vec<Failure>>,
    release: Mutex<Release>,
    allocated_total: AtomicU64,
    allocated_done: AtomicU64,
}

impl<'a> Job<'a> {
    fn new(shared: &'a Shared, request: &'a Request, volume: Volume) -> Job<'a> {
        let count = request.items.len();
        Job {
            shared,
            items: &request.items,
            kind: request.kind,
            conflict: request.conflict,
            on_error: request.on_error,
            volume,
            record: Mutex::new(Record {
                resolved: vec![None; count],
                results: vec![None; count],
                ..Record::default()
            }),
            failures: Mutex::new(Vec::new()),
            release: Mutex::new(Release::default()),
            allocated_total: AtomicU64::new(0),
            allocated_done: AtomicU64::new(0),
        }
    }

    fn is_move(&self) -> bool {
        self.kind == Kind::Move
    }

    fn workers(&self) -> usize {
        match (self.volume.network, self.volume.xattrs) {
            (true, _) => 8,
            (false, true) => 4,
            (false, false) => 2,
        }
    }

    fn halted(&self) -> bool {
        self.shared.halted()
    }

    fn allocation(&self, task: &Task) -> u64 {
        match task.what {
            What::File if !task.clone || task.over => self.volume.allocated(task.size),
            What::Dir(_) | What::File | What::Link => 0,
        }
    }

    fn count(&self, task: &Task) {
        let counts = &self.shared.counts;
        counts.items_total.fetch_add(1, Ordering::Relaxed);
        counts.bytes_total.fetch_add(task.size, Ordering::Relaxed);
        self.allocated_total
            .fetch_add(self.allocation(task), Ordering::Relaxed);
    }

    fn count_done(&self, bytes: u64) {
        let counts = &self.shared.counts;
        counts.items_total.fetch_add(1, Ordering::Relaxed);
        counts.items_done.fetch_add(1, Ordering::Relaxed);
        counts.bytes_total.fetch_add(bytes, Ordering::Relaxed);
        counts.bytes_done.fetch_add(bytes, Ordering::Relaxed);
    }

    fn forget(&self, task: &Task, partial: u64) {
        if self.halted() {
            return;
        }
        let counts = &self.shared.counts;
        counts.items_total.fetch_sub(1, Ordering::Relaxed);
        counts.bytes_total.fetch_sub(task.size, Ordering::Relaxed);
        counts.bytes_done.fetch_sub(partial, Ordering::Relaxed);
        self.allocated_total
            .fetch_sub(self.allocation(task), Ordering::Relaxed);
    }

    fn item_done(&self) {
        self.shared
            .counts
            .items_done
            .fetch_add(1, Ordering::Relaxed);
    }

    fn bytes_done(&self, bytes: u64) {
        self.shared
            .counts
            .bytes_done
            .fetch_add(bytes, Ordering::Relaxed);
    }

    fn claim(&self, task: &Task) {
        self.allocated_done
            .fetch_add(self.allocation(task), Ordering::SeqCst);
    }

    fn unclaim(&self, task: &Task) {
        self.allocated_done
            .fetch_sub(self.allocation(task), Ordering::SeqCst);
    }

    fn needed(&self) -> u64 {
        let total = self.allocated_total.load(Ordering::SeqCst);
        total.saturating_sub(self.allocated_done.load(Ordering::SeqCst))
    }

    fn fits(&self) -> bool {
        if self.halted() {
            return false;
        }
        // Room is read on both sides of `needed`, or a file claiming or unclaiming counts twice.
        let room = self.volume.available();
        let needed = self.needed();
        if needed <= room || needed <= self.volume.available() {
            return true;
        }
        let first = &self.items[0];
        let resolved = self.record.lock().resolved[0].clone();
        let dest = resolved.unwrap_or_else(|| first.dest.clone());
        self.fail(
            &first.source,
            &dest,
            io::Error::from_raw_os_error(libc::ENOSPC),
        );
        false
    }

    fn refusal(&self, refusal: Refusal, dest: &Path) -> Option<String> {
        let item = match self.items {
            [only] => Some(name_of(&only.source)),
            _ => None,
        };
        refusal_message(
            refusal,
            item.as_deref(),
            dest,
            self.is_move(),
            self.needed(),
        )
    }

    fn fail(&self, source: &Path, dest: &Path, error: io::Error) {
        let fatal = matches!(
            error.raw_os_error(),
            Some(libc::ENOSPC | libc::EROFS | libc::EDQUOT)
        );
        let error = copy::worded(error, source, dest, self);
        self.shared.counts.failures.fetch_add(1, Ordering::Relaxed);
        self.failures.lock().push(Failure {
            source: source.to_path_buf(),
            dest: dest.to_path_buf(),
            error,
        });
        if fatal || self.on_error == OnError::Stop {
            self.shared.halt();
        }
    }

    fn resolved(&self, root: usize, dest: &Path) {
        self.record.lock().resolved[root] = Some(dest.to_path_buf());
    }

    fn result(&self, root: usize, dest: &Path) {
        self.record.lock().results[root] = Some(dest.to_path_buf());
    }

    fn landed(&self, task: &Task, at: &Path) {
        let mut record = self.record.lock();
        if task.top {
            record.landed.push(Restore {
                item: at.to_path_buf(),
                to: task.source.clone(),
            });
        }
        if task.is_root {
            record.results[task.root] = Some(at.to_path_buf());
            record.resolved[task.root] = Some(at.to_path_buf());
        }
    }

    fn made(&self, task: &Task) {
        self.landed(task, &task.dest);
        let made = (task.source.clone(), task.dest.clone());
        self.record.lock().made.push(made);
    }

    fn premade(&self, root: usize, dest: &Path) {
        self.record.lock().premade.push((root, dest.to_path_buf()));
    }

    /// Nothing has run yet, so each folder the plan made is still empty.
    fn unmake(&self) {
        let mut record = self.record.lock();
        for (root, dest) in std::mem::take(&mut record.premade) {
            if std::fs::remove_dir(&dest).is_err() {
                continue;
            }
            record.made.retain(|(_, made)| *made != dest);
            record.landed.retain(|landed| landed.item != dest);
            record.results[root] = None;
        }
    }

    fn next_free(&self, task: &Task) -> PathBuf {
        if !task.is_root {
            return free_path(&task.dest);
        }
        let item = &self.items[task.root];
        next_free(&item.source, &item.dest)
    }

    fn displaced(&self, item: PathBuf, to: &Path) {
        self.record.lock().displaced.push(Restore {
            item,
            to: to.to_path_buf(),
        });
    }

    fn placed(&self, root: usize, placed: Placed) {
        let mut record = self.record.lock();
        record.results[root] = placed.dest;
        record.landed.extend(placed.landed);
        record.displaced.extend(placed.displaced);
    }

    fn unsettled(&self, dest: &Path) {
        self.record.lock().unsettled.push(dest.to_path_buf());
    }

    fn stage(&self, swap: Swap) {
        self.record.lock().swaps.push(swap);
    }

    fn emptied(&self, dir: &Path, junk: Vec<PathBuf>) {
        self.record.lock().emptied.push(Emptied {
            dir: dir.to_path_buf(),
            junk,
        });
    }

    fn release(&self, task: &Task, at: &Path, stamp: copy::Stamp) {
        if !self.is_move() {
            return;
        }
        let pair = Freed {
            source: task.source.clone(),
            dest: at.to_path_buf(),
            stamp,
        };
        {
            let mut record = self.record.lock();
            let staged = record
                .swaps
                .iter()
                .any(|swap| swap.root == task.root && task.source.starts_with(&swap.source));
            if staged {
                record.held.push(pair);
                return;
            }
        }
        let mut release = self.release.lock();
        release.pairs.push(pair);
        release.bytes += task.size;
        release.since.get_or_insert_with(Instant::now);
    }

    fn tick(&self, meter: &mut Meter, space_checked: &mut Option<Instant>) {
        if !self.halted() {
            self.measure(meter);
            if !space_checked.is_some_and(|at| at.elapsed() < SPACE_EVERY) {
                *space_checked = Some(Instant::now());
                self.fits();
            }
        }
        self.release_sources(false);
    }

    fn measure(&self, meter: &mut Meter) {
        let counts = &self.shared.counts;
        let paused = *self.shared.paused.lock();
        let bytes_done = counts.bytes_done.load(Ordering::Relaxed);
        let bytes_total = counts.bytes_total.load(Ordering::Relaxed);
        let items_done = counts.items_done.load(Ordering::Relaxed);
        let items_total = counts.items_total.load(Ordering::Relaxed);
        let counting = self.shared.status.lock().counting;
        let left = Left {
            bytes: bytes_total.saturating_sub(bytes_done),
            items: items_total.saturating_sub(items_done),
            items_done,
            counting,
        };
        meter.sample(Instant::now(), bytes_done, paused, &left);
        let mut status = self.shared.status.lock();
        status.bytes_per_sec = meter.bytes_per_sec();
        status.eta = meter.eta().filter(|_| !paused);
    }

    /// Batched: flushing an SD card can take seconds.
    fn release_sources(&self, all: bool) {
        let batch = {
            let mut release = self.release.lock();
            let due = release.bytes >= RELEASE_BYTES
                || release.pairs.len() >= RELEASE_ITEMS
                || release
                    .since
                    .is_some_and(|since| since.elapsed() >= RELEASE_EVERY);
            if release.pairs.is_empty() || !(all || due) {
                return;
            }
            std::mem::take(&mut *release)
        };
        if let Err(error) = self.volume.flush() {
            for freed in &batch.pairs {
                let error = io::Error::new(error.kind(), error.to_string());
                let error = copy::not_removed(error, &freed.source);
                self.fail(&freed.source, &freed.dest, error);
            }
            return;
        }
        for freed in batch.pairs {
            // The flush names the mount point, which a vanished volume leaves behind as a plain folder.
            let landed = deep::lstat(&freed.dest)
                .is_ok_and(|meta| meta.dev() == self.volume.dev && meta.len() == freed.stamp.len());
            let kept = match (landed, freed.stamp.holds(&freed.source)) {
                (false, _) => Some("its copy can't be found"),
                (true, false) => Some("it changed after it was copied"),
                (true, true) => None,
            };
            if let Some(why) = kept {
                self.fail(&freed.source, &freed.dest, copy::kept(&freed.source, why));
                continue;
            }
            if let Err(error) = deep::remove_file(&freed.source) {
                let error = copy::not_removed(error, &freed.source);
                self.fail(&freed.source, &freed.dest, error);
            }
        }
    }

    fn finish(&self) -> Outcome {
        if !self.halted() {
            self.shared.enter(Phase::Finishing);
        }
        let mut record = std::mem::take(&mut *self.record.lock());
        for (source, dest) in record.made.iter().rev() {
            if !record.unsettled.contains(dest) {
                let _ = copy::settle_dir(self, source, dest);
            }
        }
        self.swap(&mut record);
        if self.allocated_done.load(Ordering::Relaxed) > 0 {
            if let Err(error) = self.volume.flush() {
                let into = &self.items[0].dest;
                self.fail(into, into, error);
            }
        }
        self.release_sources(true);
        if self.is_move() {
            for emptied in record
                .emptied
                .iter()
                .filter(|emptied| only_junk_in(emptied))
            {
                for junk in &emptied.junk {
                    let _ = deep::remove_file(junk);
                }
                let _ = deep::remove_dir(&emptied.dir);
            }
        }
        self.shared.enter(Phase::Done);
        let failures = std::mem::take(&mut *self.failures.lock());
        self.outcome(record, failures)
    }

    fn swap(&self, record: &mut Record) {
        let fm = NSFileManager::defaultManager();
        let mut held = std::mem::take(&mut record.held);
        for swap in std::mem::take(&mut record.swaps) {
            let failed = self
                .failures
                .lock()
                .iter()
                .any(|failure| failure.dest.starts_with(&swap.staged));
            let swapped = if failed || self.halted() {
                Err(None)
            } else {
                exchange(&swap.staged, &swap.dest).map_err(Some)
            };
            if let Err(error) = swapped {
                drop_slot(&swap.staged);
                held.retain(|freed| !freed.source.starts_with(&swap.source));
                if let Some(error) = error {
                    self.fail(&swap.source, &swap.dest, error);
                }
                continue;
            }
            match trash_slot(&fm, &swap.staged) {
                Ok(item) => record.displaced.push(Restore {
                    item,
                    to: swap.dest.clone(),
                }),
                Err(_) => drop_slot(&swap.staged),
            }
            record.landed.push(Restore {
                item: swap.dest.clone(),
                to: swap.source.clone(),
            });
            record.results[swap.root] = Some(swap.dest.clone());
            for freed in &mut held {
                if let Ok(inside) = freed.dest.strip_prefix(&swap.staged) {
                    freed.dest = swap.dest.join(inside);
                }
            }
        }
        self.release.lock().pairs.extend(held);
    }

    fn outcome(&self, record: Record, failures: Vec<Failure>) -> Outcome {
        let end = match (
            self.shared.cancelled.load(Ordering::SeqCst),
            self.shared.stopped.load(Ordering::SeqCst),
        ) {
            (true, _) => End::Cancelled,
            (false, true) => End::Stopped,
            (false, false) => End::Finished,
        };
        let unfinished = || {
            (self.items.iter().zip(&record.resolved))
                .filter_map(|(item, resolved)| {
                    let dest = resolved.clone()?;
                    taken(&item.source).then(|| Item {
                        source: item.source.clone(),
                        dest,
                    })
                })
                .collect()
        };
        let again: Vec<Item> = match end {
            End::Cancelled => Vec::new(),
            End::Stopped => unfinished(),
            End::Finished if self.is_move() && !failures.is_empty() => unfinished(),
            End::Finished => failures
                .iter()
                .filter(|failure| failure.source != failure.dest && taken(&failure.source))
                .map(|failure| Item {
                    source: failure.source.clone(),
                    dest: failure.dest.clone(),
                })
                .collect(),
        };
        let retry = (!again.is_empty()).then_some(Request {
            kind: self.kind,
            items: again,
            conflict: Conflict::Merge,
            on_error: self.on_error,
        });
        Outcome {
            done: Done {
                results: record.results.into_iter().flatten().collect(),
                undo: undo_of_transfer(self.is_move(), record.landed, record.displaced),
            },
            failures,
            end,
            retry,
        }
    }
}

fn only_junk_in(emptied: &Emptied) -> bool {
    deep::list(&emptied.dir).is_ok_and(|entries| {
        (entries.iter()).all(|(name, _)| emptied.junk.contains(&emptied.dir.join(name)))
    })
}
