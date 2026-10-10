use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use finder_fs::ops::transfer::{End, OnError, Progress, Transfer};
use finder_fs::ops::{self, Conflict, Done, Op, Unfinished};
use serde_json::{Value, json};

use crate::journal::Step;
use crate::out::{self, Out};
use crate::{CANCELLED, OK, PARTIAL, REFUSED, signals};

const TICK: Duration = Duration::from_millis(250);
const POLL: Duration = Duration::from_millis(10);
const POLL_ARMED: Duration = Duration::from_millis(1);

pub enum Limit {
    Bytes(u64),
    Items(u64),
}

pub struct Plan {
    pub conflict: Conflict,
    pub on_error: OnError,
    pub pause_after: Option<u64>,
    pub cancel_after: Option<Limit>,
}

pub struct Ran {
    pub done: Done,
    pub failures: Vec<Failed>,
    pub end: End,
    pub left: Option<Op>,
}

pub struct Failed {
    pub source: Option<PathBuf>,
    pub dest: Option<PathBuf>,
    pub error: String,
}

impl Ran {
    pub fn code(&self) -> u8 {
        let nothing_done = self.done.results.is_empty() && self.done.undo.is_none();
        match (self.end, self.failures.is_empty()) {
            (End::Cancelled, _) => CANCELLED,
            (End::Finished, true) => OK,
            _ if nothing_done => REFUSED,
            _ => PARTIAL,
        }
    }

    pub fn json(&self, command: &str) -> Value {
        json!({
            "event": "result",
            "command": command,
            "end": match self.end {
                End::Finished => "finished",
                End::Cancelled => "cancelled",
                End::Stopped => "stopped",
            },
            "exit": self.code(),
            "results": self.done.results.iter().map(|path| out::path(path)).collect::<Vec<_>>(),
            "failures": self.failures.iter().map(|failed| json!({
                "source": failed.source.as_deref().map(out::path),
                "dest": failed.dest.as_deref().map(out::path),
                "error": failed.error,
            })).collect::<Vec<_>>(),
            "undo": self.done.undo.as_ref().map(Step::from),
            "left": self.left.as_ref().map(Step::from),
        })
    }

    pub fn report(&self, command: &str, out: &mut Out) {
        if out.json {
            return out.emit(self.json(command));
        }
        for path in &self.done.results {
            out.say(&path.to_string_lossy());
        }
        for failed in &self.failures {
            out.warn(&failed.error);
        }
        match self.end {
            End::Cancelled => out.warn("cancelled"),
            End::Stopped if self.failures.is_empty() => out.warn("stopped"),
            End::Stopped | End::Finished => {}
        }
    }
}

#[derive(Default)]
struct Steps {
    done: AtomicU64,
    total: AtomicU64,
    cancelled: AtomicBool,
}

/// Runs `op` the way Found'er's job queue does, on a thread of its own, reporting as it goes.
pub fn run(op: Op, plan: &Plan, verb: &str, out: &mut Out) -> Ran {
    signals::install();
    let transfer = Transfer::default();
    let steps = Arc::new(Steps::default());
    let worker = {
        let (transfer, steps) = (transfer.clone(), steps.clone());
        let (conflict, on_error) = (plan.conflict, plan.on_error);
        thread::spawn(move || execute(op, conflict, on_error, &transfer, &steps))
    };
    let mut hooks = Hooks {
        pause_after: plan.pause_after,
        cancel_after: plan.cancel_after.as_ref(),
    };
    let started = Instant::now();
    let mut shown: Option<Instant> = None;
    while !worker.is_finished() {
        if signals::interrupted() {
            transfer.cancel();
            steps.cancelled.store(true, Ordering::SeqCst);
        }
        if signals::take_pause() {
            transfer.pause();
        }
        if signals::take_resume() {
            transfer.resume();
            if out.json {
                out.emit(json!({ "event": "resumed" }));
            }
        }
        let progress = progress_of(&transfer, &steps);
        hooks.check(&progress, &transfer, &steps, out);
        if shown.is_none_or(|at| at.elapsed() >= TICK) {
            out.progress(verb, &progress, started.elapsed());
            shown = Some(Instant::now());
        }
        thread::sleep(match hooks.armed() {
            true => POLL_ARMED,
            false => POLL,
        });
    }
    out.clear();
    let ran = worker.join().unwrap_or_else(|_| Ran {
        done: Done::default(),
        failures: vec![Failed {
            source: None,
            dest: None,
            error: "the operation panicked".into(),
        }],
        end: End::Stopped,
        left: None,
    });
    settle_trash(&ran.done);
    ran
}

