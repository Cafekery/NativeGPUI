use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ffi::{OsString, c_void};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::ptr::{NonNull, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use objc2_core_foundation::{
    CFArray, CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, CFString,
    kCFRunLoopDefaultMode,
};
#[allow(deprecated)]
use objc2_core_services::FSEventStreamScheduleWithRunLoop;
use objc2_core_services::{
    ConstFSEventStreamRef, FSEventStreamCallback, FSEventStreamContext, FSEventStreamCreate,
    FSEventStreamCreateFlags, FSEventStreamEventFlags, FSEventStreamEventId,
    FSEventStreamGetLatestEventId, FSEventStreamInvalidate, FSEventStreamRef, FSEventStreamRelease,
    FSEventStreamStart, FSEventStreamStop, kFSEventStreamCreateFlagFileEvents,
    kFSEventStreamCreateFlagUseCFTypes, kFSEventStreamCreateFlagWatchRoot,
    kFSEventStreamEventFlagEventIdsWrapped, kFSEventStreamEventFlagHistoryDone,
    kFSEventStreamEventFlagItemIsDir, kFSEventStreamEventFlagItemIsFile,
    kFSEventStreamEventFlagItemIsSymlink, kFSEventStreamEventIdSinceNow,
};

const LATENCY: f64 = 0.1;
const IDLE: f64 = 1.0e10;

type OnChange = Box<dyn Fn(Vec<PathBuf>, Vec<PathBuf>) + Send>;

struct Shared {
    paths: Mutex<HashSet<PathBuf>>,
    changed: Condvar,
    shutdown: AtomicBool,
    /// The watcher's run loop and its wake-up source, while its thread runs.
    run_loop: Mutex<Option<(usize, usize)>>,
    stale: AtomicBool,
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

fn as_watched(views: &[(PathBuf, PathBuf)], dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: HashSet<PathBuf> = HashSet::with_capacity(dirs.len());
    for dir in dirs {
        for (real, asked) in views {
            if let Ok(rest) = dir.strip_prefix(real) {
                out.insert(if rest.as_os_str().is_empty() {
                    asked.clone()
                } else {
                    asked.join(rest)
                });
            }
        }
        out.insert(dir);
    }
    out.into_iter().collect()
}

impl Watcher {
    /// Reports each changed folder, then each changed item and the folder an `Icon\r` dresses,
    /// as FSEvents names them and under every path they were watched by.
    pub fn new(on_change: impl Fn(Vec<PathBuf>, Vec<PathBuf>) + Send + 'static) -> Watcher {
        let shared = Arc::new(Shared {
            paths: Mutex::default(),
            changed: Condvar::new(),
            shutdown: AtomicBool::new(false),
            run_loop: Mutex::new(None),
            stale: AtomicBool::new(false),
        });
        let thread_shared = shared.clone();
        let thread = thread::spawn(move || {
            crate::utility_qos();
            run(thread_shared, Box::new(on_change))
        });
        Watcher {
            shared,
            thread: Some(thread),
        }
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

struct Stream(FSEventStreamRef);

impl Stream {
    fn new(
        paths: &[&CFString],
        callback: FSEventStreamCallback,
        info: *mut c_void,
        since: FSEventStreamEventId,
        latency: f64,
        flags: FSEventStreamCreateFlags,
    ) -> Option<Stream> {
        let array = CFArray::from_objects(paths);
        let mut context = FSEventStreamContext {
            version: 0,
            info,
            retain: None,
            release: None,
            copyDescription: None,
        };
        let stream = unsafe {
            FSEventStreamCreate(
                None,
                callback,
                &mut context,
                array.as_opaque(),
                since,
                latency,
                flags,
            )
        };
        (!stream.is_null()).then_some(Stream(stream))
    }

    fn start(&self, run_loop: &CFRunLoop, mode: &CFString) -> bool {
        unsafe {
            #[allow(deprecated)]
            FSEventStreamScheduleWithRunLoop(self.0, run_loop, mode);
            FSEventStreamStart(self.0)
        }
    }

    fn latest_event_id(&self) -> u64 {
        unsafe { FSEventStreamGetLatestEventId(self.0) }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            FSEventStreamStop(self.0);
            FSEventStreamInvalidate(self.0);
            FSEventStreamRelease(self.0);
        }
    }
}

struct Probe {
    dir: PathBuf,
    changed: Cell<bool>,
    history_done: Cell<bool>,
}

/// Whether a change since `event_id` may alter what `dir` lists; unanswered within `wait`, yes.
pub fn changed_since(dir: &Path, event_id: u64, wait: Duration) -> bool {
    let probe = Probe {
        dir: resolved(dir),
        changed: Cell::new(false),
        history_done: Cell::new(false),
    };
    let Some(run_loop) = CFRunLoop::current() else {
        return true;
    };
    let path = CFString::from_str(&probe.dir.to_string_lossy());
    let Some(stream) = Stream::new(
        &[&*path],
        Some(probed),
        &probe as *const Probe as *mut c_void,
        event_id,
        0.0,
        kFSEventStreamCreateFlagUseCFTypes,
    ) else {
        return true;
    };
    let mode = CFString::from_static_str("finder-fs.changed-since");
    let started = stream.start(&run_loop, &mode);
    let deadline = Instant::now() + wait;
    while started && !probe.changed.get() && !probe.history_done.get() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        CFRunLoop::run_in_mode(Some(&mode), left.as_secs_f64(), true);
    }
    drop(stream);
    probe.changed.get() || !probe.history_done.get()
}

fn resolved(dir: &Path) -> PathBuf {
    let real = crate::list::real_path(dir).unwrap_or_else(|| dir.to_path_buf());
    strip_trailing_slash(&real.to_string_lossy())
}

fn shares() -> Vec<PathBuf> {
    crate::volumes::mount_table()
        .iter()
        .filter(|mount| mount.f_flags & libc::MNT_LOCAL as u32 == 0)
        .map(crate::volumes::mount_path)
        .collect()
}

/// FSEvents never hears of a change made on a server, and a dead share would hang this thread.
fn on_share(path: &Path) -> bool {
    leads_into(path, &shares())
}

/// Follows symlinks only while the path so far is on this Mac, so nothing on a share is touched.
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
    let trimmed = raw
        .strip_suffix('/')
        .filter(|t| !t.is_empty())
        .unwrap_or(raw);
    PathBuf::from(if trimmed.is_empty() { "/" } else { trimmed })
}

struct Context {
    on_change: OnChange,
    views: RefCell<Vec<(PathBuf, PathBuf)>>,
}

fn run(shared: Arc<Shared>, on_change: OnChange) {
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
    let context = Context {
        on_change,
        views: RefCell::default(),
    };
    let mut since = kFSEventStreamEventIdSinceNow;

    while !shared.shutdown.load(Ordering::SeqCst) {
        shared.stale.store(false, Ordering::SeqCst);
        let all: HashSet<PathBuf> = shared.paths.lock().unwrap().clone();
        let views: Vec<(PathBuf, PathBuf)> = all
            .iter()
            .filter(|path| !on_share(path))
            .map(|asked| (resolved(asked), asked.clone()))
            .collect();
        if views.is_empty() {
            let mut paths = shared.paths.lock().unwrap();
            while *paths == all && !shared.shutdown.load(Ordering::SeqCst) {
                paths = shared.changed.wait(paths).unwrap();
            }
            continue;
        }

        let roots: HashSet<&PathBuf> = views.iter().map(|(real, _)| real).collect();
        let cf_paths: Vec<CFRetained<CFString>> = roots
            .iter()
            .map(|p| CFString::from_str(&p.to_string_lossy()))
            .collect();
        let refs: Vec<&CFString> = cf_paths.iter().map(|s| &**s).collect();
        *context.views.borrow_mut() = views;
        let stream = Stream::new(
            &refs,
            Some(callback),
            &context as *const Context as *mut c_void,
            since,
            LATENCY,
            // Without WatchRoot a stream says nothing when the folder it watches is renamed or deleted.
            kFSEventStreamCreateFlagUseCFTypes
                | kFSEventStreamCreateFlagWatchRoot
                | kFSEventStreamCreateFlagFileEvents,
        )
        .expect("FSEventStreamCreate failed");
        stream.start(&run_loop, mode.expect("default run loop mode"));

        while !shared.stale.load(Ordering::SeqCst) {
            CFRunLoop::run_in_mode(mode, IDLE, true);
        }
        since = stream.latest_event_id();
    }

    *shared.run_loop.lock().unwrap() = None;
    run_loop.remove_source(Some(&source), mode);
}

unsafe extern "C-unwind" fn woken(_: *mut c_void) {}

unsafe extern "C-unwind" fn callback(
    _stream: ConstFSEventStreamRef,
    info: *mut c_void,
    num_events: usize,
    event_paths: NonNull<c_void>,
    event_flags: NonNull<FSEventStreamEventFlags>,
    _event_ids: NonNull<FSEventStreamEventId>,
) {
    let context = unsafe { &*(info as *const Context) };
    let paths = unsafe { &*(event_paths.as_ptr() as *const CFArray<CFString>) };
    let flags = unsafe { std::slice::from_raw_parts(event_flags.as_ptr(), num_events) };
    let item = kFSEventStreamEventFlagItemIsFile
        | kFSEventStreamEventFlagItemIsDir
        | kFSEventStreamEventFlagItemIsSymlink;
    let (mut dirs, mut items) = (Vec::new(), Vec::new());
    for (i, flag) in flags.iter().enumerate() {
        if flag & kFSEventStreamEventFlagHistoryDone != 0 {
            continue;
        }
        let Some(path) = paths.get(i) else { continue };
        let path = strip_trailing_slash(&path.to_string());
        let Some(dir) = path.parent().filter(|_| flag & item != 0) else {
            dirs.push(path);
            continue;
        };
        if path.file_name() == Some("Icon\r".as_ref()) {
            items.push(dir.to_path_buf());
        }
        dirs.push(dir.to_path_buf());
        items.push(path);
    }
    if !dirs.is_empty() {
        let views = context.views.borrow();
        (context.on_change)(as_watched(&views, dirs), as_watched(&views, items));
    }
}

unsafe extern "C-unwind" fn probed(
    _stream: ConstFSEventStreamRef,
    info: *mut c_void,
    num_events: usize,
    event_paths: NonNull<c_void>,
    event_flags: NonNull<FSEventStreamEventFlags>,
    _event_ids: NonNull<FSEventStreamEventId>,
) {
    let probe = unsafe { &*(info as *const Probe) };
    let paths = unsafe { &*(event_paths.as_ptr() as *const CFArray<CFString>) };
    let flags = unsafe { std::slice::from_raw_parts(event_flags.as_ptr(), num_events) };
    for (i, flag) in flags.iter().enumerate() {
        if flag & kFSEventStreamEventFlagHistoryDone != 0 {
            probe.history_done.set(true);
            continue;
        }
        let Some(path) = paths.get(i) else { continue };
        let path = strip_trailing_slash(&path.to_string());
        if flag & kFSEventStreamEventFlagEventIdsWrapped != 0
            || path.parent() == Some(probe.dir.as_path())
            || probe.dir.starts_with(&path)
        {
            probe.changed.set(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::sync::MutexGuard;
    use std::sync::mpsc;

    // Disk-heavy tests running alongside starve an FSEvents stream's callbacks past the timeouts.
    fn serial() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("finder-fs-watch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        resolved(&dir)
    }

    fn watching(dirs: &[&Path]) -> (Watcher, mpsc::Receiver<Vec<PathBuf>>) {
        let (tx, rx) = mpsc::channel();
        let watcher = Watcher::new(move |dirs, _| {
            let _ = tx.send(dirs);
        });
        for dir in dirs {
            watcher.watch(dir);
        }
        (watcher, rx)
    }

    fn recv_containing(
        rx: &mpsc::Receiver<Vec<PathBuf>>,
        wanted: &Path,
        timeout: Duration,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match rx.recv_timeout(remaining) {
                Ok(dirs) if dirs.iter().any(|d| d == wanted) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    }

    #[test]
    fn a_changed_item_is_reported_with_the_folder_its_icon_dresses() {
        let _serial = serial();
        let dir = temp_dir("items");
        let folder = dir.join("folder");
        fs::create_dir_all(&folder).unwrap();
        let (tx, rx) = mpsc::channel();
        let watcher = Watcher::new(move |_, items| {
            let _ = tx.send(items);
        });
        watcher.watch(&dir);
        thread::sleep(Duration::from_millis(500));
        let wait = Duration::from_secs(10);

        fs::write(dir.join("file"), b"").unwrap();
        assert!(recv_containing(&rx, &dir.join("file"), wait));
        fs::write(folder.join("Icon\r"), b"").unwrap();
        assert!(recv_containing(&rx, &folder, wait));
        drop(watcher);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn watch_reports_creates_renames_deletes() {
        let _serial = serial();
        let dir = temp_dir("basic");
        let sub = dir.join("sub");
        fs::create_dir_all(&sub).unwrap();
        let sub = resolved(&sub);
        let (_watcher, rx) = watching(&[&dir, &sub]);
        thread::sleep(Duration::from_millis(500));
        let wait = Duration::from_secs(10);

        fs::write(dir.join("a.txt"), b"hi").unwrap();
        assert!(
            recv_containing(&rx, &dir, wait),
            "no callback for top-level create"
        );
        fs::rename(dir.join("a.txt"), dir.join("b.txt")).unwrap();
        assert!(recv_containing(&rx, &dir, wait), "no callback for rename");
        fs::write(sub.join("c.txt"), b"hi").unwrap();
        assert!(
            recv_containing(&rx, &sub, wait),
            "no callback for subdir create"
        );
        fs::remove_file(dir.join("b.txt")).unwrap();
        assert!(recv_containing(&rx, &dir, wait), "no callback for delete");

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
        symlink(&real, &through).unwrap();
        let (_watcher, rx) = watching(&[&linked, &firmlinked, &holder, &through]);
        thread::sleep(Duration::from_millis(500));

        fs::write(real.join("a.txt"), b"hi").unwrap();
        let mut missing = HashSet::from([linked, firmlinked, through]);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !missing.is_empty() {
            let Ok(dirs) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            else {
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
        symlink(base.join("share"), base.join("abs")).unwrap();
        symlink("../share/in", base.join("dir/rel")).unwrap();
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
        let (_watcher, rx) = watching(&[&dir]);
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
        let (watcher, rx) = watching(&[&dir]);
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
    fn drop_stops_callbacks() {
        let _serial = serial();
        let dir = temp_dir("drop");
        let (watcher, rx) = watching(&[&dir]);
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
