//! The arena the index answers from: one fixed-size node per entry, names interned once, a
//! directory's children a linked list in listing order.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::entry::{Entry, EntryKind};

pub const NONE: u32 = u32::MAX;
pub const ROOT: u32 = 0;

pub const KIND: u8 = 0b11;
pub const HIDDEN: u8 = 1 << 2;
pub const PACKAGE: u8 = 1 << 3;
pub const DELETED: u8 = 1 << 4;

pub const LISTED: u8 = 1 << 0;
pub const VISITED: u8 = 1 << 1;
pub const COMPLETE: u8 = 1 << 2;

/// Doubling a vector of millions of entries costs more resident memory than the index itself.
const CHUNK: usize = 1 << 19;

fn reserve<T>(items: &mut Vec<T>, more: usize) {
    if items.len() + more > items.capacity() {
        items.reserve_exact(CHUNK.max(more));
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Node {
    pub size: u64,
    pub modified: i64,
    pub created: i64,
    pub added: i64,
    pub parent: u32,
    pub first_child: u32,
    pub next_sibling: u32,
    pub flags: u8,
    pub state: u8,
    pub depth: u8,
    /// The snapshot writes these structs as bytes, so the tail has to be a field: padding the
    /// compiler inserts is never written and reading it back would be reading uninitialized memory.
    pub pad: [u8; 1],
}

const _: () = assert!(std::mem::size_of::<Node>() == 48);

impl Node {
    pub fn kind(&self) -> EntryKind {
        match self.flags & KIND {
            0 => EntryKind::File,
            1 => EntryKind::Directory,
            2 => EntryKind::Symlink,
            _ => EntryKind::Other,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.flags & KIND == 1
    }

    pub fn deleted(&self) -> bool {
        self.flags & DELETED != 0
    }
}

fn kind_bits(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::File => 0,
        EntryKind::Directory => 1,
        EntryKind::Symlink => 2,
        EntryKind::Other => 3,
    }
}

/// A time the volume does not record. Zero is 1970 and negative is older than that, so a
/// missing time needs a value of its own.
pub const NO_TIME: i64 = i64::MIN;

/// Seconds from the UNIX epoch, signed: a file older than 1970 keeps its date.
pub fn secs(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(since) => since.as_secs() as i64,
        Err(before) => -(before.duration().as_secs() as i64),
    }
}

pub fn time(s: i64) -> SystemTime {
    let gap = Duration::from_secs(s.unsigned_abs());
    match s < 0 {
        true => UNIX_EPOCH.checked_sub(gap),
        false => UNIX_EPOCH.checked_add(gap),
    }
    .unwrap_or(UNIX_EPOCH)
}

/// The number of intern tables. Growing one costs a shard of a rehash, not a whole one.
const SHARDS: usize = 64;

/// Every distinct name once, with a letter set per name so a search can reject most of them
/// without looking at their bytes.
#[derive(Default)]
pub struct Names {
    pub blob: Vec<u8>,
    pub ends: Vec<u32>,
    mask: Vec<u64>,
    tables: Vec<Vec<u32>>,
    counts: Vec<u32>,
}

fn hash(bytes: &[u8]) -> u32 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h = (h ^ b as u64).wrapping_mul(0x1000_0000_01b3);
    }
    (h ^ (h >> 32)) as u32
}

/// What a search matches against: ASCII as it stands, anything else case-, width- and
/// diacritic-folded, so "cafe" and "café" are one string however either is composed.
pub fn folded(bytes: &[u8]) -> Cow<'_, [u8]> {
    match bytes.is_ascii() {
        true => Cow::Borrowed(bytes),
        false => Cow::Owned(crate::sort::fold(&String::from_utf8_lossy(bytes)).into_bytes()),
    }
}

/// Set on a name whose bytes are not ASCII, so a search folds only those. A query never sets it:
/// it is folded before its own mask is taken, and an ASCII query matches a folded name.
pub const NEEDS_FOLD: u64 = 1 << 28;

