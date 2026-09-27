mod catchup;
mod crawl;
mod lookup;
mod search;
mod snapshot;
mod store;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fs::{File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant, SystemTime};

use crate::entry::{Entry, EntryKind};
use crate::list::list_dir;
use crate::watch::Replay;
use crawl::Walk;
use store::{COMPLETE, EXACT, HIDDEN, LISTED, PACKAGE, Tree, VISITED};

pub use crawl::off_limits;

const BATCH: usize = 4096;
const YIELDS: usize = 64;
/// Long enough for an instance that is quitting as this one launches to let go of the store.
const CLAIM_WAIT: Duration = Duration::from_millis(500);
/// A folder whose relist cost more than this waits `COOLDOWN` times that cost before the next,
/// so one big folder under constant change holds at most a twentieth of a core. A rescan's check
/// cools its roots the same way: macOS rescans a whole root each time it drops events.
const COSTLY: Duration = Duration::from_millis(10);
const COOLDOWN: u32 = 20;
const MIN_GAP: Duration = Duration::from_secs(2);
/// Room for coarse timestamps when finding the folders modified since the tree was in sync.
const SYNC_MARGIN: Duration = Duration::from_secs(2);
/// A snapshot can be saved while the tree trails the disk, so its catch-up looks this far back.
const SAVE_GAP: Duration = Duration::from_secs(3600);
/// fseventsd answers a history query only after several milliseconds.
const PROBE_WAIT: Duration = Duration::from_millis(25);

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

/// What the live follower still owes the tree: folders to walk, folders put off while they cool
/// down, and the last event it applied since the one it started from.
#[derive(Default)]
struct Pending {
    roots: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
    from: u64,
    applied: u64,
    advanced: u64,
    later: HashMap<PathBuf, Instant>,
    cooling: HashMap<PathBuf, Instant>,
    rescans: Vec<PathBuf>,
    checked: HashMap<PathBuf, Instant>,
    rechecks: HashMap<PathBuf, Instant>,
    synced: Option<SystemTime>,
}

pub struct Index {
    store: PathBuf,
    tree: RwLock<Tree>,
    lookup: RwLock<lookup::Lookup>,
    /// Readers queued on `tree`: a writer yields to them, or a stream of writes starves them.
    waiting: AtomicUsize,
    crawling: AtomicBool,
    /// A rescan's check runs apart from crawling, so new folders keep landing while it reads.
    checking: AtomicBool,
    pending: Mutex<Pending>,
    /// Only the store's owner walks whole roots and writes the snapshot; any other instance
    /// reads it and follows the disk live.
    owner: bool,
    _lock: Option<File>,
    dirty: AtomicBool,
    saving: AtomicBool,
    writing: Mutex<()>,
    saved: Option<SystemTime>,
}

impl Index {
    pub fn open(store: &Path) -> Arc<Index> {
        let (owner, lock) = claim(store);
        Arc::new(Index {
            store: store.to_path_buf(),
            tree: RwLock::new(snapshot::load(store).unwrap_or_default()),
            lookup: RwLock::new(lookup::Lookup::default()),
            waiting: AtomicUsize::new(0),
            crawling: AtomicBool::new(false),
            checking: AtomicBool::new(false),
            pending: Mutex::default(),
            owner,
            _lock: lock,
            dirty: AtomicBool::new(false),
            saving: AtomicBool::new(false),
            writing: Mutex::new(()),
            saved: std::fs::metadata(store)
                .and_then(|meta| meta.modified())
                .ok(),
        })
    }

    pub fn crawl(
        self: &Arc<Self>,
        roots: Vec<PathBuf>,
        progress: impl Fn(Progress) + Send + 'static,
    ) {
        if !self.owner {
            return progress(Progress {
                done: true,
                ..Progress::default()
            });
        }
        if self.crawling.swap(true, Ordering::SeqCst) {
            return;
        }
        self.pending.lock().unwrap().synced = Some(SystemTime::now());
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
        self.pending.lock().unwrap().synced = Some(SystemTime::now());
        catchup::spawn(self, roots, progress);
    }

    pub fn children(&self, dir: &Path) -> Option<Vec<Entry>> {
        let tree = self.read();
        let id = listed_dir(&tree, dir)?;
        Some(tree.children(id).map(|child| tree.entry(child)).collect())
    }

