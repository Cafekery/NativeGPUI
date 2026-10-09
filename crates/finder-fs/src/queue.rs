use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use parking_lot::Mutex;

use crate::ops::transfer::{Kind, OnError, Progress, Transfer};
use crate::ops::{self, Carried, Conflict, Done, Op, Unfinished};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobId(u64);

impl JobId {
    pub fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Debug)]
pub enum Event {
    Finished(JobId, io::Result<Done>),
    Failed {
        job: JobId,
        done: Done,
        error: io::Error,
        left: Option<Op>,
    },
    Cancelled {
        job: JobId,
        done: Done,
    },
    Carried(JobId, Carried),
}

struct Shared {
    running: Mutex<HashMap<JobId, Transfer>>,
    next_id: AtomicU64,
    on_event: Mutex<Box<dyn Fn(Event) + Send>>,
}

/// Every job starts at once on a thread of its own, and is paused, cancelled and read by its id.
#[derive(Clone)]
pub struct Queue {
    shared: Arc<Shared>,
}

impl Queue {
    pub fn new(on_event: impl Fn(Event) + Send + 'static) -> Queue {
        Queue {
            shared: Arc::new(Shared {
                running: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(1),
                on_event: Mutex::new(Box::new(on_event)),
            }),
        }
    }

    pub fn submit(&self, op: Op, conflict: Conflict) -> JobId {
        let id = JobId(self.shared.next_id.fetch_add(1, Ordering::Relaxed));
        let transfer = Transfer::default();
        self.shared.running.lock().insert(id, transfer.clone());
        let shared = self.shared.clone();
        thread::spawn(move || {
            let event = run(id, op, conflict, &transfer);
            shared.running.lock().remove(&id);
            (shared.on_event.lock())(event);
        });
        id
    }

    pub fn progress(&self, job: JobId) -> Option<Progress> {
        self.shared.running.lock().get(&job).map(Transfer::progress)
    }

    pub fn pause(&self, job: JobId) {
        if let Some(transfer) = self.transfer(job) {
            transfer.pause();
        }
    }

    pub fn resume(&self, job: JobId) {
        if let Some(transfer) = self.transfer(job) {
            transfer.resume();
        }
    }

    pub fn cancel(&self, job: JobId) {
        if let Some(transfer) = self.transfer(job) {
            transfer.cancel();
        }
    }

    pub fn pending(&self) -> usize {
        self.shared.running.lock().len()
    }

    fn transfer(&self, job: JobId) -> Option<Transfer> {
        self.shared.running.lock().get(&job).cloned()
    }
}

fn run(id: JobId, op: Op, conflict: Conflict, transfer: &Transfer) -> Event {
    let mut op = match ops::carry(op, conflict, OnError::Continue, transfer) {
        Ok(carried) => return Event::Carried(id, carried),
        Err(op) => op,
    };
    let Some(items) = items_mut(&mut op).map(std::mem::take) else {
        return match ops::perform(op, conflict).map_err(Unfinished::from_error) {
            Ok(done) => Event::Finished(id, Ok(done)),
            Err(Ok(unfinished)) => Event::Failed {
                job: id,
                done: unfinished.done,
                error: unfinished.cause,
                left: Some(unfinished.left),
            },
            Err(Err(error)) => Event::Finished(id, Err(error)),
        };
    };
    transfer.begin_steps(items.len() as u64);
    let mut results = Vec::with_capacity(items.len());
    let mut undos = Vec::with_capacity(items.len());
    for (at, item) in items.iter().enumerate() {
        if !transfer.next_step() {
            return Event::Cancelled {
                job: id,
                done: batch_done(results, undos),
            };
        }
        match ops::perform(with_items(&op, vec![item.clone()]), conflict) {
            Ok(done) => {
                results.extend(done.results);
                undos.extend(done.undo);
                transfer.step_done();
            }
            Err(error) => {
                let error = match Unfinished::from_error(error) {
                    Ok(unfinished) => {
                        results.extend(unfinished.done.results);
                        undos.extend(unfinished.done.undo);
                        unfinished.cause
                    }
                    Err(error) => error,
                };
                return Event::Failed {
                    job: id,
                    done: batch_done(results, undos),
                    error,
                    left: Some(with_items(&op, items[at..].to_vec())),
                };
            }
        }
    }
    Event::Finished(id, Ok(batch_done(results, undos)))
}

