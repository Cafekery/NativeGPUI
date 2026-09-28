use std::collections::{HashSet, VecDeque};
use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crate::ops::{self, Conflict, Done, Op};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobId(u64);

impl JobId {
    pub fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Debug)]
pub enum Event {
    Started(JobId),
    Progress {
        job: JobId,
        done_bytes: u64,
        total_bytes: u64,
        bytes_per_sec: f64,
        item: PathBuf,
    },
    Finished(JobId, io::Result<Done>),
    Failed {
        job: JobId,
        done: Done,
        error: io::Error,
        left: Option<Op>,
    },
    Paused,
    Resumed,
    Cancelled {
        job: JobId,
        done: Done,
    },
}

struct Job {
    id: JobId,
    op: Op,
    conflict: Conflict,
}

struct State {
    jobs: VecDeque<Job>,
    paused: bool,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    cond: Condvar,
    cancelled: Mutex<HashSet<u64>>,
    pending: AtomicUsize,
    next_id: AtomicU64,
    on_event: Mutex<Box<dyn Fn(Event) + Send>>,
}

impl Shared {
    fn emit(&self, event: Event) {
        (self.on_event.lock().unwrap())(event);
    }
}

#[derive(Clone)]
pub struct Queue {
    shared: Arc<Shared>,
    _alive: Arc<Alive>,
}

/// The worker holds `Shared` itself, so the last `Queue` to go tells it to finish up and exit.
struct Alive(Arc<Shared>);

impl Drop for Alive {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().shutdown = true;
        self.0.cond.notify_all();
    }
}

impl Queue {
    pub fn new(on_event: impl Fn(Event) + Send + 'static) -> Queue {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                jobs: VecDeque::new(),
                paused: false,
                shutdown: false,
            }),
            cond: Condvar::new(),
            cancelled: Mutex::new(HashSet::new()),
            pending: AtomicUsize::new(0),
            next_id: AtomicU64::new(1),
            on_event: Mutex::new(Box::new(on_event)),
        });
        let worker = shared.clone();
        thread::spawn(move || worker_loop(worker));
        Queue {
            _alive: Arc::new(Alive(shared.clone())),
            shared,
        }
    }

    pub fn submit(&self, op: Op, conflict: Conflict) -> JobId {
        let id = JobId(self.shared.next_id.fetch_add(1, Ordering::SeqCst));
        self.shared.pending.fetch_add(1, Ordering::SeqCst);
        self.shared
            .state
            .lock()
            .unwrap()
            .jobs
            .push_back(Job { id, op, conflict });
        self.shared.cond.notify_all();
        id
    }

    pub fn pause(&self) {
        self.shared.state.lock().unwrap().paused = true;
        self.shared.emit(Event::Paused);
    }

    pub fn resume(&self) {
        self.shared.state.lock().unwrap().paused = false;
        self.shared.cond.notify_all();
        self.shared.emit(Event::Resumed);
    }

    /// A job already running finishes its current item first.
    pub fn cancel(&self, job: JobId) {
        let mut state = self.shared.state.lock().unwrap();
        if let Some(pos) = state.jobs.iter().position(|j| j.id == job) {
            state.jobs.remove(pos);
            drop(state);
            self.shared.pending.fetch_sub(1, Ordering::SeqCst);
            self.shared.emit(Event::Cancelled {
                job,
                done: batch_done(Vec::new(), Vec::new()),
            });
        } else {
            drop(state);
            self.shared.cancelled.lock().unwrap().insert(job.0);
        }
    }

    pub fn pending(&self) -> usize {
        self.shared.pending.load(Ordering::SeqCst)
    }
}

