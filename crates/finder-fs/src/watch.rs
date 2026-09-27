use std::cell::Cell;
use std::collections::HashSet;
use std::ffi::{CStr, OsStr, OsString, c_void};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::ptr::{NonNull, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use objc2_core_foundation::{
    CFArray, CFRetained, CFRunLoop, CFRunLoopRunResult, CFRunLoopSource, CFRunLoopSourceContext,
    CFString, kCFRunLoopDefaultMode,
};
#[allow(deprecated)]
use objc2_core_services::FSEventStreamScheduleWithRunLoop;
use objc2_core_services::{
    ConstFSEventStreamRef, FSEventStreamContext, FSEventStreamCreate, FSEventStreamEventFlags,
    FSEventStreamEventId, FSEventStreamGetLatestEventId, FSEventStreamInvalidate,
    FSEventStreamRelease, FSEventStreamStart, FSEventStreamStop, FSEventsGetCurrentEventId,
    kFSEventStreamCreateFlagUseCFTypes, kFSEventStreamCreateFlagWatchRoot,
    kFSEventStreamEventFlagEventIdsWrapped, kFSEventStreamEventFlagHistoryDone,
    kFSEventStreamEventFlagKernelDropped, kFSEventStreamEventFlagMount,
    kFSEventStreamEventFlagMustScanSubDirs, kFSEventStreamEventFlagRootChanged,
    kFSEventStreamEventFlagUnmount, kFSEventStreamEventFlagUserDropped,
    kFSEventStreamEventIdSinceNow,
};

#[derive(Clone, Debug, Default)]
pub struct Replay {
    pub dirs: Vec<PathBuf>,
    pub rescan: Vec<PathBuf>,
    pub event_id: u64,
    pub history_done: bool,
    pub wrapped: bool,
}

const RESCAN: FSEventStreamEventFlags = kFSEventStreamEventFlagMustScanSubDirs
    | kFSEventStreamEventFlagUserDropped
    | kFSEventStreamEventFlagKernelDropped
    | kFSEventStreamEventFlagRootChanged
    | kFSEventStreamEventFlagMount
    | kFSEventStreamEventFlagUnmount;

/// A `since` callback that put work off, and says so, gets an empty replay a second later.
const TICK: f64 = 1.0;
/// Otherwise the thread sleeps until an event or a change of paths.
const IDLE: f64 = 1.0e10;

type OnChange = Box<dyn Fn(Replay) -> bool + Send>;

/// Each watched path beside the path FSEvents reports it by.
type Views = Arc<Mutex<Vec<(PathBuf, PathBuf)>>>;

struct Shared {
    paths: Mutex<HashSet<PathBuf>>,
    /// Filled on the watcher's thread; `None` for a caller that wants the reported paths.
    views: Option<Views>,
    changed: Condvar,
    since: AtomicU64,
    shutdown: AtomicBool,
    /// The watcher's run loop and the source that wakes it, while its thread runs.
    run_loop: Mutex<Option<(usize, usize)>>,
    /// The paths moved since the stream was made.
    stale: AtomicBool,
    latency: f64,
}

impl Shared {
    fn wake(&self) {
        self.stale.store(true, Ordering::SeqCst);
        self.changed.notify_all();
        if let Some((run_loop, source)) = *self.run_loop.lock().unwrap() {
            // A stop lands only in a run already under way; a signalled source waits for the next.
            unsafe {
                (&*(source as *const CFRunLoopSource)).signal();
                (&*(run_loop as *const CFRunLoop)).wake_up();
            }
        }
    }
}

pub struct Watcher {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// A reported folder, and the same folder under every watched path it lies beneath.
fn as_watched(views: &[(PathBuf, PathBuf)], dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: HashSet<PathBuf> = HashSet::with_capacity(dirs.len());
    for dir in dirs {
        for (real, asked) in views {
            if let Ok(rest) = dir.strip_prefix(real) {
                out.insert(match rest.as_os_str().is_empty() {
                    true => asked.clone(),
                    false => asked.join(rest),
                });
            }
        }
        out.insert(dir);
    }
    out.into_iter().collect()
}

impl Watcher {
    /// Reports each changed folder as FSEvents names it and under every path it was watched by.
    pub fn new(on_change: impl Fn(Vec<PathBuf>) + Send + 'static) -> Watcher {
        let views = Views::default();
        let seen = views.clone();
        Watcher::spawn(
            HashSet::new(),
            kFSEventStreamEventIdSinceNow,
            0.1,
            Some(views),
            Box::new(move |replay: Replay| {
                let mut dirs = replay.dirs;
                dirs.extend(replay.rescan);
                if !dirs.is_empty() {
                    on_change(as_watched(&seen.lock().unwrap(), dirs));
                }
                false
            }),
            false,
        )
    }

    /// Touches no disk: the watcher's own thread resolves the path.
    pub fn watch(&self, dir: &Path) {
        self.shared.paths.lock().unwrap().insert(dir.to_path_buf());
        self.shared.wake();
    }

    pub fn unwatch(&self, dir: &Path) {
        self.shared.paths.lock().unwrap().remove(dir);
        self.shared.wake();
    }

    fn spawn(
        paths: HashSet<PathBuf>,
        since: FSEventStreamEventId,
        latency: f64,
        views: Option<Views>,
        on_change: OnChange,
        tick: bool,
    ) -> Watcher {
        let shared = Arc::new(Shared {
            paths: Mutex::new(paths),
            views,
            changed: Condvar::new(),
            since: AtomicU64::new(since),
            shutdown: AtomicBool::new(false),
            run_loop: Mutex::new(None),
            stale: AtomicBool::new(false),
            latency,
        });
        let thread_shared = shared.clone();
        let thread = thread::spawn(move || {
            crate::utility_qos();
            run(thread_shared, on_change, tick)
        });
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

/// `on_change` answers whether it put work off; until it says it has not, it ticks.
pub fn since(
    event_id: u64,
    roots: Vec<PathBuf>,
    on_change: impl Fn(Replay) -> bool + Send + 'static,
) -> Watcher {
    let since_when = if event_id == 0 {
        kFSEventStreamEventIdSinceNow
    } else {
        event_id
    };
    // A folder written ten times a second is read once a second; the index can lag that much.
    Watcher::spawn(
        roots.into_iter().collect(),
        since_when,
        1.0,
        None,
        Box::new(on_change),
        true,
    )
}

pub fn latest_event_id() -> u64 {
    unsafe { FSEventsGetCurrentEventId() }
}

/// Whether FSEvents holds a change since `event_id` that could alter what `dir` lists: in it, in a
/// folder in it, or dropped above it. Unanswered within `wait`, it says there may be one.
pub fn changed_since(dir: &Path, event_id: u64, wait: Duration) -> bool {
    let probe: Probe = (resolved(dir), Cell::new(false), Cell::new(false));
    let path = CFString::from_str(&probe.0.to_string_lossy());
    let array = CFArray::from_objects(&[&*path]);
    let mut context = FSEventStreamContext {
        version: 0,
        info: &probe as *const Probe as *mut c_void,
        retain: None,
        release: None,
        copyDescription: None,
    };
    let stream = unsafe {
        FSEventStreamCreate(
            None,
            Some(probed),
            &mut context,
            array.as_opaque(),
            event_id,
            0.0,
            kFSEventStreamCreateFlagUseCFTypes,
        )
    };
    let Some(run_loop) = CFRunLoop::current().filter(|_| !stream.is_null()) else {
        return true;
    };
    let mode = CFString::from_static_str("finder-fs.changed-since");
    #[allow(deprecated)]
    let started = unsafe {
        FSEventStreamScheduleWithRunLoop(stream, &run_loop, &mode);
        FSEventStreamStart(stream)
    };
    let deadline = Instant::now() + wait;
    while started && !probe.1.get() && !probe.2.get() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        CFRunLoop::run_in_mode(Some(&mode), left.as_secs_f64(), true);
    }
    unsafe {
        FSEventStreamStop(stream);
        FSEventStreamInvalidate(stream);
        FSEventStreamRelease(stream);
    }
    probe.1.get() || !probe.2.get()
}

pub(crate) fn resolved(dir: &Path) -> PathBuf {
    let real = crate::list::real_path(dir).unwrap_or_else(|| dir.to_path_buf());
    strip_trailing_slash(&real.to_string_lossy())
}

/// Mount points of network shares, from the kernel's cached table.
fn shares() -> Vec<PathBuf> {
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    let mut stats: Vec<libc::statfs> = Vec::with_capacity(count.max(0) as usize);
    let bytes = (stats.capacity() * std::mem::size_of::<libc::statfs>()) as libc::c_int;
    let got = unsafe { libc::getfsstat(stats.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
    unsafe { stats.set_len(got.clamp(0, count.max(0)) as usize) };
    stats
        .iter()
        .filter(|mount| mount.f_flags & libc::MNT_LOCAL as u32 == 0)
        .map(|mount| {
            let on = unsafe { CStr::from_ptr(mount.f_mntonname.as_ptr()) };
            PathBuf::from(OsStr::from_bytes(on.to_bytes()))
        })
        .collect()
}

/// Whether a path leads onto a network share. Nothing there is watched: the kernel never hears
/// of a change made on the server, and one call into a share that stopped answering would hold
/// this thread and every folder it watches. Symlinks are followed only while the path so far is
/// on this Mac, so nothing on a share is touched to find out.
fn on_share(path: &Path) -> bool {
    leads_into(path, &shares())
}

fn leads_into(path: &Path, shares: &[PathBuf]) -> bool {
    if shares.is_empty() {
        return false;
    }
    let reaches = |at: &Path| {
        let bare = at
            .strip_prefix("/System/Volumes/Data")
            .map(|rest| Path::new("/").join(rest));
        shares.iter().any(|share| {
            at.starts_with(share) || bare.as_ref().is_ok_and(|bare| bare.starts_with(share))
        })
    };
    let mut real = PathBuf::from("/");
    let mut rest: Vec<OsString> = path.iter().rev().map(OsString::from).collect();
    let mut hops = 0;
    while let Some(part) = rest.pop() {
        match part.as_bytes() {
            b"/" => real = PathBuf::from("/"),
            b"." => {}
            b".." => {
                real.pop();
            }
            _ => {
                let next = real.join(&part);
                if reaches(&next) {
                    return true;
                }
                match std::fs::read_link(&next) {
                    Ok(target) if hops < 32 => {
                        hops += 1;
                        rest.extend(target.iter().rev().map(OsString::from));
                    }
                    _ => real = next,
                }
            }
        }
    }
    false
}

fn strip_trailing_slash(raw: &str) -> PathBuf {
    match raw.strip_suffix('/') {
        Some("") | None => PathBuf::from(if raw.is_empty() { "/" } else { raw }),
        Some(trimmed) => PathBuf::from(trimmed),
    }
}

/// The callback, and whether it last asked for a tick.
type Context = (OnChange, Cell<bool>);

fn run(shared: Arc<Shared>, on_change: OnChange, tick: bool) {
    let run_loop = CFRunLoop::current().expect("watcher thread has a run loop");
    let mode = unsafe { kCFRunLoopDefaultMode };
    let mut stirred = CFRunLoopSourceContext {
        version: 0,
        info: null_mut(),
        retain: None,
        release: None,
        copyDescription: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform: Some(woken),
    };
    let source = unsafe { CFRunLoopSource::new(None, 0, &mut stirred) }.expect("run loop source");
    run_loop.add_source(Some(&source), mode);
    *shared.run_loop.lock().unwrap() = Some((
        (&*run_loop) as *const CFRunLoop as usize,
        (&*source) as *const CFRunLoopSource as usize,
    ));
    let context: *mut Context = Box::into_raw(Box::new((on_change, Cell::new(tick))));

    while !shared.shutdown.load(Ordering::SeqCst) {
        shared.stale.store(false, Ordering::SeqCst);
        let all: HashSet<PathBuf> = shared.paths.lock().unwrap().clone();
        let snapshot: Vec<PathBuf> = all.iter().filter(|p| !on_share(p)).cloned().collect();
        if snapshot.is_empty() {
            let mut paths = shared.paths.lock().unwrap();
            while *paths == all && !shared.shutdown.load(Ordering::SeqCst) {
                paths = shared.changed.wait(paths).unwrap();
            }
            continue;
        }

        let views: Vec<(PathBuf, PathBuf)> = snapshot
            .into_iter()
            .map(|asked| (resolved(&asked), asked))
            .collect();
        let roots: HashSet<&PathBuf> = views.iter().map(|(real, _)| real).collect();
        let cf_paths: Vec<CFRetained<CFString>> = roots
            .iter()
            .map(|p| CFString::from_str(&p.to_string_lossy()))
            .collect();
        if let Some(published) = &shared.views {
            *published.lock().unwrap() = views;
        }
        let refs: Vec<&CFString> = cf_paths.iter().map(|s| &**s).collect();
        let array = CFArray::from_objects(&refs);
        // Without WatchRoot a stream says nothing when the folder it watches is renamed or deleted.
        let flags = match shared.views {
            Some(_) => kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagWatchRoot,
            None => kFSEventStreamCreateFlagUseCFTypes,
        };
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
                shared.latency,
                flags,
            )
        };
        assert!(!stream.is_null(), "FSEventStreamCreate failed");

        unsafe {
            #[allow(deprecated)]
            FSEventStreamScheduleWithRunLoop(
                stream,
                &run_loop,
                mode.expect("default run loop mode"),
            );
            FSEventStreamStart(stream);
        }

        while !shared.stale.load(Ordering::SeqCst) {
            let (on_change, ticking) = unsafe { &*context };
            let wait = if ticking.get() { TICK } else { IDLE };
            if CFRunLoop::run_in_mode(mode, wait, true) == CFRunLoopRunResult::TimedOut {
                ticking.set(on_change(Replay::default()));
            }
        }

        unsafe {
            FSEventStreamStop(stream);
            shared
                .since
                .store(FSEventStreamGetLatestEventId(stream), Ordering::SeqCst);
            FSEventStreamInvalidate(stream);
            FSEventStreamRelease(stream);
        }
    }

    *shared.run_loop.lock().unwrap() = None;
    run_loop.remove_source(Some(&source), mode);
    drop(unsafe { Box::from_raw(context) });
}

unsafe extern "C-unwind" fn woken(_: *mut c_void) {}

unsafe extern "C-unwind" fn callback(
    _stream: ConstFSEventStreamRef,
    info: *mut c_void,
    num_events: usize,
    event_paths: NonNull<c_void>,
    event_flags: NonNull<FSEventStreamEventFlags>,
    event_ids: NonNull<FSEventStreamEventId>,
) {
    let (on_change, ticking) = unsafe { &*(info as *const Context) };
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
    ticking.set(on_change(replay));
}

/// The folder asked about, whether a change to it came, and whether history is done.
type Probe = (PathBuf, Cell<bool>, Cell<bool>);

unsafe extern "C-unwind" fn probed(
    _stream: ConstFSEventStreamRef,
    info: *mut c_void,
    num_events: usize,
    event_paths: NonNull<c_void>,
    event_flags: NonNull<FSEventStreamEventFlags>,
    _event_ids: NonNull<FSEventStreamEventId>,
) {
    let (dir, changed, done) = unsafe { &*(info as *const Probe) };
    let paths = unsafe { &*(event_paths.as_ptr() as *const CFArray<CFString>) };
    let flags = unsafe { std::slice::from_raw_parts(event_flags.as_ptr(), num_events) };
    for (i, flag) in flags.iter().enumerate() {
        if flag & kFSEventStreamEventFlagHistoryDone != 0 {
            done.set(true);
            continue;
        }
        let Some(path) = paths.get(i) else { continue };
        let path = strip_trailing_slash(&path.to_string());
        if flag & kFSEventStreamEventFlagEventIdsWrapped != 0
            || path.parent() == Some(dir.as_path())
            || dir.starts_with(&path)
        {
            changed.set(true);
        }
    }
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
        resolved(&dir)
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
        let sub = resolved(&sub);

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
    fn a_change_is_reported_under_every_path_its_folder_was_watched_by() {
        let _serial = serial();
        let real = temp_dir("shapes");
        let linked = std::env::temp_dir().join(real.file_name().unwrap());
        assert_ne!(linked, real, "the temp dir is under a symlink");
        let firmlinked = Path::new("/System/Volumes/Data").join(real.strip_prefix("/").unwrap());
        let holder = temp_dir("shapes-holder");
        let through = holder.join("link");
        std::os::unix::fs::symlink(&real, &through).unwrap();

        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
        let watcher = Watcher::new(move |dirs| {
            let _ = tx.send(dirs);
        });
        for asked in [&linked, &firmlinked, &holder, &through] {
            watcher.watch(asked);
        }
        thread::sleep(Duration::from_millis(500));

        fs::write(real.join("a.txt"), b"hi").unwrap();
        let mut missing: HashSet<PathBuf> = HashSet::from([linked, firmlinked, through]);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !missing.is_empty() {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let Ok(dirs) = rx.recv_timeout(remaining) else {
                break;
            };
            for dir in dirs {
                missing.remove(&dir);
            }
        }
        assert!(missing.is_empty(), "never reported under {missing:?}");

        let _ = fs::remove_dir_all(&real);
        let _ = fs::remove_dir_all(&holder);
    }

    #[test]
    fn a_path_through_symlinks_is_found_to_lead_onto_a_share() {
        let base = temp_dir("shares");
        fs::create_dir_all(base.join("share/in")).unwrap();
        fs::create_dir_all(base.join("dir")).unwrap();
        std::os::unix::fs::symlink(base.join("share"), base.join("abs")).unwrap();
        std::os::unix::fs::symlink("../share/in", base.join("dir/rel")).unwrap();
        let shares = [base.join("share")];
        let linked = std::env::temp_dir().join(base.file_name().unwrap());
        assert_ne!(linked, base, "the temp dir is under a symlink");

        for onto in ["share/in", "abs", "abs/in", "dir/rel", "dir/../share"] {
            assert!(leads_into(&linked.join(onto), &shares), "{onto}");
        }
        let firmlinked = Path::new("/System/Volumes/Data").join(base.strip_prefix("/").unwrap());
        assert!(leads_into(&firmlinked.join("share"), &shares));
        for off in ["dir", "dir/../abs-not-there", "shared"] {
            assert!(!leads_into(&linked.join(off), &shares), "{off}");
        }
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_watched_folder_that_is_renamed_is_reported() {
        let _serial = serial();
        let dir = temp_dir("renamed");
        let moved =
            dir.with_file_name(format!("finder-fs-watch-renamed-to-{}", std::process::id()));
        let _ = fs::remove_dir_all(&moved);
        let (tx, rx) = mpsc::channel();
        let watcher = Watcher::new(move |dirs| {
            let _ = tx.send(dirs);
        });
        watcher.watch(&dir);
        thread::sleep(Duration::from_millis(500));

        fs::rename(&dir, &moved).unwrap();
        assert!(
            recv_containing(&rx, &dir, Duration::from_secs(10)),
            "the rename of the watched folder went unreported"
        );

        let _ = fs::remove_dir_all(&moved);
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
            if !replay.dirs.is_empty() {
                let _ = tx.send((replay.dirs, replay.event_id));
            }
            false
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

    #[test]
    fn a_drop_during_a_tick_still_stops_the_watcher() {
        let _serial = serial();
        let dir = temp_dir("tick-drop");
        let (ticked, tick) = mpsc::channel();
        let watcher = since(0, vec![dir.clone()], move |replay: Replay| {
            if replay.event_id == 0 {
                let _ = ticked.send(());
                thread::sleep(Duration::from_millis(300));
            }
            true
        });
        tick.recv_timeout(Duration::from_secs(3)).unwrap();
        let (dropped, done) = mpsc::channel();
        thread::spawn(move || {
            drop(watcher);
            let _ = dropped.send(());
        });
        assert!(done.recv_timeout(Duration::from_secs(3)).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }
}
