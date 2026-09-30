use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock};

use finder_fs::volumes::{Standard, standard};

use super::paths::{intern, intern_path};
use super::row::Row;
use super::volumes::{is_volume_root, volume_of};
use crate::listing::next_rev;

const CACHED: usize = 64;

pub(crate) struct Folder {
    pub(crate) title: &'static str,
    pub(crate) parent: Option<&'static str>,
    /// From the root down: each crumb's name and the folder it opens.
    pub(crate) crumbs: Vec<(&'static str, &'static str)>,
    pub(crate) rows: Arc<Vec<Arc<Row>>>,
    staged: Option<Vec<Arc<Row>>>,
    pub(crate) loading: bool,
    rev: u64,
    stale: bool,
}

pub(crate) fn folder_of(id: &'static str) -> Folder {
    let crumbs = match crate::locations::place(id) {
        Some((title, _)) => vec![(title, id)],
        None => crumbs_of(id),
    };
    Folder {
        title: crumbs[crumbs.len() - 1].0,
        parent: parent_of(id),
        crumbs,
        rows: Arc::default(),
        staged: None,
        loading: true,
        rev: next_rev(),
        stale: false,
    }
}

pub(crate) fn parent_of(id: &str) -> Option<&'static str> {
    match crate::locations::place(id) {
        Some((_, parent)) => parent,
        None if is_volume_root(id) => Some(crate::locations::COMPUTER),
        None => Path::new(id).parent().map(intern_path),
    }
}

pub(crate) fn icloud_drive() -> Option<&'static str> {
    static DRIVE: LazyLock<Option<&'static str>> =
        LazyLock::new(|| finder_fs::icloud::drive().map(|drive| intern_path(&drive)));
    *DRIVE
}

/// Finder starts the Trash and iCloud Drive at themselves, an app's iCloud folder at iCloud Drive.
fn roots_of(id: &'static str) -> Vec<(&'static str, &'static str)> {
    let path = Path::new(id);
    let say = |english: &str| intern(&finder_fs::words::say(english));
    if let Some(trash) = standard(Standard::Trash).filter(|trash| path.starts_with(trash)) {
        return vec![(say("Trash"), intern_path(&trash))];
    }
    let drive = icloud_drive().map(|drive| (say("iCloud Drive"), drive));
    if let Some(drive) = drive.filter(|(_, drive)| path.starts_with(drive)) {
        return vec![drive];
    }
    if let Some(app) = finder_fs::icloud::app_folder_of(path) {
        let folder = (intern(&app.name), intern_path(&app.documents));
        return drive.into_iter().chain([folder]).collect();
    }
    let (mount, volume) = volume_of(id);
    vec![(volume, mount)]
}

fn crumbs_of(id: &'static str) -> Vec<(&'static str, &'static str)> {
    let mut crumbs = roots_of(id);
    let root = crumbs[crumbs.len() - 1].1.trim_end_matches('/');
    let mut at = root.to_string();
    let rest = id.strip_prefix(root).unwrap_or("");
    crumbs.extend(rest.split('/').filter(|part| !part.is_empty()).map(|part| {
        at.push('/');
        at.push_str(part);
        (
            intern(&finder_fs::name::of_path(Path::new(&at))),
            intern(&at),
        )
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
