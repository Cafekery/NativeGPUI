use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use finder_fs::volumes::{Standard, standard};

use super::paths::{intern, intern_path};
use super::row::Row;
use super::volumes::{is_volume_root, volume_of};
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
    let (title, parent, crumbs) = match crate::locations::place(id) {
        Some((title, parent)) => (title, parent, vec![title]),
        None => {
            let crumbs = crumbs_of(id);
            let title = crumbs[crumbs.len() - 1];
            let parent = match is_volume_root(id) {
                true => Some(crate::locations::COMPUTER),
                false => Path::new(id).parent().map(intern_path),
            };
            (title, parent, crumbs)
        }
    };
    Folder {
        title,
        parent,
        crumbs,
        rows: Arc::default(),
        staged: None,
        loading: true,
        rev: next_rev(),
        stale: false,
    }
}

fn crumbs_of(id: &'static str) -> Vec<&'static str> {
    let trash = standard(Standard::Trash).filter(|trash| Path::new(id).starts_with(trash));
    let (mount, volume) = match trash {
        Some(trash) => (intern_path(&trash), intern(&finder_fs::words::say("Trash"))),
        None => volume_of(id),
    };
    let mount = mount.trim_end_matches('/');
    let mut at = mount.to_string();
    let mut crumbs = vec![volume];
    let rest = id.strip_prefix(mount).unwrap_or("");
    crumbs.extend(rest.split('/').filter(|part| !part.is_empty()).map(|part| {
        at.push('/');
        at.push_str(part);
        intern(&finder_fs::name::of_path(Path::new(&at)))
    }));
    crumbs
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
    pub(crate) fn get(&self, id: &'static str) -> Option<&Folder> {
        let folder = self.cache.get(id);
        let read = match folder {
            Some(folder) => folder.stale,
            None => {
                (id.starts_with('/') || crate::locations::is_location(id))
                    && !self.reading.contains(id)
            }
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

    pub(crate) fn changed(&self) -> u64 {
        self.changed
    }

    pub(crate) fn grew(&self) -> u64 {
        self.grew
    }

    pub(crate) fn rev(&self, id: &'static str) -> u64 {
        self.cache.get(id).map_or(0, |folder| folder.rev)
    }

    pub(crate) fn pin(&mut self, ids: impl IntoIterator<Item = &'static str>) {
        self.pinned = ids.into_iter().collect();
    }

    pub(crate) fn has(&self, id: &'static str) -> bool {
        self.cache.contains_key(id)
    }

    // Not interned: the watcher reports every changing folder under home.
    pub(crate) fn known(&self, path: &str) -> Option<&'static str> {
        self.cache.get_key_value(path).map(|(id, _)| *id)
    }

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
        let held = CACHED.max(self.pinned.len() + self.reading.len());
        let mut over = self.order.len().saturating_sub(held);
        if over == 0 {
            return;
        }
        self.order.retain(|id| {
            let drop =
                over > 0 && *id != keep && !self.pinned.contains(id) && !self.reading.contains(id);
            if drop {
                over -= 1;
                self.changed += 1;
                self.cache.remove(id);
            }
            !drop
        });
    }

    pub(crate) fn held(&self) -> Vec<(&'static str, Vec<Arc<Row>>)> {
        self.order
            .iter()
            .filter_map(|&id| self.cache.get(id).map(|folder| (id, folder.rows.to_vec())))
            .collect()
    }

    pub(crate) fn shared(&self) -> HashMap<&'static str, Arc<Vec<Arc<Row>>>> {
        self.cache
            .iter()
            .map(|(&id, folder)| (id, folder.rows.clone()))
            .collect()
    }
}
