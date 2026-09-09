//! An FSEvents stream is immutable, so `watch`/`unwatch` recreate it from the last event id seen.

use std::collections::HashSet;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use objc2_core_foundation::{CFArray, CFRetained, CFRunLoop, CFString, kCFRunLoopDefaultMode};
#[allow(deprecated)]
use objc2_core_services::FSEventStreamScheduleWithRunLoop;
use objc2_core_services::{
    ConstFSEventStreamRef, FSEventStreamContext, FSEventStreamCreate, FSEventStreamEventFlags,
    FSEventStreamEventId, FSEventStreamGetLatestEventId, FSEventStreamInvalidate,
    FSEventStreamRelease, FSEventStreamStart, FSEventStreamStop, FSEventsGetCurrentEventId,
    kFSEventStreamCreateFlagUseCFTypes, kFSEventStreamEventFlagEventIdsWrapped,
    kFSEventStreamEventFlagHistoryDone, kFSEventStreamEventFlagKernelDropped,
    kFSEventStreamEventFlagMount, kFSEventStreamEventFlagMustScanSubDirs,
    kFSEventStreamEventFlagRootChanged, kFSEventStreamEventFlagUnmount,
    kFSEventStreamEventFlagUserDropped, kFSEventStreamEventIdSinceNow,
};

#[derive(Clone, Debug, Default)]
pub struct Replay {
    pub dirs: Vec<PathBuf>,
    /// Subtrees to read again whole: the log dropped events, or a volume came or went.
    pub rescan: Vec<PathBuf>,
    pub event_id: u64,
    pub history_done: bool,
    /// FSEvents ids restarted: no saved id means anything any more.
    pub wrapped: bool,
}

/// The events that say "I cannot tell you what changed under here, look again yourself".
const RESCAN: FSEventStreamEventFlags = kFSEventStreamEventFlagMustScanSubDirs
    | kFSEventStreamEventFlagUserDropped
    | kFSEventStreamEventFlagKernelDropped
    | kFSEventStreamEventFlagRootChanged
    | kFSEventStreamEventFlagMount
    | kFSEventStreamEventFlagUnmount;

struct Shared {
    paths: Mutex<HashSet<PathBuf>>,
    changed: Condvar,
    since: AtomicU64,
    shutdown: AtomicBool,
    /// Set while `CFRunLoopRun` is blocking; `None` while idle so a stray wake-up can't poison
    /// the next run.
    run_loop: Mutex<Option<usize>>,
}

impl Shared {
    fn wake(&self) {
        self.changed.notify_all();
        if let Some(addr) = *self.run_loop.lock().unwrap() {
            unsafe { (&*(addr as *const CFRunLoop)).stop() };
        }
    }
}

pub struct Watcher {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    pub fn new(on_change: impl Fn(Vec<PathBuf>) + Send + 'static) -> Watcher {
        Watcher::spawn(
            HashSet::new(),
            kFSEventStreamEventIdSinceNow,
            Box::new(move |replay: Replay| {
                let mut dirs = replay.dirs;
                dirs.extend(replay.rescan);
                if !dirs.is_empty() {
                    on_change(dirs);
                }
            }),
        )
    }

    pub fn watch(&self, dir: &Path) {
        let dir = watched_path(dir);
        self.shared.paths.lock().unwrap().insert(dir);
        self.shared.wake();
    }

    pub fn unwatch(&self, dir: &Path) {
        let dir = watched_path(dir);
        self.shared.paths.lock().unwrap().remove(&dir);
        self.shared.wake();
    }