fn next_job(shared: &Shared) -> Option<Job> {
    loop {
        let mut state = shared.state.lock().unwrap();
        while !state.shutdown && (state.paused || state.jobs.is_empty()) {
            state = shared.cond.wait(state).unwrap();
        }
        if state.paused || state.jobs.is_empty() {
            return None;
        }
        let front_id = state.jobs.front().unwrap().id;
        if shared.cancelled.lock().unwrap().remove(&front_id.0) {
            let job = state.jobs.pop_front().unwrap();
            drop(state);
            shared.pending.fetch_sub(1, Ordering::SeqCst);
            shared.emit(Event::Cancelled {
                job: job.id,
                done: batch_done(Vec::new(), Vec::new()),
            });
            continue;
        }
        return state.jobs.pop_front();
    }
}

fn wait_while_paused(shared: &Shared) {
    let mut state = shared.state.lock().unwrap();
    while state.paused && !state.shutdown {
        state = shared.cond.wait(state).unwrap();
    }
}

fn worker_loop(shared: Arc<Shared>) {
    while let Some(job) = next_job(&shared) {
        shared.emit(Event::Started(job.id));
        let outcome = run_job(&shared, job.id, job.op, job.conflict);
        shared.pending.fetch_sub(1, Ordering::SeqCst);
        match outcome {
            RunOutcome::Done(result) => shared.emit(Event::Finished(job.id, result)),
            RunOutcome::Failed { done, error, left } => shared.emit(Event::Failed {
                job: job.id,
                done,
                error,
                left,
            }),
            RunOutcome::Cancelled(done) => shared.emit(Event::Cancelled { job: job.id, done }),
        }
    }
}

enum RunOutcome {
    Done(io::Result<Done>),
    Failed {
        done: Done,
        error: io::Error,
        left: Option<Op>,
    },
    Cancelled(Done),
}

fn run_job(shared: &Shared, id: JobId, op: Op, conflict: Conflict) -> RunOutcome {
    let items = split_items(&op).map(<[PathBuf]>::to_vec);
    let Some(items) = items else {
        wait_while_paused(shared);
        return match ops::perform(op, conflict, |_| {}).map_err(ops::Unfinished::from_error) {
            Ok(done) => RunOutcome::Done(Ok(done)),
            Err(Ok(unfinished)) => RunOutcome::Failed {
                done: unfinished.done,
                error: unfinished.cause,
                left: Some(unfinished.left),
            },
            Err(Err(error)) => RunOutcome::Done(Err(error)),
        };
    };
    run_split_job(shared, id, op, items, conflict)
}

fn run_split_job(
    shared: &Shared,
    id: JobId,
    op: Op,
    items: Vec<PathBuf>,
    conflict: Conflict,
) -> RunOutcome {
    let sized = moves_bytes(&op, &items);
    let total_bytes = match sized {
        true => total_size(items.clone(), Duration::from_millis(100)),
        false => 0,
    };
    let mut meter = Meter::new();
    let mut results = Vec::with_capacity(items.len());
    let mut undos = Vec::with_capacity(items.len());
    for (at, item) in items.iter().enumerate() {
        wait_while_paused(shared);
        if shared.cancelled.lock().unwrap().remove(&id.0) {
            return RunOutcome::Cancelled(batch_done(results, undos));
        }
        let size = if sized { ops::walk_size(item) } else { 0 };
        match ops::perform(op_for(&op, vec![item.clone()]), conflict, |_| {}) {
            Ok(done) => {
                results.extend(done.results);
                undos.extend(done.undo);
                let bytes_per_sec = meter.tick(size);
                shared.emit(Event::Progress {
                    job: id,
                    done_bytes: meter.done_bytes,
                    total_bytes,
                    bytes_per_sec,
                    item: item.clone(),
                });
            }
            Err(error) => {
                let error = match ops::Unfinished::from_error(error) {
                    Ok(unfinished) => {
                        results.extend(unfinished.done.results);
                        undos.extend(unfinished.done.undo);
                        unfinished.cause
                    }
                    Err(error) => error,
                };
                return RunOutcome::Failed {
                    done: batch_done(results, undos),
                    error,
                    left: Some(op_for(&op, items[at..].to_vec())),
                };
            }
        }
    }
    RunOutcome::Done(Ok(batch_done(results, undos)))
}

