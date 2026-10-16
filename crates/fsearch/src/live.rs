//! The index as it is now: the immutable base, a bitset of dead base entries, and an overlay of entries added since.

use crate::index::Index;
use crate::walk::{self, FLAG_HARDLINK, FLAG_HIDDEN, FLAG_MOUNT, KIND_DIR, Listing, NONE, RawEnt};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

#[derive(Clone, Copy)]
pub struct OEnt {
    pub kind: u8,
    pub size: u64,
    /// Bytes allocated on disk.
    pub alloc: u64,
    pub mtime: u32,
    /// `index::name_mask` of the entry's name.
    pub mask: u64,
    /// Location prior of the nearest folder the base knows.
    pub prior: i8,
    slot: u32,
}

impl OEnt {
    /// `path` may be the whole path or just the name.
    pub fn new(path: &[u8], kind: u8, size: u64, alloc: u64, mtime: u32) -> OEnt {
        let name = &path[path.iter().rposition(|&b| b == b'/').map_or(0, |p| p + 1)..];
        OEnt { kind, size, alloc, mtime, mask: crate::index::name_mask(name), prior: 0, slot: 0 }
    }
}

/// The overlay laid out for searches; a free slot's mask is 0, which no name has.
#[derive(Default)]
pub struct Flat {
    pub masks: Vec<u64>,
    pub items: Vec<(Vec<u8>, OEnt)>,
    free: Vec<u32>,
}

impl Flat {
    fn set(&mut self, path: &[u8], e: OEnt) {
        let s = e.slot as usize;
        if s == self.masks.len() {
            self.masks.push(0);
            self.items.push((Vec::new(), e));
        }
        self.masks[s] = e.mask;
        if self.items[s].0 != path {
            self.items[s].0 = path.to_vec();
        }
        self.items[s].1 = e;
    }

    fn clear(&mut self, e: OEnt) {
        self.masks[e.slot as usize] = 0;
        self.items[e.slot as usize].0 = Vec::new();
        self.free.push(e.slot);
    }
}

pub struct Live {
    pub base: Index,
    dead: Vec<u64>,
    pub dead_count: usize,
    pub over: BTreeMap<Vec<u8>, OEnt>,
    pub flat: Flat,
    /// Last FSEvents id fully applied.
    pub event_id: u64,
    /// Directories added or removed since last drained, for the content index to re-sync.
    pub trees: Vec<Vec<u8>>,
    /// The last name search's scored names.
    pub names_cache: crate::query::NameCache,
    /// Wall-clock second up to which every change is known applied.
    pub synced_at: u32,
    priors: HashMap<Vec<u8>, i8>,
    dirs: HashMap<u32, (u8, u32)>,
}

/// What `Live::fetch` read from disk for one folder update.
pub struct Fetched {
    path: Vec<u8>,
    recursive: bool,
    blocked: bool,
    attrs: Option<OEnt>,
    listing: Option<Listing>,
    scans: HashMap<Vec<u8>, Vec<Listing>>,
}

pub enum Applied {
    Done,
    /// The whole disk needs rescanning.
    Rebuild,
}

/// What a folder holds, as the index has it now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// Logical bytes of everything below it.
    pub bytes: u64,
    pub allocated: u64,
    pub items: u64,
    /// Its children that are neither dot-named nor hidden.
    pub visible: u64,
    /// Entries below it with another hard link; each link counts in the totals.
    pub hardlinks: u64,
    /// Folders below it the index never lists; nothing inside them counts.
    pub unindexed: Vec<PathBuf>,
}

impl Live {
    pub fn new(base: Index) -> Live {
        base.prefault();
        let words = base.n.div_ceil(64);
        let event_id = base.event_id;
        let synced_at = base.synced_at;
        Live {
            base,
            dead: vec![0; words],
            dead_count: 0,
            over: BTreeMap::new(),
            flat: Flat::default(),
            event_id,
            trees: Vec::new(),
            names_cache: Default::default(),
            synced_at,
            priors: HashMap::new(),
            dirs: HashMap::new(),
        }
    }