/// Set on a name starting with a dot. Spotlight indexes neither a dot-file nor anything inside a
/// dot-folder, so neither turns up in Finder's search; `~/Library` carries the hidden flag and is
/// indexed all the same, which is why this reads the name rather than the flag.
pub const DOTTED: u64 = 1 << 29;

/// Which pairs of neighbouring bytes a name holds, hashed into 32 buckets: a query whose pairs
/// a name does not all hold cannot be in it, which throws out far more names than its letters do.
pub fn grams(bytes: &[u8]) -> u32 {
    let folded = folded(bytes);
    let mut m = 0;
    for pair in folded.windows(2) {
        let (a, b) = (pair[0] | 0x20, pair[1] | 0x20);
        m |= 1 << ((a as u32).wrapping_mul(31).wrapping_add(b as u32) & 31);
    }
    m
}

/// What a name is filtered by, and what a query asks of it: the letters, the pairs, and — for a
/// name — the two flags a query never sets.
pub fn mask_of(bytes: &[u8]) -> u64 {
    letters(bytes) as u64 | (grams(bytes) as u64) << 32
}

pub fn letters(bytes: &[u8]) -> u32 {
    let mut m = 0;
    for &b in folded(bytes).iter() {
        m |= match b | 0x20 {
            c @ b'a'..=b'z' => 1 << (c - b'a'),
            b'0'..=b'9' if b.is_ascii_digit() => 1 << 26,
            _ => 1 << 27,
        };
    }
    m
}

fn fold_bit(bytes: &[u8]) -> u64 {
    let fold = match bytes.is_ascii() {
        true => 0,
        false => NEEDS_FOLD,
    };
    match bytes.first() {
        Some(b'.') => fold | DOTTED,
        _ => fold,
    }
}

impl Names {
    pub fn with_blob(blob: Vec<u8>, ends: Vec<u32>) -> Names {
        Names {
            blob,
            ends,
            ..Names::default()
        }
    }

    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn bytes(&self, id: u32) -> &[u8] {
        let id = id as usize;
        let start = if id == 0 {
            0
        } else {
            self.ends[id - 1] as usize
        };
        &self.blob[start..self.ends[id] as usize]
    }

    pub fn masks(&self) -> &[u64] {
        &self.mask
    }

    pub fn intern(&mut self, name: &str) -> u32 {
        let bytes = name.as_bytes();
        let hash = hash(bytes);
        let shard = hash as usize % SHARDS;
        if self.tables.is_empty() {
            self.tables = vec![Vec::new(); SHARDS];
            self.counts = vec![0; SHARDS];
        }
        if (self.counts[shard] as usize + 1) * 4 > self.tables[shard].len() * 3 {
            self.grow(shard);
        }
        let slot = self.slot(shard, hash, bytes);
        if self.tables[shard][slot] != NONE {
            return self.tables[shard][slot];
        }
        let id = self.ends.len() as u32;
        reserve(&mut self.blob, bytes.len());
        reserve(&mut self.ends, 1);
        reserve(&mut self.mask, 1);
        self.blob.extend_from_slice(bytes);
        self.ends.push(self.blob.len() as u32);
        self.mask.push(mask_of(bytes) | fold_bit(bytes) as u64);
        self.tables[shard][slot] = id;
        self.counts[shard] += 1;
        id
    }

    fn slot(&self, shard: usize, hash: u32, bytes: &[u8]) -> usize {
        let table = &self.tables[shard];
        let mask = table.len() - 1;
        let mut at = (hash >> 8) as usize & mask;
        while table[at] != NONE && self.bytes(table[at]) != bytes {
            at = (at + 1) & mask;
        }
        at
    }

    fn grow(&mut self, shard: usize) {
        let size = ((self.counts[shard] as usize + 1) * 2)
            .next_power_of_two()
            .max(64);
        let old = std::mem::take(&mut self.tables[shard]);
        let live = old
            .into_iter()
            .filter(|&id| id != NONE)
            .map(|id| (hash(self.bytes(id)), id));
        self.tables[shard] = self.place(live, size);
    }

