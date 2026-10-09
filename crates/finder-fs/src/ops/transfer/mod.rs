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
    Conflict, Done, Placed, Refusal, Restore, deep, exchange, refusal_message, refuse_into_itself,
    taken, trash_as, undo_of_transfer,
};
use meter::{Left, Meter};
use volume::Volume;

mod copy;
mod meter;
mod plan;
mod volume;

const TICK: Duration = Duration::from_millis(250);
const SPACE_EVERY: Duration = Duration::from_secs(1);
const RELEASE_BYTES: u64 = 256 << 20;
const RELEASE_ITEMS: usize = 1000;
const RELEASE_EVERY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Copy,
    Move,
    /// A copy that also keeps each item's owner and group, as far as this process may set them.
    CopyExactly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnError {
    /// Note the item and go on with the rest.
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
        let items = sources
            .into_iter()
            .map(|source| Item {
                dest: into.join(source.file_name().unwrap_or_default()),
                source,
            })
            .collect();
        Request {
            kind,
            items,
            conflict,
            on_error,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Copying,
    /// Writing out what the destination still holds in memory, so that done means on the disk.
    Finishing,
    Done,
}

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub phase: Phase,
    /// The totals still grow while the sources are walked.
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
    /// Only once the speed has settled and every item is counted.
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
    /// A failure under `OnError::Stop`, or one no other item could get past, such as a full disk.
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
    /// The failed items again, or after a stop everything not yet done, merged into what landed.
    pub fn retry(&self) -> Option<Request> {
        self.retry.clone()
    }
}

/// One transfer: `run` it on a worker thread and read `progress` from anywhere else.
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

/// The file one worker is copying.
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

    /// A file being copied pauses where it is.
    pub fn pause(&self) {
        *self.shared.paused.lock() = true;
    }

    pub fn resume(&self) {
        *self.shared.paused.lock() = false;
        self.shared.unpaused.notify_all();
    }

    /// What has landed stays; a file caught halfway is removed.
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::SeqCst);
        let _paused = self.shared.paused.lock();
        self.shared.unpaused.notify_all();
    }

    pub fn run(&self, request: Request) -> Outcome {
        let shared = &*self.shared;
        shared.begin();
        for item in &request.items {
            if let Err(error) = refusal(&request, item) {
                return refused(shared, item, error);
            }
        }
        let Some(first) = request.items.first() else {
            shared.end();
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
                shared.status.lock().counting = false;
                leave();
            });
            for slot in &slots {
                scope.spawn(|| {
                    copy::work(&job, &queue, slot);
                    leave();
                });
            }
            let mut meter = Meter::new();
            while alive.load(Ordering::SeqCst) > 0 {
                thread::park_timeout(TICK);
                job.tick(&mut meter);
            }
        });
        shared.status.lock().slots.clear();
        job.finish(&request)
    }
}

fn refusal(request: &Request, item: &Item) -> io::Result<()> {
    let into = item.dest.parent();
    match (into, item.source.file_name()) {
        (Some(into), Some(_)) => refuse_into_itself(&item.source, into, request.kind == Kind::Move),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "item has no file name",
        )),
    }
}

/// Nothing was touched: the whole request was turned down before it began.
fn refused(shared: &Shared, item: &Item, error: io::Error) -> Outcome {
    shared.counts.failures.fetch_add(1, Ordering::Relaxed);
    shared.stopped.store(true, Ordering::SeqCst);
    shared.end();
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
        self.cancelled.store(false, Ordering::SeqCst);
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

    fn end(&self) {
        let mut status = self.status.lock();
        status.phase = Phase::Done;
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

    /// Returns at once once the transfer halts, so a paused job can still be cancelled.
    fn wait_while_paused(&self) {
        let mut paused = self.paused.lock();
        while *paused && !self.halted() {
            self.unpaused.wait(&mut paused);
        }
    }
}

/// A destination folder that the items inside it wait for.
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
    /// Makes the folder and opens its gate.
    Dir(Arc<Gate>),
    File,
    Link,
    /// A folder whose paths run past `PATH_MAX`, taken whole by descriptor.
    Deep,
}

struct Task {
    what: What,
    root: usize,
    source: PathBuf,
    dest: PathBuf,
    parent: Arc<Gate>,
    size: u64,
    /// Something is already at `dest` and goes to the Trash once this lands.
    over: bool,
    /// The top-level item itself.
    is_root: bool,
    /// The first thing created where there was nothing, which an undo takes away.
    top: bool,
    /// Same volume as the destination, and it clones.
    clone: bool,
}

/// A folder this transfer made, given its source's metadata once everything inside has landed.
struct Made {
    source: PathBuf,
    dest: PathBuf,
}