    fn put(&mut self, path: Vec<u8>, mut e: OEnt) {
        let cut = path.iter().rposition(|&b| b == b'/').unwrap_or(0);
        e.prior = match self.priors.get(&path[..cut]) {
            Some(&p) => p,
            None => {
                let mut up = &path[..cut];
                let mut prior = 0;
                while !up.is_empty() {
                    if let Some(d) = self.base.lookup(up).and_then(|e| self.base.dir_of(e)) {
                        prior = self.base.dir_prior()[d as usize];
                        break;
                    }
                    up = &up[..up.iter().rposition(|&b| b == b'/').unwrap_or(0)];
                }
                self.priors.insert(path[..cut].to_vec(), prior);
                prior
            }
        };
        e.slot = match self.over.get(&path) {
            Some(old) => old.slot,
            None => self.flat.free.pop().unwrap_or(self.flat.masks.len() as u32),
        };
        self.flat.set(&path, e);
        self.over.insert(path, e);
    }

    #[inline]
    pub fn is_dead(&self, i: u32) -> bool {
        self.dead[i as usize >> 6] & (1 << (i & 63)) != 0
    }

    fn kill(&mut self, i: u32) {
        let w = &mut self.dead[i as usize >> 6];
        if *w & (1 << (i & 63)) == 0 {
            *w |= 1 << (i & 63);
            self.dead_count += 1;
        }
    }

    /// A base entry's kind, as it is now.
    pub fn kind_of(&self, i: u32) -> u8 {
        self.dirs.get(&i).map_or(self.base.kind()[i as usize], |d| d.0)
    }

    /// A base entry's mtime, as it is now.
    pub fn mtime_of(&self, i: u32) -> u32 {
        self.dirs.get(&i).map_or(self.base.mtime()[i as usize], |d| d.1)
    }

    fn redate(&mut self, i: u32, kind: u8, mtime: u32) {
        if (kind, mtime) == (self.base.kind()[i as usize], self.base.mtime()[i as usize]) {
            self.dirs.remove(&i);
        } else {
            self.dirs.insert(i, (kind, mtime));
        }
    }

    fn kill_subtree(&mut self, e: u32) {
        self.kill(e);
        if let Some(d) = self.base.dir_of(e) {
            let (a, b) = (self.base.dir_start()[d as usize], self.base.dir_end()[d as usize]);
            for i in a..b {
                self.kill(i);
            }
        }
    }

    fn drop_over_subtree(&mut self, path: &[u8]) {
        if let Some(o) = self.over.remove(path) {
            self.flat.clear(o);
        }
        let (lo, hi) = subtree_bounds(path);
        let keys: Vec<Vec<u8>> = self.over.range(lo..hi).map(|(k, _)| k.clone()).collect();
        for k in keys {
            if let Some(o) = self.over.remove(&k) {
                self.flat.clear(o);
            }
        }
    }

    fn base_alive(&self, path: &[u8]) -> Option<u32> {
        self.base.lookup(path).filter(|&e| !self.is_dead(e))
    }

    fn remove_path(&mut self, path: &[u8]) {
        self.trees.push(path.to_vec());
        if let Some(e) = self.base_alive(path) {
            self.kill_subtree(e);
        }
        self.drop_over_subtree(path);
    }

    /// Bring one directory (or, with `recursive`, its whole subtree) in line with the disk.
    pub fn apply_dir(&mut self, path: &[u8], recursive: bool) -> Applied {
        let f = self.fetch(path, recursive);
        self.apply(f)
    }

    /// The disk half of `apply_dir`, which needs only `&self`.
    pub fn fetch(&self, path: &[u8], recursive: bool) -> Fetched {
        let p = normalize(path);
        let mut f = Fetched { path: p, recursive, blocked: false, attrs: None, listing: None, scans: HashMap::new() };
        if walk::blocked(&f.path) {
            f.blocked = true;
            return f;
        }
        if recursive {
            if f.path != b"/" {
                f.attrs = lstat(&f.path);
                if f.attrs.is_some_and(|a| a.kind & 3 == KIND_DIR && a.kind & FLAG_MOUNT == 0) {
                    f.scans.insert(f.path.clone(), walk::scan(&f.path, 4));
                }
            }
            return f;
        }
        f.attrs = lstat(&f.path);
        let Some(listing) = walk::list_one(&f.path) else { return f };
        let cur = self.current_children(&f.path);
        for r in &listing.ents {
            if r.kind & 3 != KIND_DIR || r.kind & FLAG_MOUNT != 0 {
                continue;
            }
            let name = &listing.names[r.name_off as usize..][..r.name_len as usize];
            let was_dir = match cur.get(name) {
                Some(Some(c)) => self.base.kind()[*c as usize] & 3 == KIND_DIR,
                Some(None) => self.over.get(&join(&f.path, name)).is_some_and(|o| o.kind & 3 == KIND_DIR),
                None => false,
            };
            if !was_dir {
                let child = join(&f.path, name);
                let ls = walk::scan(&child, 4);
                f.scans.insert(child, ls);
            }
        }
        f.listing = Some(listing);
        f
    }

