//! The rows a frame draws: built once from the listings on screen, kept until what they were
//! built from moves.

use std::cell::RefMut;
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as Atomic};

use gpui::SharedString;

use crate::FinderDemo;
use crate::data::{Column, Line, group_of};
use crate::view::LIST_VIEW;

/// What a grouped list draws: Finder's band header, or the row at that index.
pub(crate) enum Slot {
    Header(SharedString),
    Row(usize),
}

pub(crate) fn next_rev() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Atomic::Relaxed)
}

/// A set of row keys a frame shares rather than copies: handing it to an element costs a clone of
/// the handle, and every change gives the set a new revision the caches built from it compare.
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

/// The band slots built for one listing, beside the listing they were built from.
type Bands = (Arc<Vec<Line>>, Arc<Vec<Slot>>);

/// The cursor's key, and the line it names while that listing holds one.
type Cursor = (SharedString, Option<Line>);

/// The rows, bands and browser listings a frame draws, kept until [`Stamp`] moves: walking a
/// disclosed home folder is millions of rows, which no frame can pay for twice.
#[derive(Default)]
pub(crate) struct Listings {
    stamp: Option<Stamp>,
    tree: Option<Arc<Vec<Line>>>,
    flat: Option<Arc<Vec<Line>>>,
    slots: Option<Bands>,
    columns: HashMap<&'static str, Arc<Vec<Line>>>,
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

impl FinderDemo {
    /// The rows a grouped list draws, with Finder's band headers between them.
    pub(crate) fn slots(&self, lines: &Arc<Vec<Line>>) -> Arc<Vec<Slot>> {
        let cached = self
            .listings()
            .slots
            .as_ref()
            .filter(|(built_for, _)| Arc::ptr_eq(built_for, lines))
            .map(|(_, slots)| slots.clone());
        if let Some(slots) = cached {
            return slots;
        }
        let built = Arc::new(self.build_slots(lines));
        self.listings().slots = Some((lines.clone(), built.clone()));
        built
    }

    fn build_slots(&self, lines: &[Line]) -> Vec<Slot> {
        if !self.grouped {
            return (0..lines.len()).map(Slot::Row).collect();
        }
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
        self.slots(lines)
            .iter()
            .position(|slot| matches!(slot, Slot::Row(at) if *at == index))
            .unwrap_or(index)
    }

    /// The rows on screen, built once and shared by everything that draws this frame.
    pub(crate) fn lines(&self, tree: bool) -> Arc<Vec<Line>> {
        let cached = match tree {
            true => self.listings().tree.clone(),
            false => self.listings().flat.clone(),
        };
        if let Some(lines) = cached {
            return lines;
        }
        let built = Arc::new(self.build_lines(tree));
        let mut listings = self.listings();
        match tree {
            true => listings.tree = Some(built.clone()),
            false => listings.flat = Some(built.clone()),
        }
        built
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

    pub(crate) fn listing_cached(&self, folder: &'static str) -> Option<Arc<Vec<Line>>> {
        self.listings().columns.get(folder).cloned()
    }

    pub(crate) fn listing_cache(
        &self,
        folder: &'static str,
        lines: Arc<Vec<Line>>,
    ) -> Arc<Vec<Line>> {
        self.listings().columns.insert(folder, lines.clone());
        lines
    }

    fn build_lines(&self, tree: bool) -> Vec<Line> {
        let mut out = Vec::new();
        if self.tag_listing.is_none() {
            self.collect(self.here(), 0, tree, &mut out);
        }
        if !self.results() {
            return out;
        }
        // The index's hits are already ranked best first; anything only the folder listing
        // still knows about (too new for the index) trails behind, unranked.
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
        let Some(folder) = self.folders.get(id) else {
            return;
        };
        let query = self.query.trim().to_lowercase();
        let type_filter = self.type_filter.as_deref().unwrap_or("");
        let rows = folder
            .rows
            .iter()
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