/// A copy staged beside the item it replaces, swapped in once all of it has landed.
struct Swap {
    root: usize,
    source: PathBuf,
    staged: PathBuf,
    dest: PathBuf,
}

/// A source folder a move empties, with the leftovers it never carried.
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
    made: Vec<Made>,
    swaps: Vec<Swap>,
    emptied: Vec<Emptied>,
    /// Moved files inside a staged replace, whose sources go once it is swapped in.
    held: Vec<(PathBuf, PathBuf)>,
    /// Folders whose source could not be read, left writable for a retry.
    unsettled: Vec<PathBuf>,
}

#[derive(Default)]
struct Release {
    pairs: Vec<(PathBuf, PathBuf)>,
    bytes: u64,
    since: Option<Instant>,
}

struct Job<'a> {
    shared: &'a Shared,
    kind: Kind,
    conflict: Conflict,
    on_error: OnError,
    volume: Volume,
    record: Mutex<Record>,
    failures: Mutex<Vec<Failure>>,
    release: Mutex<Release>,
    allocated_total: AtomicU64,
    allocated_done: AtomicU64,
    space_checked: Mutex<Option<Instant>>,
}

impl<'a> Job<'a> {
    fn new(shared: &'a Shared, request: &Request, volume: Volume) -> Job<'a> {
        let count = request.items.len();
        Job {
            shared,
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
            space_checked: Mutex::new(None),
        }
    }

    fn is_move(&self) -> bool {
        self.kind == Kind::Move
    }

    /// Latency, not bandwidth, bounds a network volume, so it takes more files at once.
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

    /// A clone shares its source's blocks, so only a real copy needs room.
    fn allocation(&self, task: &Task) -> u64 {
        match task.what {
            What::File if !(task.clone && !task.over) => self.volume.allocated(task.size),
            What::Dir(_) | What::File | What::Link | What::Deep => 0,
        }
    }

    fn count(&self, task: &Task) {
        let counts = &self.shared.counts;
        counts.items_total.fetch_add(1, Ordering::Relaxed);
        counts.bytes_total.fetch_add(task.size, Ordering::Relaxed);
        self.allocated_total
            .fetch_add(self.allocation(task), Ordering::Relaxed);
    }

    /// An item found already in place, as a merge finds a file that matches.
    fn count_done(&self, bytes: u64) {
        let counts = &self.shared.counts;
        counts.items_total.fetch_add(1, Ordering::Relaxed);
        counts.items_done.fetch_add(1, Ordering::Relaxed);
        counts.bytes_total.fetch_add(bytes, Ordering::Relaxed);
        counts.bytes_done.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Takes back what a task that never landed was counted for, partial bytes included.
    fn forget(&self, task: &Task, partial: u64) {
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

    fn file_done(&self, task: &Task) {
        self.allocated_done
            .fetch_add(self.allocation(task), Ordering::Relaxed);
    }

    fn needed(&self) -> u64 {
        let total = self.allocated_total.load(Ordering::Relaxed);
        total.saturating_sub(self.allocated_done.load(Ordering::Relaxed))
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
        }
    }

    fn made(&self, task: &Task) {
        self.landed(task, &task.dest);
        self.record.lock().made.push(Made {
            source: task.source.clone(),
            dest: task.dest.clone(),
        });
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

    /// A moved file's source goes only once its copy is on the destination's disk.
    fn release(&self, task: &Task, at: &Path) {
        if !self.is_move() {
            return;
        }
        let pair = (task.source.clone(), at.to_path_buf());
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

    fn tick(&self, meter: &mut Meter) {
        let counts = &self.shared.counts;
        let now = Instant::now();
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
        meter.sample(now, bytes_done, paused, &left);
        {
            let mut status = self.shared.status.lock();
            status.bytes_per_sec = meter.bytes_per_sec();
            status.eta = meter.eta().filter(|_| !paused);
        }
        self.check_space(now);
        self.release_sources(false);
    }

    fn check_space(&self, now: Instant) {
        {
            let mut checked = self.space_checked.lock();
            if checked.is_some_and(|at| now.duration_since(at) < SPACE_EVERY) {
                return;
            }
            *checked = Some(now);
        }
        let needed = self.needed();
        if self.halted() || needed == 0 || needed <= self.volume.available() {
            return;
        }
        let record = self.record.lock();
        let at = record.resolved.iter().flatten().next().cloned();
        drop(record);
        if let Some(at) = at {
            self.fail(&at, &at, io::Error::from_raw_os_error(libc::ENOSPC));
        }
    }

    /// Batches the flush, which on an SD card can take seconds, rather than paying it per file.
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
            for (source, dest) in &batch.pairs {
                let error = io::Error::new(error.kind(), error.to_string());
                self.fail(source, dest, copy::not_removed(error, source));
            }
            return;
        }
        for (source, dest) in batch.pairs {
            if let Err(error) = std::fs::remove_file(&source) {
                self.fail(&source, &dest, copy::not_removed(error, &source));
            }
        }
    }

    fn finish(&self, request: &Request) -> Outcome {
        {
            let mut status = self.shared.status.lock();
            status.phase = Phase::Finishing;
            status.counting = false;
            status.eta = None;
        }
        let mut record = std::mem::take(&mut *self.record.lock());
        let made = record.made.iter().rev();
        for made in made.filter(|made| !record.unsettled.contains(&made.dest)) {
            let _ = copy::settle_dir(self, &made.source, &made.dest);
        }
        self.swap(&mut record);
        if self.shared.counts.items_done.load(Ordering::Relaxed) > 0 {
            if let Err(error) = self.volume.flush() {
                let into = request.items.first().map(|item| item.dest.clone());
                let into = into.unwrap_or_default();
                self.fail(&into, &into, error);
            }
        }
        self.release_sources(true);
        if self.is_move() {
            for emptied in &record.emptied {
                for junk in &emptied.junk {
                    let _ = std::fs::remove_file(junk);
                }
                let _ = std::fs::remove_dir(&emptied.dir);
            }
        }
        self.shared.end();
        let failures = std::mem::take(&mut *self.failures.lock());
        self.outcome(request, record, failures)
    }

    /// A staged replace swaps in only whole: with anything inside it failed, the original stays.
    fn swap(&self, record: &mut Record) {
        let fm = NSFileManager::defaultManager();
        let mut held = std::mem::take(&mut record.held);
        for swap in std::mem::take(&mut record.swaps) {
            let failed = self
                .failures
                .lock()
                .iter()
                .any(|failure| failure.dest.starts_with(&swap.staged));
            let swapped = match failed || self.halted() {
                true => Err(None),
                false => exchange(&swap.staged, &swap.dest).map_err(Some),
            };
            if let Err(error) = swapped {
                let _ = deep::remove(&swap.staged);
                held.retain(|(source, _)| !source.starts_with(&swap.source));
                if let Some(error) = error {
                    self.fail(&swap.source, &swap.dest, error);
                }
                continue;
            }
            match trash_as(&fm, &swap.staged, &swap.dest) {
                Ok(item) => record.displaced.push(Restore {
                    item,
                    to: swap.dest.clone(),
                }),
                Err(_) => {
                    let _ = deep::remove(&swap.staged);
                }
            }
            record.landed.push(Restore {
                item: swap.dest.clone(),
                to: swap.source.clone(),
            });
            record.results[swap.root] = Some(swap.dest.clone());
            for (_, at) in held.iter_mut() {
                if let Ok(inside) = at.strip_prefix(&swap.staged) {
                    *at = swap.dest.join(inside);
                }
            }
        }
        self.release.lock().pairs.extend(held);
    }

    fn outcome(&self, request: &Request, record: Record, failures: Vec<Failure>) -> Outcome {
        let end = match (
            self.shared.cancelled.load(Ordering::SeqCst),
            self.shared.stopped.load(Ordering::SeqCst),
        ) {
            (true, _) => End::Cancelled,
            (false, true) => End::Stopped,
            (false, false) => End::Finished,
        };
        let again: Vec<Item> = match end {
            End::Cancelled => Vec::new(),
            End::Stopped => request
                .items
                .iter()
                .zip(&record.resolved)
                .filter_map(|(item, resolved)| {
                    let dest = resolved.clone()?;
                    taken(&item.source).then(|| Item {
                        source: item.source.clone(),
                        dest,
                    })
                })
                .collect(),
            End::Finished => failures
                .iter()
                .filter(|failure| failure.source != failure.dest && taken(&failure.source))
                .map(|failure| Item {
                    source: failure.source.clone(),
                    dest: failure.dest.clone(),
                })
                .collect(),
        };
        let retry = (!again.is_empty()).then(|| Request {
            kind: request.kind,
            items: again,
            conflict: Conflict::Merge,
            on_error: request.on_error,
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

    fn no_space_message(&self, item: &str, dest: &Path) -> Option<String> {
        refusal_message(Refusal::NoSpace, item, dest, self.is_move(), self.needed())
    }

    fn read_only_message(&self, item: &str, dest: &Path) -> Option<String> {
        refusal_message(Refusal::ReadOnly, item, dest, self.is_move(), 0)
    }
}
