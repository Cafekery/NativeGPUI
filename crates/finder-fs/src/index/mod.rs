mod catchup;
mod crawl;
mod lookup;
mod search;
mod snapshot;
mod store;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant, SystemTime};

use crate::entry::Entry;
use crate::list::list_dir;
use crawl::Walk;
use store::{COMPLETE, HIDDEN, LISTED, PACKAGE, Tree, VISITED};

pub use crawl::off_limits;

const BATCH: usize = 4096;
const YIELDS: usize = 64;

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub dirs: u64,
    pub files: u64,
    pub bytes: u64,
    pub current: Option<PathBuf>,
    pub done: bool,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    pub entry: Entry,
    pub score: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub dirs: u64,
    pub files: u64,
    pub bytes: u64,
    pub crawled_at: Option<SystemTime>,
    pub event_id: u64,
}

pub struct Index {
    store: PathBuf,
    tree: RwLock<Tree>,
    lookup: RwLock<lookup::Lookup>,
    /// Readers queued on `tree`: a writer yields to them, or a stream of writes starves them.
    waiting: AtomicUsize,
    crawling: AtomicBool,
    dirty: AtomicBool,
    saving: AtomicBool,
    writing: Mutex<()>,
}

impl Index {
    pub fn open(store: &Path) -> Arc<Index> {
        Arc::new(Index {
            store: store.to_path_buf(),
            tree: RwLock::new(snapshot::load(store).unwrap_or_default()),
            lookup: RwLock::new(lookup::Lookup::default()),
            waiting: AtomicUsize::new(0),
            crawling: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            saving: AtomicBool::new(false),
            writing: Mutex::new(()),
        })
    }

    pub fn crawl(
        self: &Arc<Self>,
        roots: Vec<PathBuf>,
        progress: impl Fn(Progress) + Send + 'static,
    ) {
        if self.crawling.swap(true, Ordering::SeqCst) {
            return;
        }
        crawl::spawn(
            self,
            Walk {
                crawl: roots.clone(),
                record: roots,
                since: crate::watch::latest_event_id(),
            },
            progress,
        );
    }

    pub fn sync(
        self: &Arc<Self>,
        roots: Vec<PathBuf>,
        progress: impl Fn(Progress) + Send + 'static,
    ) {
        if self.crawling.swap(true, Ordering::SeqCst) {
            return;
        }
        catchup::spawn(self, roots, progress);
    }

    pub fn children(&self, dir: &Path) -> Option<Vec<Entry>> {
        let tree = self.read();
        let id = listed_dir(&tree, dir)?;
        Some(tree.children(id).map(|child| tree.entry(child)).collect())
    }

    pub fn visible_children(&self, dir: &Path) -> Option<usize> {
        let tree = self.read();
        let id = listed_dir(&tree, dir)?;
        Some(
            tree.children(id)
                .filter(|&child| tree.nodes[child as usize].flags & HIDDEN == 0)
                .count(),
        )
    }

    pub fn folder_size(&self, dir: &Path) -> Option<u64> {
        let tree = self.read();
        let node = tree.nodes[resolve(&tree, dir)? as usize];
        (node.is_dir() && node.state & COMPLETE != 0).then_some(node.size)
    }

    pub fn warm(self: &Arc<Self>) {
        let index = self.clone();
        std::thread::spawn(move || {
            let tree = index.read();
            drop(index.lookup(&tree));
        });
    }

