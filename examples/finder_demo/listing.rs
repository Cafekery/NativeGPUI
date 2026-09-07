//! The rows a frame draws: built once from the listings on screen, kept until they move.

use std::cell::RefMut;
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as Atomic};

use gpui::SharedString;

use crate::app::FinderDemo;
use crate::data::{Column, Line, Row, group_of};
use crate::view::{COLUMN_VIEW, LIST_VIEW};

/// What a grouped list draws: Finder's band header, or the row at that index.
pub(crate) enum Slot {
    Header(SharedString),
    Row(usize),
}

pub(crate) fn next_rev() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Atomic::Relaxed)
}

/// A set of row keys a frame shares rather than copies, under a revision its caches compare.
#[derive(Clone)]
pub(crate) struct Keys {
    keys: Arc<HashSet<SharedString>>,
    rev: u64,
}

/// Everything the rows a frame draws are built from: while this holds, so do they.
#[derive(PartialEq)]
pub(crate) struct Stamp {
    folders: u64,
    expanded: u64,
    here: &'static str,
    sort: Column,
    ascending: bool,
    grouped: bool,
    hidden: bool,
    query: String,
    filter: Option<String>,
    hits: u64,
}

/// The band slots built for one listing, beside that listing.
type Bands = (Arc<Vec<Line>>, Arc<Vec<Slot>>);

/// A listing as built, and how many of the folder's rows went into it.
#[derive(Clone)]
struct Built {
    lines: Arc<Vec<Line>>,
    used: Option<usize>,
}

/// The cursor's key, and the line it names while that listing holds one.
type Cursor = (SharedString, Option<Line>);

/// The rows, bands and browser listings a frame draws, kept until [`Stamp`] moves.
#[derive(Default)]
pub(crate) struct Listings {
    stamp: Option<Stamp>,
    tree: Option<Built>,
    flat: Option<Built>,
    slots: Option<Bands>,
    columns: HashMap<&'static str, Built>,
    cursor: Option<Cursor>,
}

impl Keys {
    pub(crate) fn snapshot(&self) -> Arc<HashSet<SharedString>> {
        self.keys.clone()
    }

    pub(crate) fn rev(&self) -> u64 {
        self.rev
    }

    pub(crate) fn insert(&mut self, key: SharedString) -> bool {
        self.rev = next_rev();
        Arc::make_mut(&mut self.keys).insert(key)
    }

    pub(crate) fn remove(&mut self, key: &SharedString) -> bool {
        self.rev = next_rev();
        Arc::make_mut(&mut self.keys).remove(key)
    }

    /// Every key in one revision, so a disclosed subtree rebuilds the rows behind it once.
    pub(crate) fn extend(&mut self, keys: impl IntoIterator<Item = SharedString>) {
        self.rev = next_rev();
        Arc::make_mut(&mut self.keys).extend(keys);
    }

    pub(crate) fn retain(&mut self, keep: impl FnMut(&SharedString) -> bool) {
        self.rev = next_rev();
        Arc::make_mut(&mut self.keys).retain(keep);
    }

    pub(crate) fn clear(&mut self) {
        self.rev = next_rev();
        self.keys = Arc::default();
    }
}

impl Default for Keys {
    fn default() -> Self {
        Self {
            keys: Arc::default(),
            rev: next_rev(),
        }
    }
}

impl Deref for Keys {
    type Target = HashSet<SharedString>;

    fn deref(&self) -> &Self::Target {
        &self.keys
    }
}

impl FromIterator<SharedString> for Keys {
    fn from_iter<I: IntoIterator<Item = SharedString>>(keys: I) -> Self {
        Self {
            keys: Arc::new(keys.into_iter().collect()),
            rev: next_rev(),
        }
    }
}

pub(crate) fn lines_of(rows: &[Arc<Row>], hidden: bool) -> Vec<Line> {
    rows.iter()
        .filter(|row| hidden || !row.hidden)
        .map(|row| Line {
            key: row.path.clone(),
            row: row.clone(),
            depth: 0,
        })
        .collect()
}