    fn current_children(&self, p: &[u8]) -> HashMap<Vec<u8>, Option<u32>> {
        let mut cur: HashMap<Vec<u8>, Option<u32>> = HashMap::new();
        if let Some(d) = self.base_alive(p).and_then(|e| self.base.dir_of(e)) {
            for c in self.base.children(d) {
                if !self.is_dead(c as u32) {
                    cur.insert(self.base.name(c).to_vec(), Some(c as u32));
                }
            }
        }
        let (lo, hi) = subtree_bounds(p);
        let plen = lo.len();
        for k in self.over.range(lo..hi).map(|(k, _)| k) {
            if !k[plen..].contains(&b'/') {
                cur.insert(k[plen..].to_vec(), None);
            }
        }
        cur
    }

    /// The memory half of `apply_dir`: diff what `fetch` read into the index.
    pub fn apply(&mut self, mut f: Fetched) -> Applied {
        if f.blocked {
            return Applied::Done;
        }
        let p = std::mem::take(&mut f.path);
        if f.recursive {
            if p == b"/" {
                return Applied::Rebuild;
            }
            self.remove_path(&p);
            if let Some(attrs) = f.attrs {
                let scan = f.scans.remove(&p);
                self.add_new(p, attrs, scan);
            }
            return Applied::Done;
        }
        let Some(listing) = f.listing else {
            if f.attrs.is_none() {
                self.remove_path(&p);
            }
            return Applied::Done;
        };
        let mut cur = self.current_children(&p);
        for r in &listing.ents {
            let name = &listing.names[r.name_off as usize..][..r.name_len as usize];
            let child = join(&p, name);
            let now = OEnt::new(name, r.kind, r.size, r.alloc, r.mtime);
            match cur.remove(name) {
                None => {
                    let scan = f.scans.remove(&child);
                    self.add_new(child, now, scan)
                }
                Some(Some(c)) => {
                    let c = c as usize;
                    let was_kind = self.base.kind()[c];
                    if was_kind & 3 != now.kind & 3 {
                        self.kill_subtree(c as u32);
                        let scan = f.scans.remove(&child);
                        self.add_new(child, now, scan);
                    } else if now.kind & 3 == KIND_DIR {
                        self.redate(c as u32, now.kind, now.mtime);
                    } else if (was_kind, self.base.size_of(c), self.base.alloc_of(c), self.base.mtime()[c])
                        != (now.kind, now.size, now.alloc, now.mtime)
                    {
                        self.kill(c as u32);
                        self.put(child, now);
                    }
                }
                Some(None) => {
                    let old = self.over[&child];
                    if old.kind & 3 != now.kind & 3 {
                        self.drop_over_subtree(&child);
                        let scan = f.scans.remove(&child);
                        self.add_new(child, now, scan);
                    } else if (old.kind, old.size, old.alloc, old.mtime) != (now.kind, now.size, now.alloc, now.mtime) {
                        self.put(child, now);
                    }
                }
            }
        }
        for (name, c) in cur {
            let child = join(&p, &name);
            match c {
                Some(c) => {
                    if self.base.kind()[c as usize] & 3 == KIND_DIR {
                        self.trees.push(child);
                    }
                    self.kill_subtree(c)
                }
                None => {
                    if self.over.get(&child).is_some_and(|o| o.kind & 3 == KIND_DIR) {
                        self.trees.push(child.clone());
                    }
                    self.drop_over_subtree(&child)
                }
            }
        }
        // FSEvents names only the folder, not its parent, so the folder's own entry is updated here.
        if let Some(a) = f.attrs {
            let hidden = |kind: u8| (kind & !FLAG_HIDDEN) | (a.kind & FLAG_HIDDEN);
            match self.base_alive(&p) {
                Some(e) => self.redate(e, hidden(self.kind_of(e)), a.mtime),
                None => {
                    if let Some(&o) = self.over.get(&p) {
                        self.put(p, OEnt { kind: hidden(o.kind), mtime: a.mtime, ..o });
                    }
                }
            }
        }
        Applied::Done
    }