    fn lookup(&self, tree: &Tree) -> RwLockReadGuard<'_, lookup::Lookup> {
        let lookup = self.lookup.read().unwrap_or_else(|held| held.into_inner());
        if !lookup.stale(tree) {
            return lookup;
        }
        drop(lookup);
        let mut building = self.lookup.write().unwrap_or_else(|held| held.into_inner());
        if building.stale(tree) {
            *building = lookup::Lookup::build(tree);
        }
        drop(building);
        self.lookup.read().unwrap_or_else(|held| held.into_inner())
    }

    /// One real file per extension, the most common first.
    pub fn extensions(&self, limit: usize) -> Vec<PathBuf> {
        let tree = self.read();
        let mut counts: HashMap<&[u8], (usize, u32)> = HashMap::new();
        for id in 0..tree.name_of.len() as u32 {
            let node = &tree.nodes[id as usize];
            if node.deleted() || node.is_dir() {
                continue;
            }
            let name = tree.names.bytes(tree.name_of[id as usize]);
            let Some(at) = name.iter().rposition(|byte| *byte == b'.') else {
                continue;
            };
            let ext = &name[at + 1..];
            if ext.is_empty() || ext.len() > 12 {
                continue;
            }
            counts.entry(ext).or_insert((0, id)).0 += 1;
        }
        let mut common: Vec<(usize, u32)> = counts.into_values().collect();
        common.sort_unstable_by_key(|found| std::cmp::Reverse(found.0));
        common
            .into_iter()
            .take(limit)
            .map(|(_, id)| tree.path(id))
            .collect()
    }

    pub fn search(&self, query: &str, under: Option<&Path>, limit: usize) -> Vec<Hit> {
        let tree = self.read();
        let under = match under.map(|dir| resolve(&tree, dir)) {
            Some(None) => return Vec::new(),
            under => under.flatten(),
        };
        let lookup = self.lookup(&tree);
        search::search(&tree, &lookup, query, under, limit)
    }

    pub fn note_changed(&self, dirs: &[PathBuf]) {
        let home = crawl::home();
        for dir in dirs {
            if catchup::open_to_us(dir, &home) {
                self.refresh(dir, &home);
            }
        }
    }

    pub fn follow(self: &Arc<Self>, roots: Vec<PathBuf>) -> crate::watch::Watcher {
        let index = self.clone();
        crate::watch::since(crate::watch::latest_event_id(), roots, move |replay| {
            let home = crawl::home();
            let open = |dir: &PathBuf| catchup::open_to_us(dir, &home);
            let mut deeper: Vec<PathBuf> = replay.rescan.into_iter().filter(open).collect();
            for dir in replay.dirs.iter().filter(|dir| open(dir)) {
                deeper.extend(index.refresh(dir, &home));
            }
            if !deeper.is_empty() {
                index.crawl(deeper, |_| {});
            }
        })
    }

    fn refresh(&self, dir: &Path, home: &Path) -> Vec<PathBuf> {
        let listed = list_dir(dir);
        let Some(id) = resolve(&self.read(), dir) else {
            return Vec::new();
        };
        let entries = match listed {
            Ok(entries) => entries,
            Err(error) => {
                if error.kind() == io::ErrorKind::NotFound {
                    let mut tree = self.write();
                    let size = tree.nodes[id as usize].size;
                    tree.tombstone(id);
                    tree.propagate(id, -(size as i64));
                }
                return Vec::new();
            }
        };
        let sealed: Vec<bool> = entries
            .iter()
            .map(|entry| crawl::sealed(&dir.join(&entry.name), entry, home))
            .collect();
        let ids = relist(self, id, &entries, &sealed);
        let mut tree = self.write();
        let before = tree.nodes[id as usize].size;
        let after: u64 = tree
            .children(id)
            .map(|child| tree.nodes[child as usize].size)
            .sum();
        tree.nodes[id as usize].size = after;
        tree.propagate(id, after as i64 - before as i64);
        entries
            .iter()
            .zip(ids)
            .zip(sealed)
            .filter(|((entry, id), sealed)| {
                entry.is_dir() && !sealed && tree.nodes[*id as usize].state & VISITED == 0
            })
            .map(|((entry, _), _)| dir.join(&entry.name))
            .collect()
    }

    pub fn save(&self) {
        let _writing = self.writing.lock().unwrap_or_else(|held| held.into_inner());
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return;
        }
        let started = Instant::now();
        self.saving.store(true, Ordering::Release);
        let written = snapshot::save(&self.read(), &self.store);
        self.saving.store(false, Ordering::Release);
        if written.is_err() {
            self.dirty.store(true, Ordering::Release);
        }
        if std::env::var_os("NG_CRAWL_TRACE").is_some() {
            let bytes = std::fs::metadata(&self.store).map_or(0, |meta| meta.len());
            eprintln!(
                "save {} ms {bytes} {}",
                started.elapsed().as_millis(),
                written.is_ok()
            );
        }
    }

    pub fn settle(&self, budget: Duration) {
        let deadline = Instant::now() + budget;
        while self.saving.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    pub fn stats(&self) -> Stats {
        let tree = self.read();
        Stats {
            dirs: tree.dirs,
            files: tree.files,
            bytes: tree.bytes,
            crawled_at: tree.crawled_at,
            event_id: tree.event_id,
        }
    }

    fn read(&self) -> RwLockReadGuard<'_, Tree> {
        self.waiting.fetch_add(1, Ordering::Acquire);
        let tree = self.tree.read().unwrap_or_else(|held| held.into_inner());
        self.waiting.fetch_sub(1, Ordering::Release);
        tree
    }

    fn write(&self) -> RwLockWriteGuard<'_, Tree> {
        self.dirty.store(true, Ordering::Release);
        // `RwLock` is fair, so a writer queued behind a save puts every later reader behind it too.
        while self.saving.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(1));
        }
        for _ in 0..YIELDS {
            if self.waiting.load(Ordering::Acquire) == 0 {
                break;
            }
            std::thread::yield_now();
        }
        self.tree.write().unwrap_or_else(|held| held.into_inner())
    }
}

fn relist(index: &Index, dir: u32, entries: &[Entry], sealed: &[bool]) -> Vec<u32> {
    let mut state = index.read().start_relist(dir);
    for (batch, sealed) in entries.chunks(BATCH).zip(sealed.chunks(BATCH)) {
        index.write().relist(&mut state, batch, sealed);
    }
    index.write().end_relist(state)
}

fn resolve(tree: &Tree, path: &Path) -> Option<u32> {
    tree.find(path)
        .or_else(|| tree.find(&std::fs::canonicalize(path).ok()?))
}

fn listed_dir(tree: &Tree, dir: &Path) -> Option<u32> {
    let id = resolve(tree, dir)?;
    let node = tree.nodes[id as usize];
    (node.is_dir() && node.flags & PACKAGE == 0 && node.state & LISTED != 0).then_some(id)
}
