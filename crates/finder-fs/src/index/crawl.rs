use std::collections::VecDeque;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Instant, SystemTime};

use super::store::{NO_TIME, NONE, VISITED, secs};
use super::{Index, Progress};
use crate::entry::{Entry, EntryKind};
use crate::list::list_dir;
use crate::volumes::is_local;

const THREADS: usize = 6;
const REPORT_EVERY_MS: u64 = 100;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Descend,
    Weigh,
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
}

struct Crawl {
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
        home: super::home(),
        walk,
    });
    for _ in 0..THREADS {
        let crawl = crawl.clone();
        std::thread::spawn(move || {
            crate::utility_qos();
            crawl.run()
        });
    }
}

static TRACE_START: LazyLock<Instant> = LazyLock::new(Instant::now);

impl Crawl {
    fn run(&self) {
        while let Some(task) = self.next() {
            trace!(
                "crawl {} {} {}",
                TRACE_START.elapsed().as_millis(),
                if task.weigh { "W" } else { "L" },
                task.path.display()
            );
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
                work.tasks = std::mem::take(&mut work.later);
                continue;
            }
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
        let ids = self.index.relist(node, &entries, &sealed);
        if sealed.contains(&true) {
            let mut tree = self.index.write();
            for (i, _, class) in &dirs {
                if *class != Class::Descend {
                    tree.nodes[ids[*i] as usize].size = 0;
                }
            }
        }
        let mut tasks = Vec::with_capacity(dirs.len());
        for (i, path, class) in dirs {
            if class == Class::Seal || self.walk.crawl.contains(&path) {
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
            self.index.write().nodes[task.node as usize].size += bytes;
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
        // A walk the follower queued lands with the periodic save, not a whole snapshot per folder.
        if !self.walk.record.is_empty() {
            self.index.save();
        }
        self.send(None, true);
        self.index.walk_pending();
    }
}

pub fn modified(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map_or(NO_TIME, secs)
}

fn class_of(path: &Path, entry: &Entry, home: &Path) -> Class {
    if off_limits(path, home) {
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

/// Reading a TCC-gated folder again unasked puts its prompt back on screen.
pub fn open_to_us(path: &Path, home: &Path) -> bool {
    !path.ancestors().any(|part| off_limits(part, home))
}

pub fn sealed(dir: &Path, entry: &Entry, home: &Path) -> bool {
    entry.kind == EntryKind::Directory
        && class_of(&dir.join(&entry.name), entry, home) != Class::Descend
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

/// An iOS Simulator device, whose `data` sits behind the media-library prompt on the home.
fn simulator(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().as_bytes();
    let uuid_shaped = name.len() == 36
        && name.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        });
    uuid_shaped && path.join("device.plist").is_file()
}

/// Walking `~/Library` raises the media-library prompt and a notice per sandboxed app.
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

/// Firmlinked volume roots (`/System/Volumes/Data` is `/` again), devices, swap, network mounts.
fn elsewhere(path: &Path) -> bool {
    if matches!(
        path.as_os_str().as_bytes(),
        b"/System/Volumes" | b"/dev" | b"/private/var/vm" | b"/Volumes/.timemachine"
    ) {
        return true;
    }
    path.parent() == Some(Path::new("/Volumes")) && !is_local(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_path_is_never_reopened() {
        let home = PathBuf::from("/Users/tester");
        assert!(open_to_us(&home.join("Projects/thing"), &home));
        assert!(!open_to_us(
            &home.join("Pictures/Library.photoslibrary/db"),
            &home
        ));
        assert!(!open_to_us(&home.join("Desktop/notes"), &home));
        assert!(!open_to_us(&home.join("Library/Caches/x"), &home));
        assert!(!open_to_us(Path::new("/System/Volumes/Data/Users"), &home));
    }

    #[test]
    fn a_home_that_is_not_ours_is_never_opened() {
        let home = PathBuf::from("/Users/tester");
        assert!(!open_to_us(Path::new("/Users/other/Documents"), &home));
        assert!(!open_to_us(
            Path::new("/Users/other/Library/Containers"),
            &home
        ));
        assert!(!open_to_us(Path::new("/Users/other/Music/iTunes"), &home));
        assert!(open_to_us(Path::new("/Users/other/Projects/thing"), &home));
        assert!(open_to_us(Path::new("/Users/other"), &home));
    }
}