    /// A listed folder's modification time as the index recorded it, in whole seconds.
    pub fn modified(&self, dir: &Path) -> Option<SystemTime> {
        let tree = self.read();
        let node = tree.nodes[listed_dir(&tree, dir)? as usize];
        (node.modified != store::NO_TIME).then(|| store::time(node.modified))
    }

    /// `children`, while the index holds every change FSEvents has for the folder's entries and
    /// its folders' own. A write to a file held open reports only when it closes.
    pub fn current_children(&self, dir: &Path) -> Option<Vec<Entry>> {
        let real = crate::watch::resolved(dir);
        catchup::open_to_us(&real, &crawl::home()).then_some(())?;
        let (from, applied) = {
            let pending = self.pending.lock().unwrap();
            let at = |path: &PathBuf| path == &real || path.parent() == Some(real.as_path());
            let owed = self.checking.load(Ordering::SeqCst)
                || pending.later.keys().any(at)
                || pending
                    .rescans
                    .iter()
                    .chain(pending.rechecks.keys())
                    .any(|path| at(path) || real.starts_with(path));
            let followed = pending.roots.iter().any(|root| real.starts_with(root));
            (followed && !owed).then_some((pending.from, pending.applied))?
        };
        let tree = self.read();
        let since = tree
            .roots
            .iter()
            .filter(|root| root.event_id >= from && real.starts_with(&root.path))
            .map(|root| root.event_id.max(applied))
            .max()?;
        let id = listed_dir(&tree, &real)?;
        (tree.nodes[id as usize].state & EXACT != 0).then_some(())?;
        let children = tree.children(id).map(|child| tree.entry(child)).collect();
        drop(tree);
        (!crate::watch::changed_since(&real, since, PROBE_WAIT)).then_some(children)
    }

    pub fn visible_children(&self, dir: &Path) -> Option<usize> {
        let tree = self.read();
        // Asked while a frame draws, so never of the disk: a share that stopped answering would stall it.
        visible(&tree, tree.find(dir)?)
    }

    pub fn folder_size(&self, dir: &Path) -> Option<u64> {
        let tree = self.read();
        let node = tree.nodes[resolve(&tree, dir)? as usize];
        (node.is_dir() && node.state & COMPLETE != 0).then_some(node.size)
    }

    /// `folder_size` and `visible_children` of every folder in `dir`, by name, in one pass.
    pub fn folders_in(&self, dir: &Path) -> HashMap<String, (Option<u64>, Option<usize>)> {
        let tree = self.read();
        let named = tree
            .find(dir)
            .filter(|&id| tree.nodes[id as usize].is_dir());
        let Some(id) = named.or_else(|| tree.find(&std::fs::canonicalize(dir).ok()?)) else {
            return HashMap::new();
        };
        tree.children(id)
            .filter(|&child| tree.nodes[child as usize].is_dir())
            .map(|child| {
                let node = tree.nodes[child as usize];
                let name = tree.names.bytes(tree.name_of[child as usize]);
                let size = (node.state & COMPLETE != 0).then_some(node.size);
                let seen = named.and_then(|_| visible(&tree, child));
                (String::from_utf8_lossy(name).into_owned(), (size, seen))
            })
            .collect()
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
        let now = Instant::now();
        for dir in dirs {
            if !catchup::open_to_us(dir, &home) {
                continue;
            }
            {
                let mut pending = self.pending.lock().unwrap();
                if let Some(until) = pending
                    .cooling
                    .get(dir)
                    .copied()
                    .filter(|until| *until > now)
                {
                    pending.later.insert(dir.clone(), until);
                    continue;
                }
            }
            let started = Instant::now();
            self.refresh(dir, &home);
            self.cool(dir.clone(), started.elapsed());
        }
    }

    fn cool(&self, dir: PathBuf, took: Duration) {
        // A folder written many times a second is read again at most once per gap.
        let until = Instant::now() + (took * COOLDOWN).max(MIN_GAP);
        self.pending.lock().unwrap().cooling.insert(dir, until);
    }

    pub fn follow(self: &Arc<Self>, roots: Vec<PathBuf>) -> crate::watch::Watcher {
        let from = crate::watch::latest_event_id();
        {
            let mut pending = self.pending.lock().unwrap();
            pending.roots = roots.iter().map(|root| crawl::real(root)).collect();
            pending.from = from;
        }
        let index = self.clone();
        crate::watch::since(from, roots, move |replay| index.apply(replay))
    }