fn batch_done(results: Vec<PathBuf>, mut undos: Vec<Op>) -> Done {
    let undo = match undos.len() {
        0 => None,
        1 => undos.pop(),
        _ => Some(Op::Batch(undos)),
    };
    Done { results, undo }
}

fn split_items(op: &Op) -> Option<&[PathBuf]> {
    match op {
        Op::Move { items, .. }
        | Op::Copy { items, .. }
        | Op::CopyExactly { items, .. }
        | Op::Duplicate(items)
        | Op::MakeAlias(items)
        | Op::Trash(items)
        | Op::DeleteImmediately(items) => Some(items),
        // One archive comes out of the whole selection, so it is never split into item jobs.
        Op::Compress(_)
        | Op::Rename { .. }
        | Op::NewFolder { .. }
        | Op::NewFile { .. }
        | Op::PutBack(_)
        | Op::Quick(_)
        | Op::Batch(_) => None,
    }
}

fn op_for(op: &Op, items: Vec<PathBuf>) -> Op {
    match op {
        Op::Move { into, .. } => Op::Move {
            items,
            into: into.clone(),
        },
        Op::Copy { into, .. } => Op::Copy {
            items,
            into: into.clone(),
        },
        Op::CopyExactly { into, .. } => Op::CopyExactly {
            items,
            into: into.clone(),
        },
        Op::Duplicate(_) => Op::Duplicate(items),
        Op::MakeAlias(_) => Op::MakeAlias(items),
        Op::Trash(_) => Op::Trash(items),
        Op::DeleteImmediately(_) => Op::DeleteImmediately(items),
        Op::Compress(_)
        | Op::Rename { .. }
        | Op::NewFolder { .. }
        | Op::NewFile { .. }
        | Op::PutBack(_)
        | Op::Quick(_)
        | Op::Batch(_) => {
            unreachable!("only called for splittable ops")
        }
    }
}

fn moves_bytes(op: &Op, items: &[PathBuf]) -> bool {
    let device = |path: &Path| std::fs::symlink_metadata(path).map(|meta| meta.dev()).ok();
    match op {
        Op::Copy { .. } | Op::CopyExactly { .. } | Op::Duplicate(_) => true,
        Op::Move { into, .. } => items.iter().any(|item| device(item) != device(into)),
        _ => false,
    }
}

fn total_size(items: Vec<PathBuf>, budget: Duration) -> u64 {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let total: u64 = items.iter().map(|p| ops::walk_size(p)).sum();
        let _ = tx.send(total);
    });
    rx.recv_timeout(budget).unwrap_or(0)
}

struct Meter {
    start: Instant,
    done_bytes: u64,
    window: VecDeque<(Instant, u64)>,
}

impl Meter {
    fn new() -> Meter {
        Meter {
            start: Instant::now(),
            done_bytes: 0,
            window: VecDeque::new(),
        }
    }

    fn tick(&mut self, added: u64) -> f64 {
        self.done_bytes += added;
        let now = Instant::now();
        self.window.push_back((now, self.done_bytes));
        while let Some(&(t, _)) = self.window.front() {
            if now.duration_since(t) > Duration::from_secs(1) {
                self.window.pop_front();
            } else {
                break;
            }
        }
        let (t0, b0) = if self.window.len() >= 2 {
            *self.window.front().unwrap()
        } else {
            (self.start, 0)
        };
        let dt = now.duration_since(t0).as_secs_f64();
        if dt > 0.0 {
            (self.done_bytes - b0) as f64 / dt
        } else {
            0.0
        }
    }
}

#[derive(Clone, Debug)]
pub struct Undoable {
    pub label: String,
    pub undo: Op,
    pub redo: Op,
}

#[derive(Default)]
pub struct Journal {
    undo_stack: Vec<Undoable>,
    redo_stack: Vec<Undoable>,
}