    fn add_new(&mut self, path: Vec<u8>, e: OEnt, scan: Option<Vec<Listing>>) {
        let is_dir = e.kind & 3 == KIND_DIR && e.kind & FLAG_MOUNT == 0;
        self.put(path.clone(), e);
        if !is_dir {
            return;
        }
        self.trees.push(path.clone());
        let ls = scan.unwrap_or_else(|| walk::scan(&path, 4));
        for_each_path(&ls, &path, |p, r| {
            let e = OEnt::new(&p, r.kind, r.size, r.alloc, r.mtime);
            self.put(p, e);
        });
    }

    /// Folders whose mtime is at least `since`: adding, removing or renaming an entry bumps its folder's mtime.
    pub fn changed_dirs(&self, since: u32) -> Vec<Vec<u8>> {
        use rayon::prelude::*;
        let idx = &self.base;
        let de = idx.dir_entry();
        let check = |p: &[u8]| !walk::blocked(p) && lstat(p).is_some_and(|o| o.mtime >= since);
        let pool = stat_pool();
        let mut out: Vec<Vec<u8>> = pool.install(|| {
            (1..idx.d)
                .into_par_iter()
                .with_min_len(1024)
                .filter(|&k| !self.is_dead(de[k]))
                .map_init(Vec::new, |buf, k| {
                    idx.path(de[k] as usize, buf);
                    check(buf).then(|| buf.clone())
                })
                .flatten()
                .collect()
        });
        out.extend(self.over.iter().filter(|(p, o)| o.kind & 3 == KIND_DIR && o.kind & FLAG_MOUNT == 0 && check(p)).map(|(p, _)| p.clone()));
        if lstat(b"/").is_some_and(|o| o.mtime >= since) {
            out.push(b"/".to_vec());
        }
        out.sort();
        out
    }

    /// Everything alive, as listings for `Index::build`.
    pub fn to_listings(&self) -> Vec<Listing> {
        let mut by_parent: HashMap<&[u8], Vec<(&[u8], OEnt)>> = HashMap::new();
        for (k, v) in &self.over {
            let cut = k.iter().rposition(|&b| b == b'/').unwrap_or(0);
            let parent: &[u8] = if cut == 0 { b"/" } else { &k[..cut] };
            by_parent.entry(parent).or_default().push((&k[cut + 1..], *v));
        }
        let mut out = Vec::new();
        let mut stack: Vec<(Option<u32>, Vec<u8>, u32)> = vec![(Some(0), b"/".to_vec(), 0)];
        let mut next = 1u32;
        while let Some((bd, path, id)) = stack.pop() {
            let mut l = Listing { id, names: Vec::new(), ents: Vec::new() };
            let mut push = |l: &mut Listing, name: &[u8], kind: u8, size: u64, alloc: u64, mtime: u32, cp: Option<Vec<u8>>, bdir: Option<u32>| {
                let child = match cp {
                    Some(cp) => {
                        let c = next;
                        next += 1;
                        stack.push((bdir, cp, c));
                        c
                    }
                    None => NONE,
                };
                l.ents.push(RawEnt { name_off: l.names.len() as u32, name_len: name.len() as u16, kind, size, alloc, mtime, child });
                l.names.extend_from_slice(name);
            };
            if let Some(d) = bd {
                for c in self.base.children(d) {
                    if self.is_dead(c as u32) {
                        continue;
                    }
                    let name = self.base.name(c);
                    let k = self.kind_of(c as u32);
                    let sub = self.base.dir_of(c as u32);
                    let cp = sub.map(|_| join(&path, name));
                    push(&mut l, name, k, self.base.size_of(c), self.base.alloc_of(c), self.mtime_of(c as u32), cp, sub);
                }
            }
            if let Some(kids) = by_parent.get(path.as_slice()) {
                for &(name, o) in kids {
                    let descend = o.kind & 3 == KIND_DIR && o.kind & FLAG_MOUNT == 0;
                    push(&mut l, name, o.kind, o.size, o.alloc, o.mtime, descend.then(|| join(&path, name)), None);
                }
            }
            out.push(l);
        }
        out
    }

