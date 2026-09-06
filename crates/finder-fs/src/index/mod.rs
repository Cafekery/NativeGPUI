//! The whole-Mac index: every reachable directory read once, kept current by FSEvents, persisted
//! between launches, and answering the questions a listing alone cannot — a folder's recursive
//! size, a name search across the disk, a listing before the disk has been asked.
//!
//! Contract:
//! - `Index::open` loads the last snapshot from `store` in well under a second for ~2M entries.
//! - `crawl` walks `roots` in order on background threads with `list::list_dir`, skipping what
//!   it cannot read, and reports progress; a second call resumes rather than restarts.
//! - `children`/`folder_size`/`search` answer from memory in milliseconds.
//! - `note_changed` re-reads the named directories (the watcher's job to call).
//! - `sync` is the launch entry: it replays FSEvents since the snapshot instead of walking again.

mod catchup;
mod crawl;
mod search;
mod snapshot;
mod store;
#[cfg(test)]
mod tests;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::SystemTime;

use crate::entry::Entry;
use crate::list::list_dir;
use crawl::Walk;
use store::{COMPLETE, LISTED, PACKAGE, Tree, VISITED};

/// Entries written under one lock: a 20 000-file directory costs a reader a few milliseconds.
const BATCH: usize = 4096;
/// How long a writer stands aside for a waiting reader before taking its turn anyway.
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
    /// Higher is better: prefix beats word start beats substring; shallower beats deeper.
    pub score: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub dirs: u64,
    pub files: u64,
    pub bytes: u64,
    pub crawled_at: Option<SystemTime>,
    /// The FSEvents id the snapshot is current to, for catching up on launch.
    pub event_id: u64,
}

pub struct Index {
    store: PathBuf,
    tree: RwLock<Tree>,
    /// Readers waiting. A stream of crawl threads taking the write lock starves a reader of
    /// `RwLock` indefinitely, so a writer waits its turn behind anyone already asking to read.
    waiting: AtomicUsize,
    crawling: AtomicBool,
}

impl Index {
    pub fn open(store: &Path) -> Arc<Index> {
        Arc::new(Index {
            store: store.to_path_buf(),
            tree: RwLock::new(snapshot::load(store).unwrap_or_default()),
            waiting: AtomicUsize::new(0),
            crawling: AtomicBool::new(false),
        })
    }

    /// Walks `roots` in order until every subtree is in; a call made while one runs is ignored.
    /// Packages and skipped trees (caches, trash, volume metadata) are weighed for their bytes
    /// last, so the entries the user can see are known first.
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

    /// The launch walk: roots the snapshot already holds are brought up to date from the FSEvents
    /// log instead of read again, and only what the log names, or has never been seen, is crawled.
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

    /// The directory's entries as the index last read them, or `None` if it has not read it.
    pub fn children(&self, dir: &Path) -> Option<Vec<Entry>> {
        let tree = self.read();
        let id = resolve(&tree, dir)?;
        let node = tree.nodes[id as usize];
        if !node.is_dir() || node.flags & PACKAGE != 0 || node.state & LISTED == 0 {
            return None;
        }
        Some(tree.children(id).map(|child| tree.entry(child)).collect())
    }

    /// Recursive logical bytes, once the whole subtree has been crawled.
    pub fn folder_size(&self, dir: &Path) -> Option<u64> {
        let tree = self.read();
        let node = tree.nodes[resolve(&tree, dir)? as usize];
        (node.is_dir() && node.state & COMPLETE != 0).then_some(node.size)
    }

    /// Name search, optionally scoped `under` a directory, best hits first.
    pub fn search(&self, query: &str, under: Option<&Path>, limit: usize) -> Vec<Hit> {
        let tree = self.read();
        let under = match under {
            Some(dir) => match resolve(&tree, dir) {
                Some(id) => Some(id),
                None => return Vec::new(),
            },
            None => None,
        };
        search::search(&tree, query, under, limit)
    }

    /// Re-reads exactly these directories and carries the size change up to the roots.
    pub fn note_changed(&self, dirs: &[PathBuf]) {
        let home = crawl::home();
        for dir in dirs {
            self.refresh(dir, &home);
        }
    }

    /// One directory read again, answering with the subdirectories the index has never been into
    /// and so still owes a walk.
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

    /// Writes the snapshot to a temp file and renames it over `store`.
    pub fn save(&self) {
        let _ = snapshot::save(&self.read(), &self.store);
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
        for _ in 0..YIELDS {
            if self.waiting.load(Ordering::Acquire) == 0 {
                break;
            }
            std::thread::yield_now();
        }
        self.tree.write().unwrap_or_else(|held| held.into_inner())
    }
}

/// A directory's children, written in batches so no reader waits on a big one.
fn relist(index: &Index, dir: u32, entries: &[Entry], sealed: &[bool]) -> Vec<u32> {
    let mut state = index.read().start_relist(dir);
    for (batch, sealed) in entries.chunks(BATCH).zip(sealed.chunks(BATCH)) {
        index.write().relist(&mut state, batch, sealed);
    }
    index.write().end_relist(state)
}

/// Paths reach the index as the crawl canonicalized them, so a symlinked path gets one retry.
fn resolve(tree: &Tree, path: &Path) -> Option<u32> {
    tree.find(path)
        .or_else(|| tree.find(&std::fs::canonicalize(path).ok()?))
}