impl Journal {
    pub fn new() -> Journal {
        Journal::default()
    }

    pub fn record(&mut self, done: &Done, original: &Op) {
        let Some(undo) = done.undo.clone() else {
            return;
        };
        self.undo_stack.push(Undoable {
            label: label_for(original),
            undo,
            redo: original.clone(),
        });
        self.redo_stack.clear();
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo_stack.last().map(|u| u.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo_stack.last().map(|u| u.label.as_str())
    }

    pub fn pop_undo(&mut self) -> Option<Undoable> {
        let undoable = self.undo_stack.pop()?;
        self.redo_stack.push(undoable.clone());
        Some(undoable)
    }

    pub fn pop_redo(&mut self) -> Option<Undoable> {
        let undoable = self.redo_stack.pop()?;
        self.undo_stack.push(undoable.clone());
        Some(undoable)
    }

    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }

    /// An undo that stopped partway stays undoable for what it did not get to.
    pub fn undo_left(&mut self, at: usize, left: Op) {
        if at < self.redo_stack.len() {
            let mut entry = self.redo_stack.remove(at);
            entry.undo = left;
            self.undo_stack.push(entry);
        }
    }

    /// A redo lands its items afresh, so undoing it again starts from where they are now.
    pub fn redone(&mut self, at: usize, done: &Done) {
        if let (Some(entry), Some(undo)) = (self.undo_stack.get_mut(at), done.undo.clone()) {
            entry.undo = undo;
        }
    }
}

fn label_for(op: &Op) -> String {
    match op {
        Op::Move { items, .. } => describe("Move", items),
        Op::Copy { items, .. } | Op::CopyExactly { items, .. } => describe("Copy", items),
        Op::Duplicate(items) => describe("Duplicate", items),
        Op::MakeAlias(items) => describe("Make Alias", items),
        Op::Compress(items) => describe("Compress", items),
        Op::Trash(items) => format!("{} to Trash", describe("Move", items)),
        Op::DeleteImmediately(items) => describe("Delete", items),
        Op::Rename { .. } => "Rename".to_string(),
        Op::NewFolder { .. } => "New Folder".to_string(),
        Op::NewFile { .. } => "New File".to_string(),
        Op::PutBack(items) => describe("Put Back", &put_back_paths(items)),
        Op::Quick(quick) => quick.label().to_string(),
        Op::Batch(ops) => ops.first().map(label_for).unwrap_or_default(),
    }
}

fn put_back_paths(items: &[ops::Restore]) -> Vec<PathBuf> {
    items.iter().map(|restore| restore.to.clone()).collect()
}