    fn folder(&self, p: &[u8]) -> Option<Option<u32>> {
        if !p.starts_with(b"/") {
            return None;
        }
        if let Some(e) = self.base_alive(p) {
            return self.base.dir_of(e).map(Some);
        }
        let o = self.over.get(p)?;
        (o.kind & 3 == KIND_DIR && o.kind & FLAG_MOUNT == 0 && !walk::blocked(p)).then_some(None)
    }

    pub fn lists(&self, path: &[u8]) -> bool {
        self.folder(&normalize(path)).is_some()
    }

    /// What the folder at `path` holds, if the index lists it.
    pub fn totals(&self, path: &[u8]) -> Option<Totals> {
        let p = normalize(path);
        let k = self.folder(&p)?;
        Some(self.total(&p, k))
    }

    pub fn folder_totals(&self, dir: &[u8]) -> Option<Vec<(Vec<u8>, Totals)>> {
        let p = normalize(dir);
        let k = self.folder(&p)?;
        let mut out = Vec::new();
        for c in k.map(|k| self.base.children(k)).into_iter().flatten() {
            let Some(ck) = self.base.dir_of(c as u32).filter(|_| !self.is_dead(c as u32)) else { continue };
            let cp = join(&p, self.base.name(c));
            let t = self.total(&cp, Some(ck));
            out.push((cp, t));
        }
        let (lo, hi) = subtree_bounds(&p);
        for (cp, o) in self.over.range(lo.clone()..hi) {
            if !cp[lo.len()..].contains(&b'/') && o.kind & 3 == KIND_DIR && o.kind & FLAG_MOUNT == 0 && !walk::blocked(cp) {
                out.push((cp.clone(), self.total(cp, None)));
            }
        }
        Some(out)
    }

    fn total(&self, p: &[u8], k: Option<u32>) -> Totals {
        let mut t = Totals::default();
        if let Some(k) = k {
            let (s, e) = (self.base.dir_start()[k as usize], self.base.dir_end()[k as usize]);
            t.bytes = self.base.dir_bytes()[k as usize];
            t.allocated = self.base.dir_alloc()[k as usize];
            t.items = (e - s) as u64;
            self.each_dead(s, e, |i| {
                t.bytes -= self.base.size_of(i);
                t.allocated -= self.base.alloc_of(i);
                t.items -= 1;
            });
            t.visible = self.base.children(k).filter(|&c| !self.is_dead(c as u32) && shown(self.base.name(c), self.kind_of(c as u32))).count() as u64;
            for &i in self.marked_in(s, e).iter().filter(|&&i| !self.is_dead(i)) {
                let kind = self.base.kind()[i as usize];
                t.hardlinks += (kind & FLAG_HARDLINK != 0) as u64;
                if kind & FLAG_MOUNT != 0 {
                    let mut b = Vec::new();
                    self.base.path(i as usize, &mut b);
                    t.unindexed.push(to_path(b));
                }
            }
        }
        let (lo, hi) = subtree_bounds(p);
        for (key, o) in self.over.range(lo.clone()..hi) {
            t.bytes += o.size;
            t.allocated += o.alloc;
            t.items += 1;
            let rest = &key[lo.len()..];
            t.visible += (!rest.contains(&b'/') && shown(rest, o.kind)) as u64;
            t.hardlinks += (o.kind & FLAG_HARDLINK != 0) as u64;
            if o.kind & FLAG_MOUNT != 0 {
                t.unindexed.push(to_path(key.clone()));
            }
        }
        for s in walk::SKIP.get().into_iter().flatten() {
            if s.starts_with(&lo) && (self.base_alive(s).is_some() || self.over.contains_key(s)) {
                t.unindexed.push(to_path(s.clone()));
            }
        }
        t
    }