impl FinderDemo {
    /// The rows a grouped list draws, with Finder's band headers between them.
    pub(crate) fn slots(&self, lines: &Arc<Vec<Line>>) -> Option<Arc<Vec<Slot>>> {
        if !self.grouped {
            return None;
        }
        let cached = self
            .listings()
            .slots
            .as_ref()
            .filter(|(built_for, _)| Arc::ptr_eq(built_for, lines))
            .map(|(_, slots)| slots.clone());
        if let Some(slots) = cached {
            return Some(slots);
        }
        let built = Arc::new(self.build_slots(lines));
        self.listings().slots = Some((lines.clone(), built.clone()));
        Some(built)
    }

    fn build_slots(&self, lines: &[Line]) -> Vec<Slot> {
        let mut band: Option<SharedString> = None;
        let mut out = Vec::with_capacity(lines.len() + 4);
        for (index, line) in lines.iter().enumerate() {
            let here = group_of(&line.row, self.sort);
            if band.as_ref() != Some(&here) {
                out.push(Slot::Header(here.clone()));
                band = Some(here);
            }
            out.push(Slot::Row(index));
        }
        out
    }

    pub(crate) fn slot_of(&self, lines: &Arc<Vec<Line>>, index: usize) -> usize {
        let Some(slots) = self.slots(lines) else {
            return index;
        };
        slots
            .iter()
            .position(|slot| matches!(slot, Slot::Row(at) if *at == index))
            .unwrap_or(index)
    }

    pub(crate) fn lines(&self, tree: bool) -> Arc<Vec<Line>> {
        let rows = self.appendable();
        let fresh = {
            let listings = self.listings();
            let built = match tree {
                true => &listings.tree,
                false => &listings.flat,
            };
            built
                .as_ref()
                .filter(|built| built.used == rows)
                .map(|built| built.lines.clone())
        };
        if let Some(lines) = fresh {
            return lines;
        }
        let taken = self.take(tree);
        let built = self
            .extended(taken, self.here(), tree, rows)
            .unwrap_or_else(|| Built {
                lines: Arc::new(self.build_lines(tree)),
                used: rows,
            });
        let mut listings = self.listings();
        match tree {
            true => listings.tree = Some(built.clone()),
            false => listings.flat = Some(built.clone()),
        }
        built.lines
    }

    /// Whether a listing is drawn as it is read, and with it whether hidden rows are drawn.
    pub(crate) fn plain(&self) -> Option<bool> {
        (self.tag_listing.is_none()
            && !self.results()
            && self.expanded.is_empty()
            && self.query.trim().is_empty()
            && self.type_filter.is_none())
        .then_some(self.show_hidden)
    }

    /// Take the lines a read built, so the frame they land under does not build them again.
    pub(crate) fn built(&self, id: &'static str, hidden: bool, lines: Vec<Line>) {
        if id != self.here() || self.plain() != Some(hidden) {
            return;
        }
        let built = Built {
            lines: Arc::new(lines),
            used: self.folders.get(id).map(|folder| folder.rows.len()),
        };
        let mut listings = self.listings();
        listings.tree = Some(built.clone());
        listings.flat = Some(built);
    }

    /// Lifts the cached listing out, so extending it does not copy what the cache still holds.
    fn take(&self, tree: bool) -> Option<Built> {
        let mut listings = self.listings();
        listings.slots = None;
        match tree {
            true => listings.tree.take(),
            false => listings.flat.take(),
        }
    }

    /// How many rows of the folder on screen a listing is built from; `None` where hits shape it.
    fn appendable(&self) -> Option<usize> {
        if self.tag_listing.is_some() || self.results() {
            return None;
        }
        self.folders
            .get(self.here())
            .map(|folder| folder.rows.len())
    }

    /// The rows a listing gained since it was built, appended to what was built.
    fn extended(
        &self,
        built: Option<Built>,
        id: &'static str,
        tree: bool,
        rows: Option<usize>,
    ) -> Option<Built> {
        let mut built = built?;
        let rows = rows?;
        let used = built.used.filter(|used| rows > *used)?;
        let lines = Arc::make_mut(&mut built.lines);
        self.collect_from(id, used, 0, tree, lines);
        built.used = Some(rows);
        Some(built)
    }