    fn place(&self, ids: impl Iterator<Item = (u32, u32)>, size: usize) -> Vec<u32> {
        let mut table = vec![NONE; size];
        let mask = size - 1;
        for (hash, id) in ids {
            let mut at = (hash >> 8) as usize & mask;
            while table[at] != NONE {
                at = (at + 1) & mask;
            }
            table[at] = id;
        }
        table
    }

    /// Rebuilds what the snapshot does not carry: the letter sets and the intern tables.
    pub fn reindex(&mut self) {
        self.mask = (0..self.ends.len() as u32)
            .map(|id| {
                let bytes = self.bytes(id);
                mask_of(bytes) | fold_bit(bytes) as u64
            })
            .collect();
        let hashes: Vec<u32> = (0..self.ends.len() as u32)
            .map(|id| hash(self.bytes(id)))
            .collect();
        self.counts = vec![0; SHARDS];
        for &hash in &hashes {
            self.counts[hash as usize % SHARDS] += 1;
        }
        self.tables = self
            .counts
            .iter()
            .map(|&count| vec![NONE; ((count as usize + 1) * 2).next_power_of_two().max(64)])
            .collect();
        for (id, &hash) in hashes.iter().enumerate() {
            let table = &mut self.tables[hash as usize % SHARDS];
            let mask = table.len() - 1;
            let mut at = (hash >> 8) as usize & mask;
            while table[at] != NONE {
                at = (at + 1) & mask;
            }
            table[at] = id as u32;
        }
    }
}

/// What one crawled root was left in: enough to ask FSEvents for the changes since, and a cheap
/// second opinion when the log cannot answer.
#[derive(Clone, Debug)]
pub struct Root {
    pub path: PathBuf,
    /// The FSEvents id this root's subtree is current to.
    pub event_id: u64,
    /// The root directory's own mtime when it was last read.
    pub modified: i64,
}

pub struct Relist {
    dir: u32,
    /// The children before the relist, by name, so a name still there keeps its node. Sorted and
    /// taken from in place: a directory of 20 000 entries is one allocation, not a hash table.
    old: Vec<(u32, u32)>,
    ids: Vec<u32>,
}

impl Relist {
    fn take(&mut self, name: u32) -> Option<u32> {
        let at = self
            .old
            .binary_search_by_key(&name, |&(name, _)| name)
            .ok()?;
        let id = std::mem::replace(&mut self.old[at].1, NONE);
        (id != NONE).then_some(id)
    }
}

pub struct Tree {
    pub nodes: Vec<Node>,
    pub name_of: Vec<u32>,
    pub names: Names,
    pub dirs: u64,
    pub files: u64,
    pub bytes: u64,
    pub crawled_at: Option<SystemTime>,
    pub event_id: u64,
    pub roots: Vec<Root>,
}

impl Default for Tree {
    fn default() -> Tree {
        let mut names = Names::default();
        let root = names.intern("");
        Tree {
            nodes: vec![Node {
                size: 0,
                modified: 0,
                created: 0,
                added: NO_TIME,
                parent: NONE,
                first_child: NONE,
                next_sibling: NONE,
                flags: 1,
                state: 0,
                depth: 0,
                pad: [0; 1],
            }],
            name_of: vec![root],
            names,
            dirs: 0,
            files: 0,
            bytes: 0,
            crawled_at: None,
            event_id: 0,
            roots: Vec::new(),
        }
    }
}

impl Tree {
    pub fn root_state(&self, path: &Path) -> Option<&Root> {
        self.roots.iter().find(|root| root.path == path)
    }

    /// Records what a finished walk leaves a root at, replacing any earlier state for it.
    pub fn set_root(&mut self, path: &Path, event_id: u64, modified: i64) {
        match self.roots.iter_mut().find(|root| root.path == path) {
            Some(root) => (root.event_id, root.modified) = (event_id, modified),
            None => self.roots.push(Root {
                path: path.to_path_buf(),
                event_id,
                modified,
            }),
        }
    }

