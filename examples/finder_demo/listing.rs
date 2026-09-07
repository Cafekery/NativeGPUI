//! The rows a frame draws: built once from the listings on screen, kept until they move.

use std::cell::RefMut;
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as Atomic};

use finder_fs::sort::fold;
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
    here: &'static str,
    sort: Column,
    ascending: bool,
    grouped: bool,
    hidden: bool,
    query: String,
    filter: Option<String>,
    hits: u64,
}

/// A listing as it comes out of a walk: the lines, and where the folders it discloses sit.
pub(crate) type Walk = (Vec<Line>, Vec<(usize, &'static str)>);

/// Where a walk finds a folder's rows: the listings in memory, or a copy held off the main thread.
type Held<'a> = dyn Fn(&'static str) -> Option<&'a [Arc<Row>]> + 'a;

/// Everything a listing is shaped by apart from the rows themselves, taken by value so a sort can
/// build off the main thread what the frame after it would otherwise build on it.
pub(crate) struct Shape {
    stamp: Stamp,
    tree: bool,
    tagged: bool,
    results: bool,
    expanded: Keys,
    hits: Vec<Arc<Row>>,
}

/// The band slots built for one listing, beside that listing.
type Bands = (Arc<Vec<Line>>, Arc<Vec<Slot>>);

/// A disclose to splice into the tree listing: where it lands, the key that must still be
/// there, and the folder to open under it, or `None` to shut what is open there.
pub(crate) type Edit = (usize, SharedString, Option<&'static str>);

/// A listing as built, how many of the folder's rows went into it, and where the folders it
/// discloses sit in it.
#[derive(Clone)]
struct Built {
    lines: Arc<Vec<Line>>,
    used: Option<usize>,
    at: Arc<Vec<(usize, &'static str)>>,
}

/// The cursor's key, and the line it names while that listing holds one.
type Cursor = (SharedString, Option<Line>);

/// The rows, bands and browser listings a frame draws, kept until [`Stamp`] moves.
#[derive(Default)]
pub(crate) struct Listings {
    stamp: Option<Stamp>,
    /// The revisions of the listings and of `expanded` the cache was filled under.
    folders: u64,
    expanded: u64,
    /// The discloses and landings that carry the tree listing to those revisions.
    edits: Vec<Edit>,
    edited: u64,
    landed: u64,
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

impl Shape {
    /// The listing a frame would build from these rows: the same walk, wherever it runs.
    pub(crate) fn build(&self, held: &Held<'_>) -> Walk {
        let mut out = Vec::new();
        let mut at = Vec::new();
        if !self.tagged {
            self.collect(held, self.stamp.here, 0, &mut out, &mut at);
        }
        if !self.results {
            return (out, at);
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
        (merged, Vec::new())
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

    /// The listing's rows from `from` on, filtered and disclosed the way a frame draws them.
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
            .filter(|row| query.is_empty() || fold(&row.name).contains(&query))
            .filter(|row| type_filter.is_empty() || fold(&row.name).contains(&type_filter));

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
            Some(built) if !edits.is_empty() && built.used == rows => self.spliced(built, &edits),
            taken => self.extended(taken, self.here(), tree, rows),
        }
        .unwrap_or_else(|| {
            let (lines, at) = self.build_lines(tree);
            Built {
                lines: Arc::new(lines),
                used: rows,
                at: Arc::new(at),
            }
        });
        let mut listings = self.listings();
        match tree {
            true => {
                listings.edited = listings.expanded;
                listings.landed = listings.folders;
                listings.tree = Some(built.clone());
            }
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
            at: Arc::default(),
        };
        let mut listings = self.listings();
        listings.edits.clear();
        listings.edited = listings.expanded;
        listings.landed = listings.folders;
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
            query: self.query.clone(),
            filter: self.type_filter.clone(),
            hits: self.hits_rev,
        }
    }

    fn shape(&self, tree: bool) -> Shape {
        self.shape_sorted(tree, (self.sort, self.ascending))
    }

    /// The shape a frame draws the listing in, taken so a sort can build it under that shape.
    pub(crate) fn shape_sorted(&self, tree: bool, by: (Column, bool)) -> Shape {
        Shape {
            stamp: self.stamp(by),
            tree,
            tagged: self.tag_listing.is_some(),
            results: self.results(),
            expanded: self.expanded.clone(),
            hits: self.hits.clone(),
        }
    }

    /// Take the listing a sort built, where the window is still drawing the shape it was built
    /// for, so the frame the new order lands under does not build it again.
    pub(crate) fn shaped(&self, shape: &Shape, walk: Walk) {
        if shape.stamp != self.stamp((self.sort, self.ascending))
            || shape.tree != (self.view == LIST_VIEW)
            || shape.expanded.rev() != self.expanded.rev()
        {
            return;
        }
        let (lines, at) = walk;
        let built = Built {
            lines: Arc::new(lines),
            used: self.appendable(),
            at: Arc::new(at),
        };
        let mut listings = self.listings();
        listings.edits.clear();
        listings.edited = listings.expanded;
        listings.landed = listings.folders;
        if shape.tree {
            listings.tree = Some(built.clone());
        }
        if !shape.tree || shape.expanded.is_empty() {
            listings.flat = Some(built);
        }
    }

    /// Lifts the cached listing out, so extending it does not copy what the cache still holds.
    /// A read hands one vector to both listings, and only its last holder can be carried through.
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
        let at = Arc::make_mut(&mut built.at);
        self.shape(tree)
            .collect_from(&self.held(), id, used, 0, lines, at);
        built.used = Some(rows);
        Some(built)
    }

    /// A disclose the tree listing is carried through rather than built again, once the frame
    /// that drew it is gone and the lines it names are still where they were.
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
            at: Arc::new(marks),
        })
    }

    /// A listing that landed under the rows disclosing it, spliced in where they sit. Anything
    /// else that moved leaves the cache to see the change and empty itself.
    pub(crate) fn landed(&self, id: &'static str) {
        let mut listings = self.cache.borrow_mut();
        if listings.landed + 1 != self.folders.changed() || listings.edited != self.expanded.rev() {
            return;
        }
        let Some(built) = listings.tree.as_ref() else {
            return;
        };
        let edits: Vec<Edit> = built
            .at
            .iter()
            .filter(|(_, held)| *held == id)
            .filter_map(|(at, _)| {
                built
                    .lines
                    .get(*at)
                    .map(|line| (*at, line.key.clone(), Some(id)))
            })
            .collect();
        if edits.is_empty() {
            return;
        }
        listings.landed += 1;
        listings.edits.extend(edits);
    }

    /// The discloses a key press made, to splice into the tree listing the frame it drew held.
    pub(crate) fn disclosed(&self, was: u64, edits: Vec<Edit>) {
        let mut listings = self.cache.borrow_mut();
        if listings.expanded == was && listings.edits.is_empty() && listings.tree.is_some() {
            listings.edited = self.expanded.rev();
            listings.edits = edits;
        }
    }

    /// The cache every listing this frame comes out of, emptied when what it holds is stale.
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
                landed: folders,
                ..Default::default()
            };
            return listings;
        }
        if listings.folders != folders {
            listings.folders = folders;
            listings.flat = None;
            listings.columns.clear();
            listings.cursor = None;
            if listings.landed != folders {
                listings.edits.clear();
                listings.tree = None;
            }
        }
        if listings.expanded != expanded {
            listings.expanded = expanded;
            listings.cursor = None;
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

    fn build_lines(&self, tree: bool) -> Walk {
        self.shape(tree).build(&self.held())
    }

    /// The rows of a folder as the listings in memory hold them.
    fn held<'a>(&'a self) -> impl Fn(&'static str) -> Option<&'a [Arc<Row>]> + 'a {
        |id| self.folders.get(id).map(|folder| folder.rows.as_slice())
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
