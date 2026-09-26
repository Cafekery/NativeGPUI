use std::cell::RefMut;
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering as Atomic};
use std::sync::{Arc, Weak};

use finder_fs::sort::fold;
use gpui::SharedString;

use crate::app::FinderDemo;
use crate::chrome::criteria_bar::Criterion;
use crate::data::{Bands, Column, Folders, Line, Row, today};
use crate::view::{COLUMN_VIEW, LIST_VIEW};
use crate::views::column::Rung;

/// Listings built for other folders are dropped past this many.
const COLUMNS_HELD: usize = 32;
/// Key maps kept for the listings last asked about.
const INDEXES_HELD: usize = 4;
/// Different keys a listing is scanned for before it is mapped.
const SCANS: usize = 2;

pub(crate) enum Slot {
    Header(SharedString),
    Row(usize),
}

pub(crate) fn next_rev() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Atomic::Relaxed)
}

#[derive(Clone)]
pub(crate) struct Keys {
    keys: Arc<HashSet<SharedString>>,
    rev: u64,
}

#[derive(PartialEq)]
pub(crate) struct Stamp {
    here: &'static str,
    sort: Column,
    ascending: bool,
    grouped: bool,
    hidden: bool,
    query: String,
    filter: Option<String>,
    hits: u64,
}

/// The lines of a walk, where it opened each disclosed folder, and every folder it read.
pub(crate) struct Walk {
    lines: Vec<Line>,
    at: Vec<(usize, &'static str)>,
    read: Vec<&'static str>,
}

type Held<'a> = dyn Fn(&'static str) -> Option<&'a [Arc<Row>]> + 'a;

pub(crate) struct Shape {
    stamp: Stamp,
    tree: bool,
    tagged: bool,
    results: bool,
    expanded: Keys,
    hits: Vec<Arc<Row>>,
    criteria: Vec<Criterion>,
}

struct Banded {
    lines: Arc<Vec<Line>>,
    day: f64,
    slots: Arc<Vec<Slot>>,
    at: Arc<Vec<usize>>,
}