    /// Whether the index has read this directory's entries.
    pub fn listed(&self, path: &Path) -> bool {
        self.find(path).is_some_and(|id| {
            let node = self.nodes[id as usize];
            node.is_dir()
                && !node.deleted()
                && node.flags & PACKAGE == 0
                && node.state & LISTED != 0
        })
    }

    /// The node for an absolute path, if the index has one.
    pub fn find(&self, path: &Path) -> Option<u32> {
        let mut cur = ROOT;
        for part in path.components() {
            match part {
                Component::RootDir => cur = ROOT,
                Component::CurDir => {}
                Component::Normal(name) => cur = self.child(cur, name.as_bytes())?,
                _ => return None,
            }
        }
        Some(cur)
    }

    pub fn child(&self, dir: u32, name: &[u8]) -> Option<u32> {
        self.children(dir)
            .find(|&id| self.names.bytes(self.name_of[id as usize]) == name)
    }

    pub fn children(&self, dir: u32) -> impl Iterator<Item = u32> + '_ {
        let mut cur = self.nodes[dir as usize].first_child;
        let mut left = self.nodes.len();
        std::iter::from_fn(move || {
            while cur != NONE && left > 0 {
                let id = cur;
                left -= 1;
                cur = self.nodes[id as usize].next_sibling;
                if !self.nodes[id as usize].deleted() {
                    return Some(id);
                }
            }
            None
        })
    }

    /// The node for `path`, creating the directories on the way as unvisited stubs.
    pub fn ensure(&mut self, path: &Path) -> u32 {
        let mut cur = ROOT;
        for part in path.components() {
            let Component::Normal(name) = part else {
                continue;
            };
            let Some(name) = name.to_str() else {
                return cur;
            };
            match self.child(cur, name.as_bytes()) {
                Some(id) => cur = id,
                None => {
                    let id = self.push(cur, name, None, false);
                    self.nodes[id as usize].next_sibling = self.nodes[cur as usize].first_child;
                    self.nodes[cur as usize].first_child = id;
                    cur = id;
                }
            }
        }
        cur
    }

    fn push(&mut self, parent: u32, name: &str, entry: Option<&Entry>, sealed: bool) -> u32 {
        let name = self.names.intern(name);
        let id = self.nodes.len() as u32;
        reserve(&mut self.nodes, 1);
        reserve(&mut self.name_of, 1);
        let depth = self.nodes[parent as usize].depth.saturating_add(1);
        let node = Node {
            size: 0,
            modified: 0,
            created: 0,
            added: NO_TIME,
            parent,
            first_child: NONE,
            next_sibling: NONE,
            flags: 1,
            state: 0,
            depth,
            pad: [0; 1],
        };
        self.nodes.push(node);
        self.name_of.push(name);
        if let Some(entry) = entry {
            self.write(id, entry, sealed);
        }
        id
    }

    fn write(&mut self, id: u32, entry: &Entry, sealed: bool) {
        let was_dir = self.nodes[id as usize].is_dir();
        let node = &mut self.nodes[id as usize];
        node.flags = kind_bits(entry.kind)
            | if entry.hidden { HIDDEN } else { 0 }
            | if entry.package { PACKAGE } else { 0 };
        node.modified = secs(entry.modified);
        node.created = secs(entry.created);
        node.added = entry.added.map_or(NO_TIME, secs);
        if !node.is_dir() {
            node.size = entry.size;
        }
        if sealed {
            node.state |= VISITED;
        }
        if was_dir && !self.nodes[id as usize].is_dir() {
            let kids: Vec<u32> = self.children(id).collect();
            self.nodes[id as usize].first_child = NONE;
            for kid in kids {
                self.tombstone(kid);
            }
        }
    }

    pub fn tombstone(&mut self, id: u32) {
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            self.nodes[id as usize].flags |= DELETED;
            let mut cur = self.nodes[id as usize].first_child;
            let mut left = self.nodes.len();
            while cur != NONE && left > 0 {
                stack.push(cur);
                left -= 1;
                cur = self.nodes[cur as usize].next_sibling;
            }
        }
    }

    /// A directory's children rewritten in batches: the old list stays whole for readers until
    /// [`Tree::end_relist`] swaps in the new one.
    pub fn start_relist(&self, dir: u32) -> Relist {
        let mut old: Vec<(u32, u32)> = self
            .children(dir)
            .map(|id| (self.name_of[id as usize], id))
            .collect();
        old.sort_unstable();
        Relist {
            dir,
            old,
            ids: Vec::new(),
        }
    }

    /// `sealed[i]` marks a child the crawl will not list: a package, a skip, a symlink.
    pub fn relist(&mut self, state: &mut Relist, entries: &[Entry], sealed: &[bool]) {
        for (i, entry) in entries.iter().enumerate() {
            let name = self.names.intern(&entry.name);
            let id = match state.take(name) {
                Some(id) => {
                    self.write(id, entry, sealed[i]);
                    id
                }
                None => self.push(state.dir, &entry.name, Some(entry), sealed[i]),
            };
            state.ids.push(id);
        }
    }

    pub fn end_relist(&mut self, state: Relist) -> Vec<u32> {
        let Relist { dir, old, ids } = state;
        for pair in ids.windows(2) {
            self.nodes[pair[0] as usize].next_sibling = pair[1];
        }
        if let Some(&last) = ids.last() {
            self.nodes[last as usize].next_sibling = NONE;
        }
        self.nodes[dir as usize].first_child = ids.first().copied().unwrap_or(NONE);
        for (_, id) in old {
            if id != NONE {
                self.tombstone(id);
            }
        }
        self.nodes[dir as usize].state |= LISTED | VISITED;
        ids
    }

    pub fn add_size(&mut self, id: u32, bytes: u64) {
        self.nodes[id as usize].size += bytes;
    }

    /// Whether enough of the tree is tombstones to be worth rebuilding it.
    pub fn worth_compacting(&self) -> bool {
        let dead = self.nodes.iter().filter(|node| node.deleted()).count();
        dead * 8 > self.nodes.len()
    }

    /// Drops the tombstoned nodes and the names nothing is left pointing at, renumbering what
    /// stays. A tombstone is never reused, so a machine that churns files grows the snapshot
    /// without bound, and every launch reads the dead weight back.
    pub fn compact(&mut self) {
        for id in 0..self.nodes.len() {
            let mut cur = self.nodes[id].first_child;
            let mut prev = NONE;
            let mut left = self.nodes.len();
            while cur != NONE && left > 0 {
                left -= 1;
                let next = self.nodes[cur as usize].next_sibling;
                if !self.nodes[cur as usize].deleted() {
                    match prev {
                        NONE => self.nodes[id].first_child = cur,
                        prev => self.nodes[prev as usize].next_sibling = cur,
                    }
                    prev = cur;
                }
                cur = next;
            }
            match prev {
                NONE => self.nodes[id].first_child = NONE,
                prev => self.nodes[prev as usize].next_sibling = NONE,
            }
        }

        let mut moved = vec![NONE; self.nodes.len()];
        let mut kept = 0;
        for id in 0..self.nodes.len() {
            // A node whose parent went with a tombstone goes too: a parent is always an earlier
            // node, so by here its own fate is known.
            let orphan = id != ROOT as usize && moved[self.nodes[id].parent as usize] == NONE;
            if self.nodes[id].deleted() || orphan {
                continue;
            }
            moved[id] = kept as u32;
            self.nodes[kept] = self.nodes[id];
            self.name_of[kept] = self.name_of[id];
            kept += 1;
        }
        self.nodes.truncate(kept);
        self.name_of.truncate(kept);
        let at = |id: u32| match id {
            NONE => NONE,
            id => moved[id as usize],
        };
        for id in 0..self.nodes.len() {
            let node = &mut self.nodes[id];
            node.parent = at(node.parent);
            node.first_child = at(node.first_child);
            node.next_sibling = at(node.next_sibling);
        }

        let mut renamed = vec![NONE; self.names.len()];
        for &name in &self.name_of {
            renamed[name as usize] = 0;
        }
        let was = std::mem::take(&mut self.names.ends);
        let mut ends = Vec::with_capacity(was.len());
        let mut blob = 0;
        for (name, &end) in was.iter().enumerate() {
            if renamed[name] == NONE {
                continue;
            }
            let start = if name == 0 { 0 } else { was[name - 1] as usize };
            renamed[name] = ends.len() as u32;
            // A name's new home is never above its old one, so the blob shifts down in place.
            self.names.blob.copy_within(start..end as usize, blob);
            blob += end as usize - start;
            ends.push(blob as u32);
        }
        self.names.blob.truncate(blob);
        self.names.ends = ends;
        self.names.reindex();
        for name in &mut self.name_of {
            *name = renamed[*name as usize];
        }
    }

    /// Folds every subtree's bytes into its directory and marks the subtrees the crawl finished.
    /// A child's index is always above its parent's, so one reverse pass is a bottom-up walk.
    pub fn recompute(&mut self) {
        for node in &mut self.nodes {
            if node.is_dir() && node.flags & PACKAGE == 0 {
                node.size = 0;
            }
            match node.is_dir() && node.state & VISITED == 0 {
                true => node.state &= !COMPLETE,
                false => node.state |= COMPLETE,
            }
        }
        let (mut dirs, mut files) = (0, 0);
        for id in (1..self.nodes.len()).rev() {
            let node = self.nodes[id];
            if node.deleted() {
                continue;
            }
            match node.is_dir() && node.flags & PACKAGE == 0 {
                true => dirs += 1,
                false => files += 1,
            }
            let parent = node.parent as usize;
            if parent >= self.nodes.len() {
                continue;
            }
            self.nodes[parent].size += node.size;
            if node.state & COMPLETE == 0 {
                self.nodes[parent].state &= !COMPLETE;
            }
        }
        self.dirs = dirs;
        self.files = files;
        self.bytes = self.nodes[ROOT as usize].size;
    }

    /// Adds `delta` to every directory above `id`, for a change already applied below.
    pub fn propagate(&mut self, id: u32, delta: i64) {
        let mut cur = self.nodes[id as usize].parent;
        while cur != NONE {
            let node = &mut self.nodes[cur as usize];
            node.size = node.size.saturating_add_signed(delta);
            cur = node.parent;
        }
        self.bytes = self.nodes[ROOT as usize].size;
    }

    pub fn entry(&self, id: u32) -> Entry {
        let node = self.nodes[id as usize];
        Entry {
            name: String::from_utf8_lossy(self.names.bytes(self.name_of[id as usize])).into_owned(),
            kind: node.kind(),
            size: node.size,
            modified: time(node.modified),
            created: time(node.created),
            added: (node.added != NO_TIME).then(|| time(node.added)),
            hidden: node.flags & HIDDEN != 0,
            package: node.flags & PACKAGE != 0,
            dataless: false,
            file_id: 0,
            children: (node.state & LISTED != 0).then(|| self.children(id).count() as u32),
        }
    }

    pub fn path(&self, id: u32) -> PathBuf {
        let mut parts = Vec::new();
        let mut cur = id;
        while cur != ROOT && cur != NONE {
            parts.push(self.names.bytes(self.name_of[cur as usize]));
            cur = self.nodes[cur as usize].parent;
        }
        let mut path = PathBuf::from("/");
        for part in parts.iter().rev() {
            path.push(OsStr::from_bytes(part));
        }
        path
    }

    pub fn is_under(&self, id: u32, root: u32) -> bool {
        let mut cur = id;
        while cur != NONE {
            if cur == root {
                return true;
            }
            if cur == ROOT {
                return false;
            }
            cur = self.nodes[cur as usize].parent;
        }
        false
    }
}