    /// The cache every listing this frame comes out of, emptied when what it holds is stale.
    fn listings(&self) -> RefMut<'_, Listings> {
        let stamp = Stamp {
            folders: self.folders.changed(),
            expanded: self.expanded.rev(),
            here: self.here(),
            sort: self.sort,
            ascending: self.ascending,
            grouped: self.grouped,
            hidden: self.show_hidden,
            query: self.query.clone(),
            filter: self.type_filter.clone(),
            hits: self.hits_rev,
        };
        let mut listings = self.cache.borrow_mut();
        if listings.stamp.as_ref() != Some(&stamp) {
            *listings = Listings {
                stamp: Some(stamp),
                ..Default::default()
            };
        }
        listings
    }

    pub(crate) fn column_listing(&self, folder: &'static str) -> Arc<Vec<Line>> {
        let rows = self.folders.get(folder).map(|listing| listing.rows.len());
        let fresh = self
            .listings()
            .columns
            .get(folder)
            .filter(|built| built.used == rows)
            .map(|built| built.lines.clone());
        if let Some(lines) = fresh {
            return lines;
        }
        let taken = self.listings().columns.remove(folder);
        let built = self
            .extended(taken, folder, false, rows)
            .unwrap_or_else(|| {
                let mut lines = Vec::new();
                self.collect(folder, 0, false, &mut lines);
                Built {
                    lines: Arc::new(lines),
                    used: rows,
                }
            });
        self.listings().columns.insert(folder, built.clone());
        built.lines
    }

    pub(crate) fn focused_lines(&self) -> Arc<Vec<Line>> {
        if self.view != COLUMN_VIEW {
            return self.lines(self.view == LIST_VIEW);
        }
        let (chain, _) = self.column_chain();
        let focused = chain
            .iter()
            .rposition(|rung| rung.selected.is_some())
            .unwrap_or(0);
        self.column_listing(chain[focused].folder)
    }

    fn build_lines(&self, tree: bool) -> Vec<Line> {
        let mut out = Vec::new();
        if self.tag_listing.is_none() {
            self.collect(self.here(), 0, tree, &mut out);
        }
        if !self.results() {
            return out;
        }
        // The index's hits are ranked best first; what only the listing knows trails behind.
        let ranked: HashSet<&str> = self.hits.iter().map(|row| row.path.as_ref()).collect();
        let mut merged: Vec<Line> = self
            .hits
            .iter()
            .map(|row| Line {
                key: row.path.clone(),
                row: row.clone(),
                depth: 0,
            })
            .collect();
        merged.extend(
            out.into_iter()
                .filter(|line| !ranked.contains(line.key.as_ref())),
        );
        merged
    }

    pub(crate) fn collect(&self, id: &'static str, depth: usize, tree: bool, out: &mut Vec<Line>) {
        self.collect_from(id, 0, depth, tree, out);
    }

    /// The listing's rows from `from` on, filtered and disclosed the way a frame draws them.
    fn collect_from(
        &self,
        id: &'static str,
        from: usize,
        depth: usize,
        tree: bool,
        out: &mut Vec<Line>,
    ) {
        let Some(folder) = self.folders.get(id) else {
            return;
        };
        out.reserve(folder.rows.len().saturating_sub(from));
        let query = self.query.trim().to_lowercase();
        let type_filter = self.type_filter.as_deref().unwrap_or("");
        let rows = folder
            .rows
            .iter()
            .skip(from)
            .filter(|row| self.show_hidden || !row.hidden)
            .filter(|row| query.is_empty() || row.name.to_lowercase().contains(&query))
            .filter(|row| type_filter.is_empty() || row.name.to_lowercase().contains(type_filter));

        for row in rows {
            let open = tree && self.expanded.contains(&row.path);
            out.push(Line {
                key: row.path.clone(),
                row: row.clone(),
                depth,
            });
            if let Some(into) = row.opens.filter(|_| open) {
                self.collect(into, depth + 1, tree, out);
            }
        }
    }

    pub(crate) fn line_at(&self, lines: &[Line], key: &SharedString) -> Option<usize> {
        lines.iter().position(|line| &line.key == key)
    }

    pub(crate) fn cursor_line(&self) -> Option<Line> {
        if self.inspecting.is_some() {
            return self.inspecting.clone();
        }
        let key = self.cursor.clone()?;
        let cached = self
            .listings()
            .cursor
            .as_ref()
            .filter(|(at, _)| *at == key)
            .map(|(_, line)| line.clone());
        if let Some(line) = cached {
            return line;
        }
        let lines = self.lines(self.view == LIST_VIEW);
        let line = lines.iter().find(|line| line.key == key).cloned();
        self.listings().cursor = Some((key, line.clone()));
        line
    }
}