fn batch_done(results: Vec<PathBuf>, mut undos: Vec<Op>) -> Done {
    let undo = match undos.len() {
        0 => None,
        1 => undos.pop(),
        _ => Some(Op::Batch(undos)),
    };
    Done { results, undo }
}

fn items_mut(op: &mut Op) -> Option<&mut Vec<PathBuf>> {
    match op {
        Op::Trash(items) | Op::DeleteImmediately(items) | Op::MakeAlias(items) => Some(items),
        // One archive comes out of the whole selection, so it is never split into item jobs.
        Op::Compress(_)
        | Op::Rename { .. }
        | Op::NewFolder { .. }
        | Op::NewFile { .. }
        | Op::Quick(_)
        | Op::Batch(_)
        | Op::Move { .. }
        | Op::Copy { .. }
        | Op::CopyExactly { .. }
        | Op::Duplicate(_)
        | Op::PutBack(_)
        | Op::Retry(_) => None,
    }
}

fn with_items(op: &Op, items: Vec<PathBuf>) -> Op {
    let mut op = op.clone();
    if let Some(slot) = items_mut(&mut op) {
        *slot = items;
    }
    op
}

#[derive(Clone, Debug)]
pub struct Undoable {
    pub label: String,
    pub undo: Op,
    pub redo: Op,
    serial: u64,
}

#[derive(Default)]
pub struct Journal {
    undo_stack: Vec<Undoable>,
    redo_stack: Vec<Undoable>,
    recorded: u64,
}

impl Journal {
    pub fn new() -> Journal {
        Journal::default()
    }