    /// Whether it put work off until a folder cools down or a check comes due.
    fn apply(self: &Arc<Self>, replay: Replay) -> bool {
        let home = crawl::home();
        let open = |dir: &PathBuf| catchup::open_to_us(dir, &home);
        let rescans: Vec<PathBuf> = replay.rescan.into_iter().filter(open).collect();
        if std::env::var_os("NG_CRAWL_TRACE").is_some() {
            for dir in &rescans {
                eprintln!("rescan {}", dir.display());
            }
        }
        let mut deeper = Vec::new();
        let mut dirs: Vec<PathBuf> = replay.dirs.into_iter().filter(open).collect();
        let now = Instant::now();
        {
            let mut pending = self.pending.lock().unwrap();
            let due: Vec<PathBuf> = pending
                .later
                .iter()
                .filter(|(_, at)| **at <= now)
                .map(|(dir, _)| dir.clone())
                .collect();
            for dir in due {
                pending.later.remove(&dir);
                dirs.push(dir);
            }
            let rechecks: Vec<PathBuf> = pending
                .rechecks
                .iter()
                .filter(|(_, at)| **at <= now)
                .map(|(dir, _)| dir.clone())
                .collect();
            for dir in rechecks {
                pending.rechecks.remove(&dir);
                pending.rescans.push(dir);
            }
            pending.cooling.retain(|_, until| *until > now);
            dirs.sort_unstable();
            dirs.dedup();
            dirs.retain(|dir| match pending.cooling.get(dir).copied() {
                Some(until) => {
                    pending.later.insert(dir.clone(), until);
                    false
                }
                None => true,
            });
        }
        for dir in dirs {
            let started = Instant::now();
            deeper.extend(self.refresh(&dir, &home));
            self.cool(dir, started.elapsed());
        }
        let mut pending = self.pending.lock().unwrap();
        pending.dirs.extend(deeper);
        pending.rescans.extend(rescans);
        pending.applied = pending.applied.max(replay.event_id);
        drop(pending);
        self.walk_pending();
        let pending = self.pending.lock().unwrap();
        !pending.later.is_empty() || !pending.rechecks.is_empty()
    }

    /// With nothing left to walk, every followed root is current to the last event applied, so
    /// the next launch catches up only on what happened while it was closed.
    fn walk_pending(self: &Arc<Self>) {
        loop {
            if self.crawling.swap(true, Ordering::SeqCst) {
                return;
            }
            let mut pending = self.pending.lock().unwrap();
            let now = Instant::now();
            pending.checked.retain(|_, until| *until > now);
            let mut check = Vec::new();
            for dir in std::mem::take(&mut pending.rescans) {
                match pending.checked.get(&dir).copied() {
                    Some(until) => {
                        pending.rechecks.insert(dir, until);
                    }
                    None => check.push(dir),
                }
            }
            if !check.is_empty() {
                match self.checking.swap(true, Ordering::SeqCst) {
                    true => pending.rescans.extend(check),
                    false => {
                        let since = pending
                            .synced
                            .map_or(std::time::UNIX_EPOCH, |at| at - SYNC_MARGIN);
                        let index = self.clone();
                        std::thread::spawn(move || {
                            crate::utility_qos();
                            let started = Instant::now();
                            let deeper = index.verify(&check, since, &crawl::home());
                            if std::env::var_os("NG_CRAWL_TRACE").is_some() {
                                eprintln!(
                                    "check {:?} took {} ms deeper {}",
                                    check,
                                    started.elapsed().as_millis(),
                                    deeper.len()
                                );
                            }
                            index.cool_check(&check, started.elapsed());
                            index.pending.lock().unwrap().dirs.extend(deeper);
                            index.checking.store(false, Ordering::SeqCst);
                            index.walk_pending();
                        });
                    }
                }
            }
            if !pending.dirs.is_empty() {
                let crawl = outermost(std::mem::take(&mut pending.dirs));
                drop(pending);
                let walk = Walk {
                    crawl,
                    record: Vec::new(),
                    since: 0,
                };
                return crawl::spawn(self, walk, |_| {});
            }
            // A folder put off until it cools down, or a check still reading, still owes its change.
            let settled = pending.later.is_empty()
                && pending.rechecks.is_empty()
                && pending.rescans.is_empty()
                && !self.checking.load(Ordering::SeqCst)
                && pending.applied > pending.advanced;
            let (roots, applied, from) = (pending.roots.clone(), pending.applied, pending.from);
            if settled {
                pending.advanced = applied;
                pending.synced = Some(SystemTime::now());
            }
            drop(pending);
            if settled {
                self.write().advance(&roots, applied, from);
            }
            self.crawling.store(false, Ordering::SeqCst);
            let pending = self.pending.lock().unwrap();
            let checking = self.checking.load(Ordering::SeqCst);
            if pending.dirs.is_empty() && (pending.rescans.is_empty() || checking) {
                return;
            }
        }
    }