    fn each_dead(&self, s: u32, e: u32, mut f: impl FnMut(usize)) {
        if self.dead_count == 0 {
            return;
        }
        let (s, e) = (s as usize, e as usize);
        let mut at = s & !63;
        while at < e {
            let mut bits = self.dead[at >> 6];
            if at < s {
                bits &= !0 << (s - at);
            }
            if e - at < 64 {
                bits &= (1 << (e - at)) - 1;
            }
            while bits != 0 {
                f(at + bits.trailing_zeros() as usize);
                bits &= bits - 1;
            }
            at += 64;
        }
    }

    fn marked_in(&self, s: u32, e: u32) -> &[u32] {
        let m = self.base.marked();
        &m[m.partition_point(|&i| i < s)..m.partition_point(|&i| i < e)]
    }

    pub fn hardlinks(&self, path: &[u8]) -> Vec<Vec<u8>> {
        let p = normalize(path);
        let Some(k) = self.folder(&p) else { return Vec::new() };
        let mut out = Vec::new();
        if let Some(k) = k {
            let (s, e) = (self.base.dir_start()[k as usize], self.base.dir_end()[k as usize]);
            for &i in self.marked_in(s, e) {
                if !self.is_dead(i) && self.base.kind()[i as usize] & FLAG_HARDLINK != 0 {
                    let mut b = Vec::new();
                    self.base.path(i as usize, &mut b);
                    out.push(b);
                }
            }
        }
        let (lo, hi) = subtree_bounds(&p);
        out.extend(self.over.range(lo..hi).filter(|(_, o)| o.kind & FLAG_HARDLINK != 0).map(|(k, _)| k.clone()));
        out
    }

    /// What the index holds directly in `dir`: (path, kind, size, mtime) each.
    #[allow(clippy::type_complexity)]
    pub fn children(&self, dir: &[u8]) -> Option<Vec<(Vec<u8>, u8, u64, u32)>> {
        let p = normalize(dir);
        let k = self.folder(&p)?;
        let mut out = Vec::new();
        for c in k.map(|k| self.base.children(k)).into_iter().flatten() {
            if !self.is_dead(c as u32) {
                out.push((join(&p, self.base.name(c)), self.kind_of(c as u32), self.base.size_of(c), self.mtime_of(c as u32)));
            }
        }
        let (lo, hi) = subtree_bounds(&p);
        out.extend(
            self.over.range(lo.clone()..hi).filter(|(cp, _)| !cp[lo.len()..].contains(&b'/')).map(|(cp, o)| (cp.clone(), o.kind, o.size, o.mtime)),
        );
        Some(out)
    }

    /// The index's record of one path: (kind, size, mtime).
    pub fn entry(&self, path: &[u8]) -> Option<(u8, u64, u32)> {
        let p = normalize(path);
        if !p.starts_with(b"/") {
            return None;
        }
        match self.base_alive(&p) {
            Some(e) => Some((self.kind_of(e), self.base.size_of(e as usize), self.mtime_of(e))),
            None => self.over.get(&p).map(|o| (o.kind, o.size, o.mtime)),
        }
    }

    /// One file per extension in the base, the most common first.
    pub fn extensions(&self, limit: usize) -> Vec<Vec<u8>> {
        let (off, ents) = (self.base.name_ents_off(), self.base.name_ents());
        let mut by_ext: HashMap<&[u8], (usize, u32)> = HashMap::new();
        for k in 0..self.base.u {
            let name = self.base.uname(k as u32);
            let Some(dot) = name.iter().rposition(|&b| b == b'.').filter(|&dot| dot > 0 && (2..=13).contains(&(name.len() - dot))) else { continue };
            let carrying = &ents[off[k] as usize..off[k + 1] as usize];
            let Some(&i) = carrying.iter().find(|&&i| self.base.kind()[i as usize] & 3 != KIND_DIR && !self.is_dead(i)) else { continue };
            by_ext.entry(&name[dot + 1..]).or_insert((0, i)).0 += carrying.len();
        }
        let mut common: Vec<(usize, u32)> = by_ext.into_values().collect();
        common.sort_unstable_by_key(|c| std::cmp::Reverse(c.0));
        common
            .into_iter()
            .take(limit)
            .map(|(_, i)| {
                let mut p = Vec::new();
                self.base.path(i as usize, &mut p);
                p
            })
            .collect()
    }
}

pub fn stat_pool() -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(12).start_handler(|_| crate::no_materialize()).build().unwrap()
}