    /// Names the entry, for a retry of the same job to fold into.
    pub fn record(&mut self, done: &Done, original: &Op) -> Option<u64> {
        let undo = done.undo.clone()?;
        self.recorded += 1;
        self.undo_stack.push(Undoable {
            label: label_for(original),
            undo,
            redo: original.clone(),
            serial: self.recorded,
        });
        self.redo_stack.clear();
        Some(self.recorded)
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

    pub fn redone(&mut self, at: usize, done: &Done) -> Option<u64> {
        let entry = self.undo_stack.get_mut(at)?;
        if let Some(undo) = done.undo.clone() {
            entry.undo = undo;
        }
        Some(entry.serial)
    }

    /// Folds a retry's undo into the entry of the job it finished; false once that entry is gone.
    pub fn retried(&mut self, serial: u64, done: &Done) -> bool {
        let Some(entry) = self
            .undo_stack
            .iter_mut()
            .find(|entry| entry.serial == serial)
        else {
            return false;
        };
        if let Some(undo) = done.undo.clone() {
            let earlier = std::mem::replace(&mut entry.undo, Op::Batch(Vec::new()));
            let mut steps = Vec::new();
            for step in [undo, earlier] {
                match step {
                    Op::Batch(ops) => steps.extend(ops),
                    op => steps.push(op),
                }
            }
            entry.undo = Op::Batch(steps);
        }
        self.redo_stack.clear();
        true
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
        Op::PutBack(items) => {
            let back: Vec<&Path> = items.iter().map(|restore| restore.to.as_path()).collect();
            describe("Put Back", &back)
        }
        Op::Retry(request) => {
            let verb = match request.kind() {
                Kind::Move => "Move",
                Kind::Copy | Kind::CopyExactly => "Copy",
            };
            let sources: Vec<&Path> = request.items().map(|(source, _)| source).collect();
            describe(verb, &sources)
        }
        Op::Quick(quick) => quick.label().to_string(),
        Op::Batch(ops) => ops.first().map(label_for).unwrap_or_default(),
    }
}

fn describe(verb: &str, items: &[impl AsRef<Path>]) -> String {
    match items {
        [only] => {
            let name = only.as_ref().file_name().unwrap_or_default();
            format!("{verb} of \u{201c}{}\u{201d}", name.to_string_lossy())
        }
        _ => format!("{verb} of {} Items", items.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::transfer::End;
    use crate::ops::transfer::tests::RamDisk;
    use std::fs;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("finder-fs-queue-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn listen() -> (Queue, mpsc::Receiver<Event>) {
        let (tx, rx) = mpsc::channel();
        let queue = Queue::new(move |event| {
            let _ = tx.send(event);
        });
        (queue, rx)
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

    fn carried(rx: &mpsc::Receiver<Event>, id: JobId) -> Carried {
        match expect(
            rx,
            Duration::from_secs(20),
            |e| matches!(e, Event::Carried(job, _) if *job == id),
        ) {
            Event::Carried(_, carried) => carried,
            _ => unreachable!(),
        }
    }

    #[test]
    fn every_job_ends_with_its_own_event() {
        let dir = temp_dir("each");
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

        let (queue, rx) = listen();
        let mut running: Vec<JobId> = files
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

        while !running.is_empty() {
            let event = rx
                .recv_timeout(Duration::from_secs(20))
                .expect("every job ends");
            let Event::Carried(job, carried) = event else {
                panic!("a copy ends carried: {event:?}");
            };
            assert!(carried.failures.is_empty(), "{:?}", carried.failures);
            assert_eq!(carried.end, End::Finished);
            running.retain(|id| *id != job);
        }
        for file in &files {
            assert!(dst.join(file.file_name().unwrap()).exists());
        }
        assert_eq!(queue.pending(), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_held_job_lets_the_next_one_run_and_cancels_whole() {
        let Some(ram) = RamDisk::new(300, "HFS+", &format!("QueueHeld{}", std::process::id()))
        else {
            return;
        };
        let dir = temp_dir("held");
        let big = dir.join("big.bin");
        fs::File::create(&big).unwrap().set_len(200 << 20).unwrap();
        let small = dir.join("small.txt");
        fs::write(&small, "small").unwrap();

        let (queue, rx) = listen();
        let held = queue.submit(
            Op::Copy {
                items: vec![big.clone()],
                into: ram.mount.clone(),
            },
            Conflict::KeepBoth,
        );
        queue.pause(held);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let progress = queue.progress(held).expect("a held job is still running");
            if progress.paused && progress.bytes_done > 0 {
                break;
            }
            assert!(Instant::now() < deadline, "no file started: {progress:?}");
            thread::sleep(Duration::from_millis(2));
        }

        let next = queue.submit(
            Op::Copy {
                items: vec![small.clone()],
                into: ram.mount.clone(),
            },
            Conflict::KeepBoth,
        );
        let ran = carried(&rx, next);
        assert_eq!(
            ran.done.results,
            vec![ram.mount.join("small.txt")],
            "the next job waited for the held one"
        );
        assert!(queue.progress(held).is_some_and(|progress| progress.paused));

        queue.cancel(held);
        let cancelled = carried(&rx, held);
        assert_eq!(cancelled.end, End::Cancelled);
        assert!(cancelled.done.results.is_empty());
        assert!(
            queue.progress(held).is_none(),
            "an ended job reports nothing"
        );
        let left: Vec<_> = fs::read_dir(&ram.mount)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("big") || name.starts_with(".finder-fs-"))
            .collect();
        assert!(left.is_empty(), "a cancelled copy left {left:?}");
        assert_eq!(fs::metadata(&big).unwrap().len(), 200 << 20);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancel_lands_between_items() {
        let dir = temp_dir("steps");
        let items: Vec<PathBuf> = (0..3)
            .map(|i| {
                let path = dir.join(format!("f{i}.txt"));
                fs::write(&path, "x").unwrap();
                path
            })
            .collect();

        let (queue, rx) = listen();
        let id = queue.submit(Op::MakeAlias(items.clone()), Conflict::KeepBoth);
        queue.pause(id);
        queue.cancel(id);
        let Event::Cancelled { done, .. } = expect(
            &rx,
            Duration::from_secs(10),
            |e| matches!(e, Event::Cancelled { job, .. } if *job == id),
        ) else {
            unreachable!()
        };
        assert!(
            done.results.len() < items.len(),
            "every item ran past the cancel"
        );
        for alias in &done.results {
            assert!(alias.exists(), "a result that never landed: {alias:?}");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_copy_that_fails_keeps_what_landed() {
        if !crate::trash_reachable() {
            return;
        }
        let dir = temp_dir("partial");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let good = src.join("good.txt");
        fs::write(&good, "landed").unwrap();
        let missing = src.join("missing.txt");

        let (queue, rx) = listen();
        let id = queue.submit(
            Op::Copy {
                items: vec![missing.clone(), good.clone()],
                into: dst.clone(),
            },
            Conflict::Replace,
        );

        let carried = carried(&rx, id);
        assert_eq!(carried.failures.len(), 1);
        assert_eq!(carried.failures[0].source, missing);
        assert_eq!(
            carried.done.results,
            vec![dst.join("good.txt")],
            "the item after the one that failed still landed"
        );
        let undo = carried.done.undo.expect("what landed is still undoable");
        let back = ops::perform(undo, Conflict::Replace).unwrap();
        assert!(!dst.join("good.txt").exists());

        crate::purge(&back.results);
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
    fn a_retried_move_undoes_and_redoes_as_one() {
        use std::os::unix::fs::PermissionsExt;
        let Some(ram) = RamDisk::new(64, "HFS+", &format!("QueueRetried{}", std::process::id()))
        else {
            return;
        };
        let src = temp_dir("retried");
        let tree = src.join("tree");
        let solo = src.join("solo.txt");
        let names = [
            "tree/a/one.txt",
            "tree/a/two.txt",
            "tree/b/three.txt",
            "tree/b/locked.txt",
            "solo.txt",
        ];
        for name in names {
            let file = src.join(name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, name).unwrap();
        }
        let lock = |mode| {
            for locked in [tree.join("b/locked.txt"), solo.clone()] {
                fs::set_permissions(locked, fs::Permissions::from_mode(mode)).unwrap();
            }
        };
        lock(0o000);

        let (queue, rx) = listen();
        let run = |op: Op, conflict| carried(&rx, queue.submit(op, conflict));
        let mut journal = Journal::new();
        let original = Op::Move {
            items: vec![tree.clone(), solo.clone()],
            into: ram.mount.clone(),
        };
        let first = run(original.clone(), Conflict::KeepBoth);
        assert_eq!(first.failures.len(), 2, "{:?}", first.failures);
        let entry = journal
            .record(&first.done, &original)
            .expect("what moved can be undone");
        lock(0o644);
        let retried = run(first.left.expect("the locked files wait"), Conflict::Merge);
        assert!(retried.failures.is_empty(), "{:?}", retried.failures);
        assert!(journal.retried(entry, &retried.done));
        assert_eq!(journal.undo_len(), 1, "the retry folds into the move");
        assert!(
            !tree.exists() && !solo.exists(),
            "the retry finished the move"
        );

        let undo = journal.pop_undo().unwrap().undo;
        let back = run(undo, Conflict::KeepBoth);
        assert!(back.failures.is_empty(), "{:?}", back.failures);
        for name in names {
            assert_eq!(fs::read_to_string(src.join(name)).unwrap(), name);
        }
        for name in ["tree", "solo.txt"] {
            let landed = ram.mount.join(name);
            assert!(!landed.exists(), "one undo left {landed:?}");
        }
        assert!(!src.join("tree 2").exists());

        let redo = journal.pop_redo().unwrap().redo;
        let again = run(redo, Conflict::KeepBoth);
        assert!(again.failures.is_empty(), "{:?}", again.failures);
        journal.redone(journal.undo_len() - 1, &again.done);
        for name in names {
            assert!(!src.join(name).exists(), "{name} stayed");
            assert!(ram.mount.join(name).exists(), "{name} did not land");
        }
        assert_eq!((journal.undo_len(), journal.redo_len()), (1, 0));

        let _ = fs::remove_dir_all(&src);
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
        let done = ops::perform(op.clone(), Conflict::Replace).unwrap();
        let mut journal = Journal::new();
        journal.record(&done, &op);
        let undoable = journal.pop_undo().unwrap();
        let back = ops::perform(undoable.undo, Conflict::Replace).unwrap();
        assert_eq!(back.results, vec![file.clone()]);
        assert!(file.exists());

        let _ = fs::remove_dir_all(&dir);
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
        let (queue, rx) = listen();
        let id = queue.submit(Op::PutBack(vec![missing, real]), Conflict::KeepBoth);
        let carried = carried(&rx, id);
        assert_eq!(carried.done.results, vec![dest.clone()]);
        assert!(matches!(carried.left, Some(Op::PutBack(left)) if left.len() == 1));
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