    fn cool_check(&self, roots: &[PathBuf], took: Duration) {
        if took <= COSTLY {
            return;
        }
        let until = Instant::now() + took * COOLDOWN;
        let mut pending = self.pending.lock().unwrap();
        for root in roots {
            pending.checked.insert(root.clone(), until);
        }
    }

    /// After dropped events, the folders read again are the ones modified since `since`: an added,
    /// removed or renamed entry moves its folder's time, new contents in a file do not.
    fn verify(&self, roots: &[PathBuf], since: SystemTime, home: &Path) -> Vec<PathBuf> {
        let dirs = self.read().listed_under(&outermost(roots.to_vec()));
        {
            let mut tree = self.write();
            for &(id, _) in &dirs {
                tree.nodes[id as usize].state &= !EXACT;
            }
        }
        let (known, mut refreshed) = (dirs.len(), 0);
        let mut deeper = Vec::new();
        for (_, dir) in dirs {
            let changed = std::fs::symlink_metadata(&dir)
                .and_then(|meta| meta.modified())
                .map_or(true, |at| at >= since);
            if changed && catchup::open_to_us(&dir, home) {
                refreshed += 1;
                deeper.extend(self.refresh(&dir, home));
            }
        }
        if std::env::var_os("NG_CRAWL_TRACE").is_some() {
            eprintln!("verify {known} known folders, {refreshed} read again");
        }
        deeper
    }

    fn refresh(&self, dir: &Path, home: &Path) -> Vec<PathBuf> {
        let started = Instant::now();
        let deeper = self.relisted(dir, home);
        if std::env::var_os("NG_CRAWL_TRACE").is_some() {
            eprintln!(
                "refresh {} us {} deeper {}",
                started.elapsed().as_micros(),
                deeper.len(),
                dir.display()
            );
        }
        deeper
    }

    fn relisted(&self, dir: &Path, home: &Path) -> Vec<PathBuf> {
        let Some(id) = resolve(&self.read(), dir) else {
            return Vec::new();
        };
        let entries = match list_dir(dir) {
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
        let own = crate::list::entry_at(dir).ok();
        let ids = relist(self, id, &entries, &sealed);
        let mut tree = self.write();
        // FSEvents names only this folder when an entry moves its time, which its parent lists.
        if let Some(own) = own.filter(|own| own.kind == EntryKind::Directory) {
            tree.write(id, &own, false);
        }
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
        if !self.owner {
            return;
        }
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

/// A burst of dropped events asks for the same subtree again and again; one walk covers them all.
fn outermost(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    dirs.sort_unstable();
    let mut kept: Vec<PathBuf> = Vec::with_capacity(dirs.len());
    for dir in dirs {
        if !kept.last().is_some_and(|outer| dir.starts_with(outer)) {
            kept.push(dir);
        }
    }
    kept
}

/// A store whose lock can't be created can't be saved either, so it is walked in memory alone.
fn claim(store: &Path) -> (bool, Option<File>) {
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(store.with_extension("lock"));
    let Ok(lock) = lock else {
        return (true, None);
    };
    let deadline = Instant::now() + CLAIM_WAIT;
    loop {
        match lock.try_lock() {
            Ok(()) => return (true, Some(lock)),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(TryLockError::WouldBlock) => return (false, None),
            Err(TryLockError::Error(_)) => return (true, None),
        }
    }
}

fn listed_dir(tree: &Tree, dir: &Path) -> Option<u32> {
    listed(tree, resolve(tree, dir)?)
}

fn listed(tree: &Tree, id: u32) -> Option<u32> {
    let node = tree.nodes[id as usize];
    (node.is_dir() && node.flags & PACKAGE == 0 && node.state & LISTED != 0).then_some(id)
}

fn visible(tree: &Tree, id: u32) -> Option<usize> {
    let id = listed(tree, id)?;
    let shown = |child: &u32| tree.nodes[*child as usize].flags & HIDDEN == 0;
    Some(tree.children(id).filter(shown).count())
}