pub fn for_each_path(ls: &[Listing], root: &[u8], mut f: impl FnMut(Vec<u8>, &RawEnt)) {
    let mut by_id: HashMap<u32, &Listing> = HashMap::with_capacity(ls.len());
    for l in ls {
        by_id.insert(l.id, l);
    }
    let mut stack = vec![(0u32, root.to_vec())];
    while let Some((id, path)) = stack.pop() {
        let Some(l) = by_id.get(&id) else { continue };
        for r in &l.ents {
            let p = join(&path, &l.names[r.name_off as usize..][..r.name_len as usize]);
            if r.child != NONE {
                stack.push((r.child, p.clone()));
            }
            f(p, r);
        }
    }
}

pub(crate) fn normalize(path: &[u8]) -> Vec<u8> {
    let mut p = path.to_vec();
    while p.len() > 1 && p.last() == Some(&b'/') {
        p.pop();
    }
    p
}

pub fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(dir.len() + 1 + name.len());
    p.extend_from_slice(dir);
    if dir != b"/" {
        p.push(b'/');
    }
    p.extend_from_slice(name);
    p
}

fn subtree_bounds(path: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let lo = join(path, b"");
    let mut hi = lo.clone();
    *hi.last_mut().unwrap() += 1; // '/' + 1 == '0'
    (lo, hi)
}

/// The Finder shows what is neither dot-named nor hidden.
fn shown(name: &[u8], kind: u8) -> bool {
    !name.starts_with(b".") && kind & FLAG_HIDDEN == 0
}

pub(crate) fn to_path(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes))
}