    fn spawn(
        paths: HashSet<PathBuf>,
        since: FSEventStreamEventId,
        on_change: Box<dyn Fn(Replay) + Send + 'static>,
    ) -> Watcher {
        let shared = Arc::new(Shared {
            paths: Mutex::new(paths),
            changed: Condvar::new(),
            since: AtomicU64::new(since),
            shutdown: AtomicBool::new(false),
            run_loop: Mutex::new(None),
        });
        let thread_shared = shared.clone();
        let thread = thread::spawn(move || run(thread_shared, on_change));
        Watcher {
            shared,
            thread: Some(thread),
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        self.shared.wake();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn since(
    event_id: u64,
    roots: Vec<PathBuf>,
    on_change: impl Fn(Replay) + Send + 'static,
) -> Watcher {
    let since_when = if event_id == 0 {
        kFSEventStreamEventIdSinceNow
    } else {
        event_id
    };
    let paths = roots.into_iter().map(|p| watched_path(&p)).collect();
    Watcher::spawn(paths, since_when, Box::new(on_change))
}

pub fn latest_event_id() -> u64 {
    unsafe { FSEventsGetCurrentEventId() }
}

fn watched_path(dir: &Path) -> PathBuf {
    let real = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    strip_trailing_slash(&real.to_string_lossy())
}

fn strip_trailing_slash(raw: &str) -> PathBuf {
    match raw.strip_suffix('/') {
        Some("") | None => PathBuf::from(if raw.is_empty() { "/" } else { raw }),
        Some(trimmed) => PathBuf::from(trimmed),
    }
}

struct Context {
    on_change: Box<dyn Fn(Replay) + Send + 'static>,
}

fn run(shared: Arc<Shared>, on_change: Box<dyn Fn(Replay) + Send + 'static>) {
    let run_loop = CFRunLoop::current().expect("watcher thread has a run loop");
    let run_loop_addr = (&*run_loop) as *const CFRunLoop as usize;
    let context = Box::into_raw(Box::new(Context { on_change }));

    loop {
        if shared.shutdown.load(Ordering::SeqCst) {
            break;
        }
        let snapshot: Vec<PathBuf> = {
            let paths = shared.paths.lock().unwrap();
            paths.iter().cloned().collect()
        };
        if snapshot.is_empty() {
            let mut paths = shared.paths.lock().unwrap();
            while paths.is_empty() && !shared.shutdown.load(Ordering::SeqCst) {
                paths = shared.changed.wait(paths).unwrap();
            }
            continue;
        }

        let cf_paths: Vec<CFRetained<CFString>> = snapshot
            .iter()
            .map(|p| CFString::from_str(&p.to_string_lossy()))
            .collect();
        let refs: Vec<&CFString> = cf_paths.iter().map(|s| &**s).collect();
        let array = CFArray::from_objects(&refs);
        let since_when = shared.since.load(Ordering::SeqCst);
        let mut stream_context = FSEventStreamContext {
            version: 0,
            info: context as *mut c_void,
            retain: None,
            release: None,
            copyDescription: None,
        };
        let stream = unsafe {
            FSEventStreamCreate(
                None,
                Some(callback),
                &mut stream_context,
                array.as_opaque(),
                since_when,
                0.1,
                kFSEventStreamCreateFlagUseCFTypes,
            )
        };
        assert!(!stream.is_null(), "FSEventStreamCreate failed");

        unsafe {
            #[allow(deprecated)]
            FSEventStreamScheduleWithRunLoop(
                stream,
                &run_loop,
                kCFRunLoopDefaultMode.expect("default run loop mode"),
            );
            FSEventStreamStart(stream);
        }
        *shared.run_loop.lock().unwrap() = Some(run_loop_addr);

        CFRunLoop::run();

        *shared.run_loop.lock().unwrap() = None;
        unsafe {
            FSEventStreamStop(stream);
            shared
                .since
                .store(FSEventStreamGetLatestEventId(stream), Ordering::SeqCst);
            FSEventStreamInvalidate(stream);
            FSEventStreamRelease(stream);
        }
    }

    drop(unsafe { Box::from_raw(context) });
}

unsafe extern "C-unwind" fn callback(
    _stream: ConstFSEventStreamRef,
    info: *mut c_void,
    num_events: usize,
    event_paths: NonNull<c_void>,
    event_flags: NonNull<FSEventStreamEventFlags>,
    event_ids: NonNull<FSEventStreamEventId>,
) {
    let context = unsafe { &*(info as *const Context) };
    let paths = unsafe { &*(event_paths.as_ptr() as *const CFArray<CFString>) };
    let flags = unsafe { std::slice::from_raw_parts(event_flags.as_ptr(), num_events) };
    let ids = unsafe { std::slice::from_raw_parts(event_ids.as_ptr(), num_events) };

    let mut replay = Replay::default();
    let (mut dirs, mut rescan) = (HashSet::new(), HashSet::new());
    for i in 0..num_events {
        replay.event_id = replay.event_id.max(ids[i]);
        replay.history_done |= flags[i] & kFSEventStreamEventFlagHistoryDone != 0;
        replay.wrapped |= flags[i] & kFSEventStreamEventFlagEventIdsWrapped != 0;
        if flags[i] & kFSEventStreamEventFlagHistoryDone != 0 {
            continue;
        }
        let Some(path) = paths.get(i) else { continue };
        let path = strip_trailing_slash(&path.to_string());
        match flags[i] & RESCAN != 0 {
            true => rescan.insert(path),
            false => dirs.insert(path),
        };
    }
    replay.dirs = dirs.into_iter().collect();
    replay.rescan = rescan.into_iter().collect();
    if replay.dirs.is_empty() && replay.rescan.is_empty() && !replay.history_done {
        return;
    }
    (context.on_change)(replay);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::time::Duration;

    // Disk-heavy tests running alongside starve an FSEvents stream's callbacks past the timeouts.
    fn serial() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("finder-fs-watch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        watched_path(&dir)
    }

    fn recv_containing(
        rx: &mpsc::Receiver<Vec<PathBuf>>,
        wanted: &Path,
        timeout: Duration,
    ) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match rx.recv_timeout(remaining) {
                Ok(dirs) if dirs.iter().any(|d| d == wanted) => return true,
                Ok(_) => continue,
                Err(_) => return false,
            }
        }
    }

    #[test]
    fn watch_reports_creates_renames_deletes() {
        let _serial = serial();
        let dir = temp_dir("basic");
        let sub = dir.join("sub");
        fs::create_dir_all(&sub).unwrap();
        let sub = watched_path(&sub);

        let (tx, rx) = mpsc::channel();
        let watcher = Watcher::new(move |dirs| {
            let _ = tx.send(dirs);
        });
        watcher.watch(&dir);
        watcher.watch(&sub);
        thread::sleep(Duration::from_millis(500));

        let started = std::time::Instant::now();
        fs::write(dir.join("a.txt"), b"hi").unwrap();
        assert!(
            recv_containing(&rx, &dir, Duration::from_secs(10)),
            "no callback for top-level create"
        );
        eprintln!("watch: create -> callback in {:?}", started.elapsed());

        let started = std::time::Instant::now();
        fs::rename(dir.join("a.txt"), dir.join("b.txt")).unwrap();
        assert!(
            recv_containing(&rx, &dir, Duration::from_secs(10)),
            "no callback for rename"
        );
        eprintln!("watch: rename -> callback in {:?}", started.elapsed());

        let started = std::time::Instant::now();
        fs::write(sub.join("c.txt"), b"hi").unwrap();
        assert!(
            recv_containing(&rx, &sub, Duration::from_secs(10)),
            "no callback for subdir create"
        );
        eprintln!(
            "watch: subdir create -> callback in {:?}",
            started.elapsed()
        );

        let started = std::time::Instant::now();
        fs::remove_file(dir.join("b.txt")).unwrap();
        assert!(
            recv_containing(&rx, &dir, Duration::from_secs(10)),
            "no callback for delete"
        );
        eprintln!("watch: delete -> callback in {:?}", started.elapsed());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unwatch_stops_callbacks() {
        let _serial = serial();
        let dir = temp_dir("unwatch");
        let (tx, rx) = mpsc::channel();
        let watcher = Watcher::new(move |dirs| {
            let _ = tx.send(dirs);
        });
        watcher.watch(&dir);
        thread::sleep(Duration::from_millis(250));
        fs::write(dir.join("a.txt"), b"hi").unwrap();
        assert!(
            recv_containing(&rx, &dir, Duration::from_secs(2)),
            "watch should have fired"
        );

        watcher.unwatch(&dir);
        thread::sleep(Duration::from_millis(300));
        while rx.try_recv().is_ok() {}
        fs::write(dir.join("d.txt"), b"hi").unwrap();
        assert!(
            !recv_containing(&rx, &dir, Duration::from_secs(2)),
            "unwatch should silence callbacks"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn since_replays_history() {
        let _serial = serial();
        let dir = temp_dir("since");
        let start_id = latest_event_id();
        fs::write(dir.join("a.txt"), b"hi").unwrap();
        thread::sleep(Duration::from_millis(500));

        let (tx, rx) = mpsc::channel();
        let started = std::time::Instant::now();
        let _watcher = since(start_id, vec![dir.clone()], move |replay: Replay| {
            let _ = tx.send((replay.dirs, replay.event_id));
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut saw_dir = false;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match rx.recv_timeout(remaining) {
                Ok((dirs, id)) => {
                    assert!(id >= start_id);
                    if dirs.iter().any(|d| d == &dir) {
                        saw_dir = true;
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        assert!(saw_dir, "since() did not replay the pre-existing change");
        eprintln!("since: replay -> callback in {:?}", started.elapsed());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn drop_stops_callbacks() {
        let _serial = serial();
        let dir = temp_dir("drop");
        let (tx, rx) = mpsc::channel();
        let watcher = Watcher::new(move |dirs| {
            let _ = tx.send(dirs);
        });
        watcher.watch(&dir);
        thread::sleep(Duration::from_millis(250));
        fs::write(dir.join("a.txt"), b"hi").unwrap();
        assert!(
            recv_containing(&rx, &dir, Duration::from_secs(2)),
            "watch should have fired"
        );

        drop(watcher);
        while rx.try_recv().is_ok() {}
        fs::write(dir.join("e.txt"), b"hi").unwrap();
        assert!(
            !recv_containing(&rx, &dir, Duration::from_secs(2)),
            "no callbacks should fire after drop"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
