use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use finder_fs::volumes::{Standard, standard};

use super::paths::{intern, intern_path};
use super::row::Row;
use super::volumes::volume_of;

const CACHED: usize = 64;

pub(crate) struct Folder {
    pub(crate) title: &'static str,
    pub(crate) parent: Option<&'static str>,
    pub(crate) crumbs: Vec<&'static str>,
    pub(crate) rows: Vec<Arc<Row>>,
    staged: Option<Vec<Arc<Row>>>,
    pub(crate) loading: bool,
}

pub(crate) fn folder_of(id: &'static str) -> Folder {
    // Finder roots the Trash's path at "Trash", not at .Trash inside the home folder.
    let trash = standard(Standard::Trash).filter(|trash| Path::new(id).starts_with(trash));
    let (mount, volume) = match trash {
        Some(trash) => (intern_path(&trash), "Trash"),
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
        rows: Vec::new(),
        staged: None,
        loading: true,
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
}

impl Folders {
    /// A miss asks for a read at the next pump and draws nothing until it lands.
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

    pub(crate) fn changed(&self) -> u64 {
        self.changed
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

    pub(crate) fn len(&self) -> usize {
        self.cache.len()
    }

    pub(crate) fn take_wanted(&mut self) -> Vec<&'static str> {
        self.wanted.take()
    }

    /// A folder already being read owes a second read: the change may have missed the first.
    pub(crate) fn start(&mut self, id: &'static str) -> bool {
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
            if let Some(staged) = folder.staged.take() {
                folder.rows = staged;
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

    pub(crate) fn deliver(&mut self, id: &'static str, rows: Vec<Arc<Row>>, replace: bool) {
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

    pub(crate) fn cached(&self) -> Vec<&'static str> {
        self.order.clone()
    }

    pub(crate) fn held(&self) -> Vec<(&'static str, Vec<Arc<Row>>)> {
        self.cached()
            .into_iter()
            .filter_map(|id| self.get(id).map(|folder| (id, folder.rows.clone())))
            .collect()
    }
}