pub fn lstat(path: &[u8]) -> Option<OEnt> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::lstat(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let mut kind = match st.st_mode & libc::S_IFMT {
        libc::S_IFREG => walk::KIND_FILE,
        libc::S_IFDIR => KIND_DIR,
        libc::S_IFLNK => walk::KIND_LINK,
        _ => walk::KIND_OTHER,
    };
    if st.st_flags & libc::UF_HIDDEN != 0 {
        kind |= FLAG_HIDDEN;
    }
    if kind & 3 != KIND_DIR && st.st_nlink > 1 {
        kind |= FLAG_HARDLINK;
    }
    Some(OEnt::new(path, kind, st.st_size as u64, st.st_blocks as u64 * 512, st.st_mtime.clamp(0, u32::MAX as i64) as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::macos::fs::MetadataExt as _;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fsearch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(dir).unwrap()
    }

    fn bytes(p: &Path) -> &[u8] {
        p.as_os_str().as_bytes()
    }

    fn write(p: &Path, len: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![b'x'; len]).unwrap();
    }

    fn hide(p: &Path) {
        let c = std::ffi::CString::new(bytes(p)).unwrap();
        assert_eq!(unsafe { libc::chflags(c.as_ptr(), libc::UF_HIDDEN) }, 0);
    }

    fn live_of(root: &Path) -> Live {
        let comps: Vec<&[u8]> = bytes(root).split(|&b| b == b'/').filter(|c| !c.is_empty()).collect();
        let above = comps.len() as u32;
        let mut ls = walk::scan(bytes(root), 2);
        for l in &mut ls {
            l.id += above;
            for r in l.ents.iter_mut().filter(|r| r.child != NONE) {
                r.child += above;
            }
        }
        for (i, name) in comps.iter().enumerate() {
            let ent = RawEnt { name_off: 0, name_len: name.len() as u16, kind: KIND_DIR, size: 0, alloc: 0, mtime: 0, child: i as u32 + 1 };
            ls.push(Listing { id: i as u32, names: name.to_vec(), ents: vec![ent] });
        }
        Live::new(Index::build(ls, 0, 0, b""))
    }

    fn weighed(dir: &Path, skip: &[PathBuf]) -> Totals {
        let mut t = Totals::default();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(d) = pending.pop() {
            for e in fs::read_dir(&d).unwrap() {
                let path = e.unwrap().path();
                let m = fs::symlink_metadata(&path).unwrap();
                let name = path.file_name().unwrap().as_bytes();
                t.items += 1;
                t.allocated += m.blocks() * 512;
                t.hardlinks += (m.nlink() > 1 && !m.is_dir()) as u64;
                t.visible += (d == dir && !name.starts_with(b".") && m.st_flags() & libc::UF_HIDDEN == 0) as u64;
                if !m.is_dir() {
                    t.bytes += m.len();
                } else if skip.contains(&path) {
                    t.unindexed.push(path);
                } else {
                    pending.push(path);
                }
            }
        }
        t
    }

    #[test]
    fn totals_count_everything_below_a_folder_exactly() {
        let root = fixture("totals");
        write(&root.join("a.txt"), 100);
        write(&root.join(".dot"), 10);
        write(&root.join("shy.txt"), 20);
        hide(&root.join("shy.txt"));
        let big = root.join("sub/big.bin");
        fs::create_dir_all(big.parent().unwrap()).unwrap();
        fs::File::create(&big).unwrap().set_len((3 << 30) + 12345).unwrap();
        write(&root.join("sub/deep/x"), 1 << 20);
        std::os::unix::fs::symlink("../../a.txt", root.join("sub/deep/link")).unwrap();
        write(&root.join("linked/one"), 4097);
        fs::hard_link(root.join("linked/one"), root.join("linked/two")).unwrap();
        fs::create_dir(root.join("empty")).unwrap();
        let skipped = root.join("skipped");
        write(&skipped.join("inside"), 50);
        walk::SKIP.set(vec![bytes(&skipped).to_vec()]).unwrap();

        let live = live_of(&root);
        let skip = [skipped.clone()];
        for dir in [&root, &root.join("sub"), &root.join("sub/deep"), &root.join("linked"), &root.join("empty")] {
            assert_eq!(live.totals(bytes(dir)), Some(weighed(dir, &skip)), "{dir:?}");
        }
        let t = live.totals(bytes(&root)).unwrap();
        assert_eq!((t.visible, t.hardlinks, t.unindexed), (5, 2, vec![skipped.clone()]));
        assert_eq!(live.entry(bytes(&big)).map(|e| e.1), Some((3 << 30) + 12345));
        assert_eq!(live.totals(bytes(&skipped)), None);
        assert_eq!(live.totals(bytes(&root.join("a.txt"))), None);
        let mut links = live.hardlinks(bytes(&root));
        links.sort();
        assert_eq!(links, [bytes(&root.join("linked/one")).to_vec(), bytes(&root.join("linked/two")).to_vec()]);
        let mut folders = live.folder_totals(bytes(&root)).unwrap();
        folders.sort_by(|a, b| a.0.cmp(&b.0));
        let names: Vec<&[u8]> = folders.iter().map(|(p, _)| &p[bytes(&root).len() + 1..]).collect();
        assert_eq!(names, [&b"empty"[..], b"linked", b"sub"]);
        for (p, t) in &folders {
            assert_eq!(Some(t), live.totals(p).as_ref());
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn totals_follow_changes_and_survive_compaction() {
        let root = fixture("changes");
        for i in 0..150 {
            write(&root.join(format!("many/{i}")), i);
        }
        write(&root.join("keep/a"), 10);
        write(&root.join("gone/deep/b"), 20);
        write(&root.join("grow"), 5);
        fs::File::create(root.join("keep/big.bin")).unwrap().set_len((5 << 30) + 1).unwrap();
        let mut live = live_of(&root);
        assert_eq!(live.totals(bytes(&root)), Some(weighed(&root, &[])));

        for i in (0..150).step_by(7) {
            fs::remove_file(root.join(format!("many/{i}"))).unwrap();
        }
        fs::remove_dir_all(root.join("gone")).unwrap();
        write(&root.join("grow"), 5000);
        write(&root.join("fresh/inner/c"), 300);
        fs::hard_link(root.join("keep/a"), root.join("keep/b")).unwrap();
        hide(&root.join("keep"));
        for dir in [root.clone(), root.join("many"), root.join("keep")] {
            live.apply_dir(bytes(&dir), false);
        }
        let check = |live: &Live| {
            for dir in [&root, &root.join("many"), &root.join("keep"), &root.join("fresh")] {
                assert_eq!(live.totals(bytes(dir)), Some(weighed(dir, &[])), "{dir:?}");
            }
        };
        check(&live);
        check(&Live::new(Index::build(live.to_listings(), 0, 0, b"")));
        fs::remove_dir_all(&root).unwrap();
    }
}
