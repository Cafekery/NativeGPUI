//! The listings the demo has read, and the reads a frame asked for and did not have.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use super::paths::{intern, intern_path};
use super::row::Row;
use super::volumes::volume_of;

/// How many listings stay in memory behind the ones a frame is drawing.
const CACHED: usize = 64;

pub(crate) struct Folder {
    pub(crate) title: &'static str,
    pub(crate) parent: Option<&'static str>,
    /// Finder's path bar: the volume's name, then every folder down to this one.
    pub(crate) crumbs: Vec<&'static str>,
    pub(crate) rows: Vec<Arc<Row>>,
    /// A re-read's rows as they arrive, while `rows` keeps what is on screen.
    staged: Option<Vec<Arc<Row>>>,
    /// Rows are still arriving from the disk.
    pub(crate) loading: bool,
}

/// The folder at `id`, named and placed the way Finder names and places it, with no rows yet.
pub(crate) fn folder_of(id: &'static str) -> Folder {
    let (mount, volume) = volume_of(id);
    let rest = id.strip_prefix(mount.trim_end_matches('/')).unwrap_or("");
    let mut crumbs = vec![volume];
    crumbs.extend(rest.split('/').filter(|part| !part.is_empty()).map(intern));
    Folder {
        title: crumbs.last().copied().unwrap_or(volume),
        parent: Path::new(id).parent().map(intern_path),
        crumbs,
        rows: Vec::new(),
        staged: None,
        loading: true,
    }
}

/// The listings the demo has read, newest last, with the misses a render wants read next.
#[derive(Default)]
pub(crate) struct Folders {
    cache: HashMap<&'static str, Folder>,
    order: Vec<&'static str>,
    reading: HashSet<&'static str>,
    wanted: RefCell<Vec<&'static str>>,
    /// The listings a frame draws from: however many there are, they stay.
    pinned: HashSet<&'static str>,
    changed: u64,
}

impl Folders {
    /// The cached listing. A miss asks for a read at the next pump and draws nothing until it lands.
    pub(crate) fn get(&self, id: &'static str) -> Option<&Folder> {
        if let Some(folder) = self.cache.get(id) {
            return Some(folder);
        }
        if id.starts_with('/') && !self.reading.contains(id) {
            let mut wanted = self.wanted.borrow_mut();
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
        None
    }

    pub(crate) fn get_mut(&mut self, id: &'static str) -> Option<&mut Folder> {
        self.changed += 1;
        self.cache.get_mut(id)
    }

    /// Bumped by every change to any listing, so what a frame built from them knows to rebuild.
    pub(crate) fn changed(&self) -> u64 {
        self.changed
    }

    /// The listings on screen. Evicting one a frame is drawing would blank it and read it again.
    pub(crate) fn pin(&mut self, ids: impl IntoIterator<Item = &'static str>) {
        self.pinned = ids.into_iter().collect();
    }

    /// Whether the listing is in memory, without asking for it.
    pub(crate) fn has(&self, id: &'static str) -> bool {
        self.cache.contains_key(id)
    }

    /// Whether a re-read of `id` is still gathering its rows beside the ones on screen.
    pub(crate) fn staging(&self, id: &'static str) -> bool {
        self.cache
            .get(id)
            .is_some_and(|folder| folder.staged.is_some())
    }

    pub(crate) fn len(&self) -> usize {
        self.cache.len()
    }

    pub(crate) fn take_wanted(&mut self) -> Vec<&'static str> {
        self.wanted.take()
    }

    /// Claims a read of `id`, unless one is already running.
    pub(crate) fn start(&mut self, id: &'static str) -> bool {
        self.reading.insert(id)
    }

    pub(crate) fn finish(&mut self, id: &'static str) {
        self.reading.remove(id);
        self.changed += 1;
        if let Some(folder) = self.cache.get_mut(id) {
            folder.loading = false;
            if let Some(staged) = folder.staged.take() {
                folder.rows = staged;
            }
        }
    }

    /// Rows from a read: the first batch stands in for whatever was there, the rest add to it. A
    /// listing that already has rows keeps them until the whole re-read is in hand.
    pub(crate) fn deliver(&mut self, id: &'static str, rows: Vec<Arc<Row>>, replace: bool) {
        self.changed += 1;
        let folder = self.cache.entry(id).or_insert_with(|| {
            self.order.push(id);
            folder_of(id)
        });
        if replace {
            folder.staged = (!folder.rows.is_empty()).then(Vec::new);
        }
        match &mut folder.staged {
            Some(staged) => staged.extend(rows),
            None => folder.rows.extend(rows),
        }
        self.evict(id);
    }

    /// Drops the oldest listings nothing is drawing or reading, back to [`CACHED`].
    fn evict(&mut self, keep: &'static str) {
        let mut over = self.order.len().saturating_sub(CACHED);
        if over == 0 {
            return;
        }
        let order = std::mem::take(&mut self.order);
        self.order = order
            .into_iter()
            .filter(|id| {
                let drop = over > 0
                    && *id != keep
                    && !self.pinned.contains(id)
                    && !self.reading.contains(id);
                if drop {
                    over -= 1;
                    self.cache.remove(id);
                }
                !drop
            })
            .collect();
    }

    pub(crate) fn cached(&self) -> Vec<&'static str> {
        self.order.clone()
    }
}