pub(crate) type Edit = (usize, SharedString, Option<&'static str>);

type Chain = (Vec<Rung>, Option<Line>);

type ChainKey = (Option<SharedString>, bool, &'static str, u64, u64);

/// Where keys sit in one listing: scanned for at first, mapped once asked for often.
struct Index {
    lines: Weak<Vec<Line>>,
    scans: usize,
    keys: Option<HashMap<SharedString, usize>>,
    last: Option<(SharedString, Option<usize>)>,
}

#[derive(Clone)]
struct Built {
    lines: Arc<Vec<Line>>,
    used: Option<usize>,
    at: Arc<Vec<(usize, &'static str)>>,
    read: Arc<Vec<(&'static str, u64)>>,
}

#[derive(Default)]
pub(crate) struct Listings {
    stamp: Option<Stamp>,
    folders: u64,
    expanded: u64,
    edits: Vec<Edit>,
    edited: u64,
    tree: Option<Built>,
    flat: Option<Built>,
    slots: Option<Banded>,
    columns: HashMap<&'static str, Built>,
    indexes: Vec<Index>,
    names: Option<(Weak<Vec<Line>>, Arc<[usize]>)>,
    chain: Option<(ChainKey, Chain)>,
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

    pub(crate) fn extend(&mut self, keys: impl IntoIterator<Item = SharedString>) {
        let set = Arc::make_mut(&mut self.keys);
        let was = set.len();
        set.extend(keys);
        if set.len() != was {
            self.rev = next_rev();
        }
    }

    pub(crate) fn retain(&mut self, keep: impl FnMut(&SharedString) -> bool) {
        let set = Arc::make_mut(&mut self.keys);
        let was = set.len();
        set.retain(keep);
        if set.len() != was {
            self.rev = next_rev();
        }
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

/// Whether `name` folded holds `folded`, without folding an ASCII name.
pub(crate) fn folds_to(name: &str, folded: &str) -> bool {
    match name.is_ascii() && folded.is_ascii() {
        true => {
            folded.is_empty()
                || name
                    .as_bytes()
                    .windows(folded.len())
                    .any(|at| at.eq_ignore_ascii_case(folded.as_bytes()))
        }
        false => fold(name).contains(folded),
    }
}

impl Shape {
    pub(crate) fn build(&self, held: &Held<'_>) -> Walk {
        let mut out = Vec::new();
        let mut at = Vec::new();
        if !self.tagged {
            self.collect(held, self.stamp.here, 0, &mut out, &mut at);
        }
        let read = (!self.tagged)
            .then_some(self.stamp.here)
            .into_iter()
            .chain(at.iter().map(|(_, id)| *id))
            .collect();
        if !self.results {
            return Walk {
                lines: out,
                at,
                read,
            };
        }
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
        merged.extend(out.into_iter().filter(|line| {
            !ranked.contains(line.key.as_ref())
                && self.criteria.iter().all(|it| it.holds(&line.row))
        }));
        Walk {
            lines: merged,
            at: Vec::new(),
            read,
        }
    }

    pub(crate) fn take_hits(&mut self) -> Vec<Arc<Row>> {
        std::mem::take(&mut self.hits)
    }

    fn collect(
        &self,
        held: &Held<'_>,
        id: &'static str,
        depth: usize,
        out: &mut Vec<Line>,
        at: &mut Vec<(usize, &'static str)>,
    ) {
        self.collect_from(held, id, 0, depth, out, at);
    }

    fn collect_from(
        &self,
        held: &Held<'_>,
        id: &'static str,
        from: usize,
        depth: usize,
        out: &mut Vec<Line>,
        at: &mut Vec<(usize, &'static str)>,
    ) {
        let Some(all) = held(id) else {
            return;
        };
        out.reserve(all.len().saturating_sub(from));
        let query = fold(self.stamp.query.trim());
        let type_filter = fold(self.stamp.filter.as_deref().unwrap_or(""));
        let rows = all
            .iter()
            .skip(from)
            .filter(|row| self.stamp.hidden || !row.hidden)
            .filter(|row| query.is_empty() || folds_to(&row.name, &query))
            .filter(|row| type_filter.is_empty() || folds_to(&row.name, &type_filter));

        for row in rows {
            let open = row
                .opens
                .filter(|_| self.tree && self.expanded.contains(&row.path));
            out.push(Line {
                key: row.path.clone(),
                row: row.clone(),
                depth,
            });
            if let Some(into) = open {
                at.push((out.len() - 1, into));
                self.collect(held, into, depth + 1, out, at);
            }
        }
    }
}

impl Listings {
    /// Drops what a folder change left stale; a disclosed folder that changed is read again in
    /// place rather than the whole tree.
    fn recheck(&mut self, folders: &Folders, expanded: u64) {
        let stale = |built: &Built| built.read.iter().any(|&(id, rev)| folders.rev(id) != rev);
        if self.flat.as_ref().is_some_and(stale) {
            self.flat = None;
        }
        self.columns.retain(|_, built| !stale(built));
        let Some(tree) = self.tree.as_mut().filter(|tree| stale(tree)) else {
            return;
        };
        let here = self.stamp.as_ref().map(|stamp| stamp.here);
        let changed: Vec<&'static str> = tree
            .read
            .iter()
            .filter(|&&(id, rev)| folders.rev(id) != rev)
            .map(|&(id, _)| id)
            .collect();
        let edits: Vec<Edit> = tree
            .at
            .iter()
            .filter(|(_, id)| changed.contains(id))
            .filter_map(|&(at, id)| {
                tree.lines
                    .get(at)
                    .map(|line| (at, line.key.clone(), Some(id)))
            })
            .collect();
        let placed = changed
            .iter()
            .all(|id| Some(*id) != here && edits.iter().any(|(_, _, into)| *into == Some(id)));
        if !placed || self.edited != expanded {
            self.edits.clear();
            self.tree = None;
            return;
        }
        for (id, rev) in Arc::make_mut(&mut tree.read) {
            *rev = folders.rev(id);
        }
        self.edits.extend(edits);
    }
}

impl FinderDemo {
    pub(crate) fn slots(&self, lines: &Arc<Vec<Line>>) -> Option<Arc<Vec<Slot>>> {
        self.banded(lines).map(|(slots, _)| slots)
    }

    fn banded(&self, lines: &Arc<Vec<Line>>) -> Option<(Arc<Vec<Slot>>, Arc<Vec<usize>>)> {
        if !self.grouped {
            return None;
        }
        // Date bands are named for the day they are drawn on.
        let day = match self.sort {
            Column::Modified => today(),
            _ => 0.,
        };
        let cached = self
            .listings()
            .slots
            .as_ref()
            .filter(|banded| Arc::ptr_eq(&banded.lines, lines) && banded.day == day)
            .map(|banded| (banded.slots.clone(), banded.at.clone()));
        if let Some(found) = cached {
            return Some(found);
        }
        let (slots, at) = self.build_slots(lines);
        let (slots, at) = (Arc::new(slots), Arc::new(at));
        self.listings().slots = Some(Banded {
            lines: lines.clone(),
            day,
            slots: slots.clone(),
            at: at.clone(),
        });
        Some((slots, at))
    }

    fn build_slots(&self, lines: &[Line]) -> (Vec<Slot>, Vec<usize>) {
        let mut bands = Bands::new(self.sort);
        let mut band: Option<SharedString> = None;
        let mut out = Vec::with_capacity(lines.len() + 4);
        let mut at = Vec::with_capacity(lines.len());
        for (index, line) in lines.iter().enumerate() {
            if let Some(here) = bands.next(&line.row, band.as_ref()) {
                out.push(Slot::Header(here.clone()));
                band = Some(here);
            }
            at.push(out.len());
            out.push(Slot::Row(index));
        }
        (out, at)
    }

    pub(crate) fn slot_of(&self, lines: &Arc<Vec<Line>>, index: usize) -> usize {
        self.banded(lines)
            .and_then(|(_, at)| at.get(index).copied())
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
                .filter(|_| !tree || listings.edits.is_empty())
                .filter(|built| built.used == rows)
                .map(|built| built.lines.clone())
        };
        if let Some(lines) = fresh {
            return lines;
        }
        let mut edits: Vec<Edit> = match tree {
            true => self.listings().edits.drain(..).collect(),
            false => Vec::new(),
        };
        edits.sort_by_key(|(at, _, _)| *at);
        let taken = self.take(tree);
        let built = match taken {
            Some(built) if !edits.is_empty() => (built.used == rows)
                .then(|| self.spliced(built, &edits))
                .flatten(),
            taken => self.extended(taken, self.here(), tree, rows),
        }
        .unwrap_or_else(|| self.walked(self.shape(tree).build(&self.held()), rows));
        let mut listings = self.listings();
        match tree {
            true => {
                listings.edited = listings.expanded;
                listings.tree = Some(built.clone());
            }
            false => listings.flat = Some(built.clone()),
        }
        built.lines
    }

    pub(crate) fn plain(&self) -> Option<bool> {
        (self.tag_listing.is_none()
            && !self.results()
            && self.expanded.is_empty()
            && self.filtered.trim().is_empty()
            && self.type_filter.is_none())
        .then_some(self.show_hidden)
    }

    pub(crate) fn built(&self, id: &'static str, hidden: bool, lines: Vec<Line>) {
        if id != self.here() || self.plain() != Some(hidden) {
            return;
        }
        let built = Built {
            lines: Arc::new(lines),
            used: self.folders.get(id).map(|folder| folder.rows.len()),
            at: Arc::default(),
            read: Arc::new(vec![(id, self.folders.rev(id))]),
        };
        let mut listings = self.listings();
        listings.edits.clear();
        listings.edited = listings.expanded;
        listings.tree = Some(built.clone());
        listings.flat = Some(built);
    }

    fn stamp(&self, by: (Column, bool)) -> Stamp {
        Stamp {
            here: self.here(),
            sort: by.0,
            ascending: by.1,
            grouped: self.grouped,
            hidden: self.show_hidden,
            query: self.filtered.clone(),
            filter: self.type_filter.clone(),
            hits: self.hits_rev,
        }
    }

    fn shape(&self, tree: bool) -> Shape {
        self.shape_sorted(tree, (self.sort, self.ascending))
    }

    pub(crate) fn shape_sorted(&self, tree: bool, by: (Column, bool)) -> Shape {
        let results = self.results();
        Shape {
            stamp: self.stamp(by),
            tree,
            tagged: self.tag_listing.is_some(),
            results,
            expanded: self.expanded.clone(),
            hits: match results {
                true => self.hits.clone(),
                false => Vec::new(),
            },
            criteria: self.criteria.clone(),
        }
    }

    pub(crate) fn shape_filtered(&self, query: &str, hits: Vec<Arc<Row>>, rev: u64) -> Shape {
        let mut shape = self.shape(self.view == LIST_VIEW);
        shape.stamp.query = query.to_owned();
        shape.stamp.hits = rev;
        shape.results = self.tag_listing.is_some() || !(query.is_empty() || hits.is_empty());
        shape.hits = hits;
        shape
    }

    pub(crate) fn shaped(&self, shape: &Shape, walk: Walk) {
        if shape.stamp != self.stamp((self.sort, self.ascending))
            || shape.tree != (self.view == LIST_VIEW)
            || shape.expanded.rev() != self.expanded.rev()
        {
            return;
        }
        let built = self.walked(walk, self.appendable());
        let mut listings = self.listings();
        listings.edits.clear();
        listings.edited = listings.expanded;
        if shape.tree {
            listings.tree = Some(built.clone());
        }
        if !shape.tree || shape.expanded.is_empty() {
            listings.flat = Some(built);
        }
    }

    fn walked(&self, walk: Walk, used: Option<usize>) -> Built {
        Built {
            lines: Arc::new(walk.lines),
            used,
            at: Arc::new(walk.at),
            read: Arc::new(
                walk.read
                    .into_iter()
                    .map(|id| (id, self.folders.rev(id)))
                    .collect(),
            ),
        }
    }

    /// The folders a listing read before, and those it opened since, as they stand now.
    fn reread(
        &self,
        read: &[(&'static str, u64)],
        at: &[(usize, &'static str)],
    ) -> Arc<Vec<(&'static str, u64)>> {
        let mut ids: Vec<&'static str> = read.iter().map(|(id, _)| *id).collect();
        ids.extend(at.iter().map(|(_, id)| *id));
        ids.sort_unstable();
        ids.dedup();
        Arc::new(
            ids.into_iter()
                .map(|id| (id, self.folders.rev(id)))
                .collect(),
        )
    }

    fn take(&self, tree: bool) -> Option<Built> {
        let mut listings = self.listings();
        listings.slots = None;
        let taken = match tree {
            true => listings.tree.take(),
            false => listings.flat.take(),
        };
        let twin = match tree {
            true => &mut listings.flat,
            false => &mut listings.tree,
        };
        if twin
            .as_ref()
            .zip(taken.as_ref())
            .is_some_and(|(twin, taken)| Arc::ptr_eq(&twin.lines, &taken.lines))
        {
            *twin = None;
        }
        taken
    }

    fn appendable(&self) -> Option<usize> {
        if self.tag_listing.is_some() || self.results() {
            return None;
        }
        self.folders
            .get(self.here())
            .map(|folder| folder.rows.len())
    }

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
        let at = Arc::make_mut(&mut built.at);
        self.shape(tree)
            .collect_from(&self.held(), id, used, 0, lines, at);
        built.read = self.reread(&built.read, at);
        built.used = Some(rows);
        Some(built)
    }

    fn spliced(&self, built: Built, edits: &[Edit]) -> Option<Built> {
        let shape = self.shape(true);
        let old = built.lines;
        if edits
            .iter()
            .any(|(at, key, _)| old.get(*at).is_none_or(|line| &line.key != key))
        {
            return None;
        }
        let grown: usize = edits
            .iter()
            .filter_map(|(_, _, into)| *into)
            .filter_map(|into| self.folders.get(into))
            .map(|folder| folder.rows.len())
            .sum();
        let old = Arc::try_unwrap(old).unwrap_or_else(|shared| (*shared).clone());
        let mut out: Vec<Line> = Vec::with_capacity(old.len() + grown);
        let mut marks: Vec<(usize, &'static str)> =
            Vec::with_capacity(built.at.len() + edits.len());
        let mut held = built.at.iter().peekable();
        let mut src = old.into_iter();
        let mut at = 0;
        for (index, _, into) in edits {
            if *index < at {
                continue;
            }
            let base = out.len();
            while let Some((held_at, held_id)) = held.next_if(|(held_at, _)| *held_at < *index) {
                if *held_at >= at {
                    marks.push((base + held_at - at, held_id));
                }
            }
            out.extend(src.by_ref().take(index + 1 - at));
            let depth = out.last()?.depth;
            at = index + 1;
            while src
                .as_slice()
                .first()
                .is_some_and(|line| line.depth > depth)
            {
                src.next();
                at += 1;
            }
            while held.next_if(|(held_at, _)| *held_at < at).is_some() {}
            if let Some(into) = into {
                marks.push((out.len() - 1, into));
                shape.collect(&self.held(), into, depth + 1, &mut out, &mut marks);
            }
        }
        let base = out.len();
        for (held_at, held_id) in held {
            if *held_at >= at {
                marks.push((base + held_at - at, held_id));
            }
        }
        out.extend(src);
        Some(Built {
            lines: Arc::new(out),
            used: built.used,
            read: self.reread(&built.read, &marks),
            at: Arc::new(marks),
        })
    }

    pub(crate) fn disclosed(&self, was: u64, edits: Vec<Edit>) {
        let mut listings = self.cache.borrow_mut();
        if listings.expanded == was && listings.edits.is_empty() && listings.tree.is_some() {
            listings.edited = self.expanded.rev();
            listings.edits = edits;
        }
    }

    fn listings(&self) -> RefMut<'_, Listings> {
        let stamp = self.stamp((self.sort, self.ascending));
        let folders = self.folders.changed();
        let expanded = self.expanded.rev();
        let mut listings = self.cache.borrow_mut();
        if listings.stamp.as_ref() != Some(&stamp) {
            *listings = Listings {
                stamp: Some(stamp),
                folders,
                expanded,
                edited: expanded,
                ..Default::default()
            };
            return listings;
        }
        if listings.folders != folders {
            listings.folders = folders;
            listings.recheck(&self.folders, expanded);
        }
        if listings.expanded != expanded {
            listings.expanded = expanded;
            if listings.edited != expanded {
                listings.edits.clear();
                listings.tree = None;
            }
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
                let mut at = Vec::new();
                self.shape(false)
                    .collect(&self.held(), folder, 0, &mut lines, &mut at);
                Built {
                    lines: Arc::new(lines),
                    used: rows,
                    at: Arc::new(at),
                    read: Arc::new(vec![(folder, self.folders.rev(folder))]),
                }
            });
        let mut listings = self.listings();
        if listings.columns.len() >= COLUMNS_HELD {
            listings.columns.clear();
        }
        listings.columns.insert(folder, built.clone());
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

    /// The browser's columns change only with the cursor, the selection's size and the folders.
    pub(crate) fn column_chain(&self) -> Chain {
        let key = (
            self.cursor.clone(),
            self.selected.len() <= 1,
            self.here(),
            self.folders.changed(),
            self.folders.grew(),
        );
        if let Some((_, chain)) = self.listings().chain.as_ref().filter(|(at, _)| *at == key) {
            return chain.clone();
        }
        let chain = self.walk_chain();
        self.listings().chain = Some((key, chain.clone()));
        chain
    }

    fn held<'a>(&'a self) -> impl Fn(&'static str) -> Option<&'a [Arc<Row>]> + 'a {
        |id| self.folders.get(id).map(|folder| folder.rows.as_slice())
    }

    pub(crate) fn line_at(&self, lines: &Arc<Vec<Line>>, key: &SharedString) -> Option<usize> {
        let mut listings = self.cache.borrow_mut();
        let held = &mut listings.indexes;
        let at = match held
            .iter()
            .position(|index| std::ptr::eq(index.lines.as_ptr(), Arc::as_ptr(lines)))
        {
            Some(at) => at,
            None => {
                if held.len() >= INDEXES_HELD {
                    held.remove(0);
                }
                held.push(Index {
                    lines: Arc::downgrade(lines),
                    scans: 0,
                    keys: None,
                    last: None,
                });
                held.len() - 1
            }
        };
        let index = &mut held[at];
        if let Some((last, found)) = &index.last {
            if last == key {
                return *found;
            }
        }
        if index.keys.is_none() && index.scans >= SCANS {
            let mut keys = HashMap::with_capacity(lines.len());
            for (at, line) in lines.iter().enumerate() {
                keys.entry(line.key.clone()).or_insert(at);
            }
            index.keys = Some(keys);
        }
        let found = match &index.keys {
            Some(keys) => keys.get(key).copied(),
            None => {
                index.scans += 1;
                lines.iter().position(|line| &line.key == key)
            }
        };
        index.last = Some((key.clone(), found));
        found
    }

    /// Tab walks names in Finder's order, which costs a sort of the whole listing, so each
    /// listing sorts once.
    pub(crate) fn name_order(&self, lines: &Arc<Vec<Line>>) -> Arc<[usize]> {
        if let Some((held, order)) = &self.listings().names {
            if held.upgrade().is_some_and(|held| Arc::ptr_eq(&held, lines)) {
                return order.clone();
            }
        }
        let mut order: Vec<usize> = (0..lines.len()).collect();
        order.sort_by(|&i, &j| finder_fs::sort::name_cmp(&lines[i].row.name, &lines[j].row.name));
        let order: Arc<[usize]> = order.into();
        self.listings().names = Some((Arc::downgrade(lines), order.clone()));
        order
    }

    pub(crate) fn cursor_line(&self) -> Option<Line> {
        if self.inspecting.is_some() {
            return self.inspecting.clone();
        }
        let key = self.cursor.as_ref()?;
        let lines = self.lines(self.view == LIST_VIEW);
        self.line_at(&lines, key).map(|at| lines[at].clone())
    }
}