/// The Trash writes down where an item came from a moment after it arrives, on a queue of its
/// own: a process that quits at once loses the note, and Put Back with it.
fn settle_trash(done: &Done) {
    let mut trashed = Vec::new();
    in_trash(done.undo.as_ref(), &mut trashed);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !trashed.is_empty() && Instant::now() < deadline {
        if finder_fs::put_back::origins(&trashed)
            .iter()
            .all(Option::is_some)
        {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn in_trash(op: Option<&Op>, found: &mut Vec<PathBuf>) {
    let trash = |item: &PathBuf| {
        let folder = item.parent();
        folder
            .and_then(Path::file_name)
            .is_some_and(|name| name == ".Trash")
            || (folder.and_then(Path::parent).and_then(Path::file_name))
                .is_some_and(|name| name == ".Trashes")
    };
    match op {
        Some(Op::PutBack(items)) => {
            found.extend(items.iter().map(|back| back.item.clone()).filter(trash));
        }
        Some(Op::Batch(ops)) => ops.iter().for_each(|op| in_trash(Some(op), found)),
        _ => {}
    }
}

fn progress_of(transfer: &Transfer, steps: &Steps) -> Progress {
    let mut progress = transfer.progress();
    let total = steps.total.load(Ordering::Relaxed);
    if total > 0 {
        progress.counting = false;
        progress.items_total = total;
        progress.items_done = steps.done.load(Ordering::Relaxed);
    }
    progress
}

struct Hooks<'a> {
    pause_after: Option<u64>,
    cancel_after: Option<&'a Limit>,
}

impl Hooks<'_> {
    fn armed(&self) -> bool {
        self.pause_after.is_some() || self.cancel_after.is_some()
    }

    fn check(&mut self, progress: &Progress, transfer: &Transfer, steps: &Steps, out: &mut Out) {
        if self
            .pause_after
            .is_some_and(|after| progress.bytes_done >= after)
        {
            self.pause_after = None;
            transfer.pause();
            let pid = std::process::id();
            match out.json {
                true => out.emit(json!({
                    "event": "paused",
                    "pid": pid,
                    "bytes_done": progress.bytes_done,
                    "items_done": progress.items_done,
                })),
                false => out.warn(&format!(
                    "paused after {}; `kill -USR1 {pid}` resumes",
                    out::size(progress.bytes_done)
                )),
            }
        }
        let reached = self.cancel_after.is_some_and(|limit| match limit {
            Limit::Bytes(bytes) => progress.bytes_done >= *bytes,
            Limit::Items(items) => progress.items_done >= *items,
        });
        if reached {
            self.cancel_after = None;
            transfer.cancel();
            steps.cancelled.store(true, Ordering::SeqCst);
            if out.json {
                out.emit(json!({
                    "event": "cancelling",
                    "bytes_done": progress.bytes_done,
                    "items_done": progress.items_done,
                }));
            }
        }
    }
}

fn execute(
    op: Op,
    conflict: Conflict,
    on_error: OnError,
    transfer: &Transfer,
    steps: &Steps,
) -> Ran {
    match ops::carry(op, conflict, on_error, transfer) {
        Ok(carried) => Ran {
            failures: (carried.failures.into_iter())
                .map(|failure| Failed {
                    source: Some(failure.source),
                    dest: Some(failure.dest),
                    error: failure.error.to_string(),
                })
                .collect(),
            done: carried.done,
            end: carried.end,
            left: carried.left,
        },
        Err(op) => in_steps(op, conflict, on_error, steps),
    }
}

/// A trash, delete or alias of several items goes one item at a time, as the app's queue runs it.
fn in_steps(op: Op, conflict: Conflict, on_error: OnError, steps: &Steps) -> Ran {
    let (items, each): (Vec<PathBuf>, fn(Vec<PathBuf>) -> Op) = match op {
        Op::Trash(items) => (items, Op::Trash),
        Op::DeleteImmediately(items) => (items, Op::DeleteImmediately),
        Op::MakeAlias(items) => (items, Op::MakeAlias),
        op => return once(op, conflict),
    };
    steps.total.store(items.len() as u64, Ordering::Relaxed);
    let (mut results, mut undos) = (Vec::new(), Vec::new());
    let (mut failures, mut left) = (Vec::new(), Vec::new());
    let mut items = items.into_iter();
    let mut end = End::Finished;
    for item in items.by_ref() {
        if steps.cancelled.load(Ordering::SeqCst) {
            end = End::Cancelled;
            break;
        }
        let ran = once(each(vec![item.clone()]), conflict);
        results.extend(ran.done.results);
        undos.extend(ran.done.undo);
        steps.done.fetch_add(1, Ordering::Relaxed);
        if ran.failures.is_empty() {
            continue;
        }
        failures.extend(ran.failures.into_iter().map(|failed| Failed {
            source: failed.source.or_else(|| Some(item.clone())),
            ..failed
        }));
        left.push(item);
        if on_error == OnError::Stop {
            end = End::Stopped;
            break;
        }
    }
    if end != End::Cancelled {
        left.extend(items);
    }
    Ran {
        done: Done {
            results,
            undo: match undos.len() {
                0 | 1 => undos.pop(),
                _ => Some(Op::Batch(undos)),
            },
        },
        failures,
        end,
        left: (!left.is_empty()).then(|| each(left)),
    }
}

/// A plain error leaves the whole op to do again, as the app keeps a failed undo whole.
fn once(op: Op, conflict: Conflict) -> Ran {
    match ops::perform(op.clone(), conflict).map_err(Unfinished::from_error) {
        Ok(done) => Ran {
            done,
            failures: Vec::new(),
            end: End::Finished,
            left: None,
        },
        Err(Ok(unfinished)) => Ran {
            done: unfinished.done,
            failures: vec![Failed {
                source: None,
                dest: None,
                error: unfinished.cause.to_string(),
            }],
            end: End::Stopped,
            left: Some(unfinished.left),
        },
        Err(Err(error)) => Ran {
            done: Done::default(),
            failures: vec![Failed {
                source: None,
                dest: None,
                error: error.to_string(),
            }],
            end: End::Stopped,
            left: Some(op),
        },
    }
}
