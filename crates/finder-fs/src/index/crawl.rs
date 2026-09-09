use std::collections::VecDeque;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Instant, SystemTime};

use super::store::{NONE, VISITED};
use super::{Index, Progress};
use crate::entry::{Entry, EntryKind};
use crate::list::list_dir;

const THREADS: usize = 6;
const REPORT_EVERY_MS: u64 = 100;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Descend,
    /// Walked for its bytes, never listed: a package, a cache, a trash.
    Weigh,
    /// Neither listed nor weighed: another volume's tree, a symlink, a device.
    Seal,
}

struct Task {
    node: u32,
    path: PathBuf,
    weigh: bool,
}

#[derive(Default)]
struct Work {
    tasks: Vec<Task>,
    later: Vec<Task>,
    roots: VecDeque<PathBuf>,
    busy: usize,
    waiting: usize,
    done: bool,
}

pub struct Crawl {
    index: Arc<Index>,
    work: Mutex<Work>,
    wake: Condvar,
    report: Mutex<Box<dyn Fn(Progress) + Send>>,
    dirs: AtomicU64,
    files: AtomicU64,
    bytes: AtomicU64,
    last_report: AtomicU64,
    finishing: AtomicBool,
    start: Instant,
    home: PathBuf,
    walk: Walk,
}

pub struct Walk {
    pub crawl: Vec<PathBuf>,
    pub record: Vec<PathBuf>,
    pub since: u64,
}

pub fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn spawn(index: &Arc<Index>, walk: Walk, report: impl Fn(Progress) + Send + 'static) {
    let walk = Walk {
        crawl: walk.crawl.iter().map(|root| real(root)).collect(),
        record: walk.record.iter().map(|root| real(root)).collect(),
        since: walk.since,
    };
    let crawl = Arc::new(Crawl {
        index: index.clone(),
        work: Mutex::new(Work {
            roots: walk.crawl.iter().cloned().collect(),
            ..Work::default()
        }),
        wake: Condvar::new(),
        report: Mutex::new(Box::new(report)),
        dirs: AtomicU64::new(0),
        files: AtomicU64::new(0),
        bytes: AtomicU64::new(0),
        last_report: AtomicU64::new(0),
        finishing: AtomicBool::new(false),
        start: Instant::now(),
        home: home(),
        walk,
    });
    for _ in 0..THREADS {
        let crawl = crawl.clone();
        std::thread::spawn(move || crawl.run());
    }
}

static TRACE_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(PathBuf::new, PathBuf::from)
}

impl Crawl {
    fn run(self: &Arc<Self>) {
        while let Some(task) = self.next() {
            if std::env::var_os("NG_CRAWL_TRACE").is_some() {
                let ms = TRACE_START
                    .get_or_init(std::time::Instant::now)
                    .elapsed()
                    .as_millis();
                eprintln!(
                    "crawl {ms} {} {}",
                    if task.weigh { "W" } else { "L" },
                    task.path.display()
                );
            }
            match task.weigh {
                true => self.weigh(task),
                false => self.list(task),
            }
            let mut work = self.work.lock().unwrap();
            work.busy -= 1;
            if work.busy == 0 {
                self.wake.notify_all();
            }
        }
        if !self.finishing.swap(true, Ordering::SeqCst) {
            self.finish();
        }
    }

    fn next(&self) -> Option<Task> {
        let mut work = self.work.lock().unwrap();
        loop {
            if let Some(task) = work.tasks.pop() {
                work.busy += 1;
                return Some(task);
            }
            if work.busy > 0 {
                work.waiting += 1;
                work = self.wake.wait(work).unwrap();
                work.waiting -= 1;
                continue;
            }
            if let Some(root) = work.roots.pop_front() {
                work.tasks.push(Task {
                    node: NONE,
                    path: root,
                    weigh: false,
                });
                continue;
            }
            if !work.later.is_empty() {
                let later = std::mem::take(&mut work.later);
                work.tasks = later;
                continue;
            }
            work.done = true;
            self.wake.notify_all();
            return None;
        }
    }

    fn list(&self, task: Task) {
        let Ok(entries) = list_dir(&task.path) else {
            let mut tree = self.index.write();
            let node = match task.node {
                NONE => tree.ensure(&task.path),
                node => node,
            };
            tree.nodes[node as usize].state |= VISITED;
            return;
        };

        let mut sealed = vec![false; entries.len()];
        let mut dirs = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            if entry.kind != EntryKind::Directory {
                continue;
            }
            let path = task.path.join(&entry.name);
            let class = class_of(&path, entry, &self.home);
            sealed[i] = class != Class::Descend;
            dirs.push((i, path, class));
        }
        let node = match task.node {
            NONE => self.index.write().ensure(&task.path),
            node => node,
        };
        let ids = super::relist(&self.index, node, &entries, &sealed);
        {
            let mut tree = self.index.write();
            for (i, _, class) in &dirs {
                if *class != Class::Descend {
                    tree.nodes[ids[*i] as usize].size = 0;
                }
            }
        }
        let mut tasks = Vec::with_capacity(dirs.len());
        for (i, path, class) in dirs {
            if class == Class::Seal {
                continue;
            }
            tasks.push(Task {
                node: ids[i],
                path,
                weigh: class == Class::Weigh,
            });
        }
        let bytes: u64 = entries.iter().map(|e| e.size).sum();
        let files = entries.iter().filter(|e| !e.is_dir()).count() as u64;
        self.dirs.fetch_add(1, Ordering::Relaxed);
        self.files.fetch_add(files, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        self.push(tasks);
        self.report(Some(task.path));
    }