fn describe(verb: &str, items: &[PathBuf]) -> String {
    match items {
        [only] => {
            let name = only.file_name().unwrap_or_default().to_string_lossy();
            format!("{verb} of \u{201c}{name}\u{201d}")
        }
        _ => format!("{verb} of {} Items", items.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    /// An HFS+ RAM disk: an APFS-to-APFS copy is a `copyfile(3)` clone, not a byte copy.
    struct RamDisk {
        device: String,
        mount: PathBuf,
    }

    impl RamDisk {
        fn new(mb: u64, name: &str) -> Option<RamDisk> {
            let blocks = mb * 2048;
            let out = Command::new("hdiutil")
                .args(["attach", "-nomount", &format!("ram://{blocks}")])
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let device = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let ok = Command::new("diskutil")
                .args(["eraseVolume", "HFS+", name, &device])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                let _ = Command::new("hdiutil").args(["detach", &device]).status();
                return None;
            }
            Some(RamDisk {
                device,
                mount: PathBuf::from(format!("/Volumes/{name}")),
            })
        }
    }

    impl Drop for RamDisk {
        fn drop(&mut self) {
            let _ = Command::new("hdiutil")
                .args(["detach", &self.device, "-force"])
                .status();
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("finder-fs-queue-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn expect(
        rx: &mpsc::Receiver<Event>,
        timeout: Duration,
        mut matches: impl FnMut(&Event) -> bool,
    ) -> Event {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let event = rx
                .recv_timeout(remaining)
                .expect("expected event before timeout");
            if matches(&event) {
                return event;
            }
        }
    }

    fn expect_finished(rx: &mpsc::Receiver<Event>, id: JobId) -> Done {
        match expect(
            rx,
            Duration::from_secs(10),
            |e| matches!(e, Event::Finished(job, _) if *job == id),
        ) {
            Event::Finished(_, result) => result.unwrap(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn submits_and_finishes_jobs_in_order() {
        let dir = temp_dir("order");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let files: Vec<_> = (0..3)
            .map(|i| {
                let p = src.join(format!("f{i}.txt"));
                fs::write(&p, "x").unwrap();
                p
            })
            .collect();

        let (tx, rx) = mpsc::channel();
        let queue = Queue::new(move |e| {
            let _ = tx.send(e);
        });

        let ids: Vec<JobId> = files
            .iter()
            .map(|f| {
                queue.submit(
                    Op::Copy {
                        items: vec![f.clone()],
                        into: dst.clone(),
                    },
                    Conflict::Replace,
                )
            })
            .collect();

        for id in &ids {
            expect_finished(&rx, *id);
        }
        for file in &files {
            assert!(dst.join(file.file_name().unwrap()).exists());
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pause_blocks_the_next_job_until_resume() {
        let dir = temp_dir("pause");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let a = src.join("a.txt");
        let b = src.join("b.txt");
        fs::write(&a, "1").unwrap();
        fs::write(&b, "1").unwrap();

        let (tx, rx) = mpsc::channel();
        let queue = Queue::new(move |e| {
            let _ = tx.send(e);
        });

        let id1 = queue.submit(
            Op::Copy {
                items: vec![a.clone()],
                into: dst.clone(),
            },
            Conflict::Replace,
        );
        expect_finished(&rx, id1);

        queue.pause();
        expect(&rx, Duration::from_secs(1), |e| matches!(e, Event::Paused));

        let id2 = queue.submit(
            Op::Copy {
                items: vec![b.clone()],
                into: dst.clone(),
            },
            Conflict::Replace,
        );
        match rx.recv_timeout(Duration::from_millis(300)) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            other => panic!("expected no activity while paused, got {other:?}"),
        }
        assert!(!dst.join("b.txt").exists());

        queue.resume();
        expect(&rx, Duration::from_secs(1), |e| matches!(e, Event::Resumed));
        expect_finished(&rx, id2);
        assert!(dst.join("b.txt").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_skips_a_pending_job() {
        let dir = temp_dir("cancel");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let a = src.join("a.txt");
        let b = src.join("b.txt");
        fs::write(&a, "1").unwrap();
        fs::write(&b, "1").unwrap();

        let (tx, rx) = mpsc::channel();
        let queue = Queue::new(move |e| {
            let _ = tx.send(e);
        });
        queue.pause();
        expect(&rx, Duration::from_secs(1), |e| matches!(e, Event::Paused));

        let id1 = queue.submit(
            Op::Copy {
                items: vec![a.clone()],
                into: dst.clone(),
            },
            Conflict::Replace,
        );
        let id2 = queue.submit(
            Op::Copy {
                items: vec![b.clone()],
                into: dst.clone(),
            },
            Conflict::Replace,
        );
        queue.cancel(id2);
        expect(
            &rx,
            Duration::from_secs(1),
            |e| matches!(e, Event::Cancelled { job, .. } if *job == id2),
        );
        assert_eq!(queue.pending(), 1);

        queue.resume();
        expect(&rx, Duration::from_secs(1), |e| matches!(e, Event::Resumed));
        expect_finished(&rx, id1);

        assert!(dst.join("a.txt").exists());
        assert!(!dst.join("b.txt").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancelled_copy_keeps_what_landed() {
        let dir = temp_dir("cancel-landed");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let files: Vec<PathBuf> = (0..200)
            .map(|i| {
                let path = src.join(format!("f{i:03}.txt"));
                fs::write(&path, "x").unwrap();
                path
            })
            .collect();

        let (tx, rx) = mpsc::channel();
        let queue = Queue::new(move |e| {
            let _ = tx.send(e);
        });
        let id = queue.submit(
            Op::Copy {
                items: files.clone(),
                into: dst.clone(),
            },
            Conflict::Replace,
        );

        expect(
            &rx,
            Duration::from_secs(10),
            |e| matches!(e, Event::Progress { job, .. } if *job == id),
        );
        queue.cancel(id);
        let event = expect(
            &rx,
            Duration::from_secs(10),
            |e| matches!(e, Event::Cancelled { job, .. } if *job == id),
        );
        let Event::Cancelled { done, .. } = event else {
            unreachable!()
        };
        assert!(
            !done.results.is_empty(),
            "the items that landed before the cancel are still results"
        );
        for landed in &done.results {
            assert!(landed.exists() && landed.starts_with(&dst));
        }
        let undo = done.undo.expect("what landed is still undoable");
        let back = ops::perform(undo, Conflict::Replace, |_| {}).unwrap();
        assert_eq!(fs::read_dir(&dst).unwrap().count(), 0);

        for trashed in back.results {
            let _ = fs::remove_file(trashed);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_batch_that_fails_keeps_what_landed() {
        let dir = temp_dir("partial");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let good = src.join("good.txt");
        fs::write(&good, "landed").unwrap();
        let missing = src.join("missing.txt");

        let (tx, rx) = mpsc::channel();
        let queue = Queue::new(move |e| {
            let _ = tx.send(e);
        });
        let id = queue.submit(
            Op::Copy {
                items: vec![good.clone(), missing.clone()],
                into: dst.clone(),
            },
            Conflict::Replace,
        );

        let event = expect(
            &rx,
            Duration::from_secs(10),
            |e| matches!(e, Event::Failed { job, .. } if *job == id),
        );
        let Event::Failed { done, .. } = event else {
            unreachable!()
        };
        assert_eq!(
            done.results,
            vec![dst.join("good.txt")],
            "the item that landed is still a result"
        );
        let undo = done.undo.expect("what landed is still undoable");
        let back = ops::perform(undo, Conflict::Replace, |_| {}).unwrap();
        assert!(!dst.join("good.txt").exists());

        for trashed in back.results {
            let _ = fs::remove_file(trashed);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_redo_is_undone_from_where_it_landed() {
        let mut journal = Journal::new();
        let back_from = |from: &str| Op::Move {
            items: vec![PathBuf::from(from)],
            into: PathBuf::from("/A"),
        };
        let first = Done {
            results: vec![PathBuf::from("/Trash/x.pdf")],
            undo: Some(back_from("/Trash/x.pdf")),
        };
        journal.record(&first, &Op::Trash(vec![PathBuf::from("/A/x.pdf")]));
        journal.pop_undo().unwrap();
        journal.pop_redo().unwrap();
        let again = Done {
            results: vec![PathBuf::from("/Trash/x 2.pdf")],
            undo: Some(back_from("/Trash/x 2.pdf")),
        };
        journal.redone(journal.undo_len() - 1, &again);

        let undo = journal.pop_undo().unwrap().undo;
        assert!(
            matches!(undo, Op::Move { ref items, .. } if items == &[PathBuf::from("/Trash/x 2.pdf")]),
            "the undo still names the first run's path"
        );
    }

    #[test]
    fn journal_undo_round_trips_a_move() {
        let dir = temp_dir("journal-undo");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let file = src.join("a.txt");
        fs::write(&file, "hi").unwrap();

        let op = Op::Move {
            items: vec![file.clone()],
            into: dst.clone(),
        };
        let done = ops::perform(op.clone(), Conflict::Replace, |_| {}).unwrap();
        let mut journal = Journal::new();
        journal.record(&done, &op);
        let undoable = journal.pop_undo().unwrap();
        let back = ops::perform(undoable.undo, Conflict::Replace, |_| {}).unwrap();
        assert_eq!(back.results, vec![file.clone()]);
        assert!(file.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn throughput_is_within_30_percent_of_wall_clock() {
        let Some(ram) = RamDisk::new(320, &format!("QueueThroughput{}", std::process::id())) else {
            eprintln!("skipping: could not create a RAM disk for a non-cloned copy");
            return;
        };
        let dir = temp_dir("throughput");
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        let dst = ram.mount.clone();
        let file = src.join("big.bin");
        fs::write(&file, vec![0u8; 200 * 1024 * 1024]).unwrap();

        let (tx, rx) = mpsc::channel();
        let queue = Queue::new(move |e| {
            let _ = tx.send(e);
        });
        let start = Instant::now();
        let id = queue.submit(
            Op::Copy {
                items: vec![file.clone()],
                into: dst.clone(),
            },
            Conflict::Replace,
        );

        let mut measured = None;
        loop {
            match rx.recv_timeout(Duration::from_secs(30)).expect("event") {
                Event::Progress {
                    job, bytes_per_sec, ..
                } if job == id => measured = Some(bytes_per_sec),
                Event::Finished(job, result) if job == id => {
                    result.unwrap();
                    break;
                }
                _ => {}
            }
        }
        let elapsed = start.elapsed().as_secs_f64();
        let wall_rate = (200.0 * 1024.0 * 1024.0) / elapsed;
        let measured = measured.expect("a progress event carried a rate");
        let ratio = measured / wall_rate;
        assert!(
            (0.7..1.3).contains(&ratio),
            "measured {measured} B/s vs wall clock {wall_rate} B/s (ratio {ratio})"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dropped_queue_lets_its_worker_go() {
        let queue = Queue::new(|_| {});
        let shared = Arc::downgrade(&queue.shared);
        drop(queue);
        let deadline = Instant::now() + Duration::from_secs(2);
        while shared.upgrade().is_some() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            shared.upgrade().is_none(),
            "the worker thread still holds the queue"
        );
    }

    #[test]
    fn an_undo_that_stops_partway_hands_back_what_is_left() {
        let dir = temp_dir("undo-left");
        let item = dir.join("item.txt");
        fs::write(&item, "item").unwrap();
        let dest = dir.join("back").join("item.txt");
        let missing = ops::Restore {
            item: dir.join("missing.txt"),
            to: dir.join("missing.txt"),
        };
        let real = ops::Restore {
            item: item.clone(),
            to: dest.clone(),
        };
        let (tx, rx) = mpsc::channel();
        let queue = Queue::new(move |event| {
            let _ = tx.send(event);
        });
        let id = queue.submit(Op::PutBack(vec![missing, real]), Conflict::KeepBoth);
        match expect(
            &rx,
            Duration::from_secs(10),
            |e| matches!(e, Event::Failed { job, .. } if *job == id),
        ) {
            Event::Failed { done, left, .. } => {
                assert_eq!(done.results, vec![dest.clone()]);
                assert!(matches!(left, Some(Op::PutBack(left)) if left.len() == 1));
            }
            _ => unreachable!(),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_undo_that_stops_partway_stays_undoable() {
        let mut journal = Journal::new();
        let original = Op::Trash(vec![PathBuf::from("/tmp/finder-fs-journal-a")]);
        let done = Done {
            results: Vec::new(),
            undo: Some(Op::PutBack(Vec::new())),
        };
        journal.record(&done, &original);
        journal.pop_undo();
        journal.undo_left(journal.redo_len() - 1, Op::PutBack(Vec::new()));
        assert!(
            journal.undo_label().is_some(),
            "what is left can be undone again"
        );
        assert!(
            journal.redo_label().is_none(),
            "a half-undone entry is not redone"
        );
    }
}
