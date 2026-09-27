use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use finder_fs::volumes::{Standard, standard};

use super::paths::{intern, intern_path};
use super::row::Row;
use super::volumes::volume_of;
use crate::listing::next_rev;

const CACHED: usize = 64;

pub(crate) struct Folder {
    pub(crate) title: &'static str,
    pub(crate) parent: Option<&'static str>,
    pub(crate) crumbs: Vec<&'static str>,
    pub(crate) rows: Arc<Vec<Arc<Row>>>,
    staged: Option<Vec<Arc<Row>>>,
    pub(crate) loading: bool,
    rev: u64,
    stale: bool,
}

pub(crate) fn folder_of(id: &'static str) -> Folder {
    // Finder roots the Trash's path at "Trash", not at .Trash inside the home folder.
    let trash = standard(Standard::Trash).filter(|trash| Path::new(id).starts_with(trash));
    let (mount, volume) = match trash {
        Some(trash) => (intern_path(&trash), intern(&finder_fs::words::say("Trash"))),
        None => volume_of(id),
    };
    let rest = id.strip_prefix(mount.trim_end_matches('/')).unwrap_or("");
    let mut at = mount.trim_end_matches('/').to_string();
    let mut crumbs = vec![volume];
    crumbs.extend(rest.split('/').filter(|part| !part.is_empty()).map(|part| {
        at = format!("{at}/{part}");
        intern(&finder_fs::name::of_path(Path::new(&at)))
    }));
    Folder {
        title: crumbs.last().copied().unwrap_or(volume),
        parent: Path::new(id).parent().map(intern_path),
        crumbs,
        rows: Arc::default(),
        staged: None,
        loading: true,
        rev: next_rev(),
        stale: false,
    }
}

#[derive(Default)]
pub(crate) struct Folders {
    cache: HashMap<&'static str, Folder>,
    order: Vec<&'static str>,
    reading: HashSet<&'static str>,
    owed: HashSet<&'static str>,
    wanted: RefCell<Vec<&'static str>>,
    pinned: HashSet<&'static str>,
    changed: u64,
    grew: u64,
}

impl Folders {
    /// A miss asks for a read at the next pump and draws nothing until it lands; a folder that
    /// changed while nobody showed it draws as it was and is read again.
    pub(crate) fn get(&self, id: &'static str) -> Option<&Folder> {
        let folder = self.cache.get(id);
        let read = match folder {
            Some(folder) => folder.stale,
            None => id.starts_with('/') && !self.reading.contains(id),
        };
        if read {
            let mut wanted = self.wanted.borrow_mut();
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
        folder
    }

    pub(crate) fn get_mut(&mut self, id: &'static str) -> Option<&mut Folder> {
        self.changed += 1;
        let folder = self.cache.get_mut(id)?;
        folder.rev = next_rev();
        Some(folder)
    }

    /// Moves whenever a listing's rows are replaced or a folder leaves the cache.
    pub(crate) fn changed(&self) -> u64 {
        self.changed
    }

    /// Moves whenever a folder joins the cache or shows more rows.
    pub(crate) fn grew(&self) -> u64 {
        self.grew
    }

    /// Moves when this folder's rows are replaced; zero while it is not cached.
    pub(crate) fn rev(&self, id: &'static str) -> u64 {
        self.cache.get(id).map_or(0, |folder| folder.rev)
    }

    pub(crate) fn pin(&mut self, ids: impl IntoIterator<Item = &'static str>) {
        self.pinned = ids.into_iter().collect();
    }

    pub(crate) fn has(&self, id: &'static str) -> bool {
        self.cache.contains_key(id)
    }

    /// The watcher reports every changing folder under home; interning each would keep them all.
    pub(crate) fn known(&self, path: &str) -> Option<&'static str> {
        self.cache.get_key_value(path).map(|(id, _)| *id)
    }

    /// Whether a change to this folder is read now rather than when it is next shown.
    pub(crate) fn shown(&self, id: &'static str) -> bool {
        self.pinned.contains(id)
    }

    pub(crate) fn stale(&mut self, id: &'static str) {
        if let Some(folder) = self.cache.get_mut(id) {
            folder.stale = true;
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.cache.len()
    }

    pub(crate) fn take_wanted(&mut self) -> Vec<&'static str> {
        self.wanted.take()
    }

    /// A folder already being read owes a second read: the change may have missed the first.
    pub(crate) fn start(&mut self, id: &'static str) -> bool {
        if let Some(folder) = self.cache.get_mut(id) {
            folder.stale = false;
        }
        let fresh = self.reading.insert(id);
        if !fresh {
            self.owed.insert(id);
        }
        fresh
    }

    /// Whether a change arrived while the read ran, so the folder must be read again.
    pub(crate) fn finish(&mut self, id: &'static str) -> bool {
        self.reading.remove(id);
        self.changed += 1;
        if let Some(folder) = self.cache.get_mut(id) {
            folder.loading = false;
            folder.rev = next_rev();
            if let Some(staged) = folder.staged.take() {
                folder.rows = Arc::new(staged);
            }
        }
        self.owed.remove(id)
    }

    pub(crate) fn forget(&mut self, id: &'static str) {
        self.reading.remove(id);
        self.owed.remove(id);
        self.pinned.remove(id);
        self.order.retain(|&held| held != id);
        if self.cache.remove(id).is_some() {
            self.changed += 1;
        }
    }

    /// Whether the rows show now: a read that replaces a listing holds them back until it ends.
    pub(crate) fn deliver(&mut self, id: &'static str, rows: Vec<Arc<Row>>, replace: bool) -> bool {
        let folder = self.cache.entry(id).or_insert_with(|| {
            self.order.push(id);
            self.grew += 1;
            folder_of(id)
        });
        if replace {
            folder.staged = (!folder.rows.is_empty()).then(Vec::new);
        }
        let shown = folder.staged.is_none() && !rows.is_empty();
        match &mut folder.staged {
            Some(staged) => staged.extend(rows),
            None => Arc::make_mut(&mut folder.rows).extend(rows),
        }
        self.grew += shown as u64;
        self.evict(id);
        shown
    }

    fn evict(&mut self, keep: &'static str) {
        // A disclosed subtree pins and reads every listing it opened: none may be dropped.
        let held = CACHED.max(self.pinned.len() + self.reading.len());
        let mut over = self.order.len().saturating_sub(held);
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
                    self.changed += 1;
                    self.cache.remove(id);
                }
                !drop
            })
            .collect();
    }

    pub(crate) fn held(&self) -> Vec<(&'static str, Vec<Arc<Row>>)> {
        self.order
            .iter()
            .filter_map(|&id| self.cache.get(id).map(|folder| (id, folder.rows.to_vec())))
            .collect()
    }

    /// Every cached listing as it stands, without copying a row.
    pub(crate) fn shared(&self) -> HashMap<&'static str, Arc<Vec<Arc<Row>>>> {
        self.cache
            .iter()
            .map(|(&id, folder)| (id, folder.rows.clone()))
            .collect()
    }
}