    fn weigh(&self, task: Task) {
        let Ok(entries) = list_dir(&task.path) else {
            return;
        };
        let mut bytes = 0;
        let mut tasks = Vec::new();
        for entry in &entries {
            if entry.kind != EntryKind::Directory {
                bytes += entry.size;
                continue;
            }
            let path = task.path.join(&entry.name);
            if class_of(&path, entry, &self.home) != Class::Seal {
                tasks.push(Task {
                    node: task.node,
                    path,
                    weigh: true,
                });
            }
        }
        if bytes > 0 {
            self.index.write().add_size(task.node, bytes);
            self.bytes.fetch_add(bytes, Ordering::Relaxed);
        }
        self.push(tasks);
        self.report(None);
    }

    fn push(&self, tasks: Vec<Task>) {
        if tasks.is_empty() {
            return;
        }
        let mut work = self.work.lock().unwrap();
        for task in tasks.into_iter().rev() {
            match task.weigh {
                true => work.later.push(task),
                false => work.tasks.push(task),
            }
        }
        if work.waiting > 0 {
            self.wake.notify_all();
        }
    }

    fn report(&self, current: Option<PathBuf>) {
        let now = self.start.elapsed().as_millis() as u64;
        let last = self.last_report.load(Ordering::Relaxed);
        if now < last + REPORT_EVERY_MS {
            return;
        }
        if self
            .last_report
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        self.send(current, false);
    }

    fn send(&self, current: Option<PathBuf>, done: bool) {
        let progress = Progress {
            dirs: self.dirs.load(Ordering::Relaxed),
            files: self.files.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            current,
            done,
        };
        (self.report.lock().unwrap())(progress);
    }

    fn finish(&self) {
        {
            let mut tree = self.index.write();
            tree.recompute();
            tree.crawled_at = Some(SystemTime::now());
            for root in &self.walk.record {
                tree.set_root(root, self.walk.since, modified(root));
            }
            tree.event_id = tree
                .roots
                .iter()
                .map(|root| root.event_id)
                .min()
                .unwrap_or(self.walk.since);
        }
        self.index.crawling.store(false, Ordering::SeqCst);
        self.index.save();
        self.send(None, true);
    }
}

pub fn modified(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map_or(super::store::NO_TIME, super::store::secs)
}

fn class_of(path: &Path, entry: &Entry, home: &Path) -> Class {
    if is_symlink(path)
        || elsewhere(path)
        || guarded(path)
        || private(path, home)
        || sandboxed(path, home)
        || simulator(path)
    {
        return Class::Seal;
    }
    if entry.package || weigh_only(path, home) {
        return Class::Weigh;
    }
    Class::Descend
}

pub fn off_limits(path: &Path, home: &Path) -> bool {
    elsewhere(path)
        || guarded(path)
        || private(path, home)
        || sandboxed(path, home)
        || simulator(path)
}

pub fn sealed(path: &Path, entry: &Entry, home: &Path) -> bool {
    entry.kind == EntryKind::Directory && class_of(path, entry, home) != Class::Descend
}

/// Opening a photo or media library's package puts a privacy prompt on the user's screen.
fn guarded(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        matches!(
            extension.as_bytes(),
            b"photoslibrary"
                | b"aplibrary"
                | b"migratedphotolibrary"
                | b"musiclibrary"
                | b"tvlibrary"
        )
    })
}

/// An iOS Simulator device: a UUID-named folder holding `device.plist`, whose `data` sits behind
/// the media-library prompt macOS puts on the user's own home.
fn simulator(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().as_bytes();
    let uuid_shaped = name.len() == 36
        && name.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        });
    uuid_shaped && path.join("device.plist").is_file()
}

/// Walking `~/Library` raises the media-library prompt and a blocked-access notice for
/// every sandboxed app.
fn sandboxed(path: &Path, home: &Path) -> bool {
    in_a_home(path, home) && path.file_name().unwrap_or_default().as_bytes() == b"Library"
}

/// The folders macOS prompts about on first access, crawled only as roots, once let in.
fn private(path: &Path, home: &Path) -> bool {
    in_a_home(path, home)
        && matches!(
            path.file_name().unwrap_or_default().as_bytes(),
            b"Desktop" | b"Documents" | b"Downloads" | b"Pictures" | b"Music" | b"Movies"
        )
}

/// A guarded read blocks in `opendir` behind a consent prompt no one answers.
fn in_a_home(path: &Path, home: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    (!home.as_os_str().is_empty() && parent == home) || parent.parent() == Some(Path::new("/Users"))
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

fn weigh_only(path: &Path, home: &Path) -> bool {
    let name = path.file_name().unwrap_or_default();
    if matches!(
        name.as_bytes(),
        b".Spotlight-V100" | b".fseventsd" | b".Trashes" | b".DocumentRevisions-V100"
    ) {
        return true;
    }
    !home.as_os_str().is_empty() && path == home.join(".Trash")
}

/// The firmlinked volume roots (`/System/Volumes/Data` is `/` again), devices, the swap files,
/// and anything mounted over a network.
fn elsewhere(path: &Path) -> bool {
    if matches!(
        path.as_os_str().as_bytes(),
        b"/System/Volumes" | b"/dev" | b"/private/var/vm" | b"/Volumes/.timemachine"
    ) {
        return true;
    }
    path.parent() == Some(Path::new("/Volumes")) && !is_local(path)
}

fn is_local(path: &Path) -> bool {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(path.as_ptr(), &mut stat) } != 0 {
        return false;
    }
    stat.f_flags & libc::MNT_LOCAL as u32 != 0
}
