use std::borrow::Cow;
use std::thread;

use super::Hit;
use super::lookup::Lookup;
use super::store::{DOTTED, NEEDS_FOLD, NONE, PACKAGE, ROOT, Tree, folded, mask_of};

const SUBSTRING: u8 = 1;
const WORD: u8 = 2;
const PREFIX: u8 = 3;
const EXACT: u8 = 4;

const LOW: u64 = 0x0101_0101_0101_0101;
const HIGH: u64 = 0x8080_8080_8080_8080;

const CHUNK: usize = 1 << 15;

/// A query byte matches case-insensitively as `byte | fold == query`.
fn fold_of(byte: u8) -> u8 {
    match byte.is_ascii_lowercase() {
        true => 0x20,
        false => 0,
    }
}

fn word_start(name: &[u8], at: usize) -> bool {
    let before = name[at - 1];
    matches!(
        before,
        b' ' | b'-' | b'_' | b'.' | b'+' | b'(' | b'[' | b',' | b'@'
    ) || (before.is_ascii_lowercase() && name[at].is_ascii_uppercase())
}

fn matches_at(name: &[u8], at: usize, query: &[u8], folds: &[u8]) -> bool {
    name[at..at + query.len()]
        .iter()
        .zip(query.iter().zip(folds))
        .all(|(&byte, (&want, &fold))| byte | fold == want)
}

fn class_at(name: &[u8], at: usize, query: &[u8], folds: &[u8]) -> u8 {
    if !matches_at(name, at, query, folds) {
        return 0;
    }
    if at == 0 {
        return PREFIX;
    }
    match word_start(name, at) {
        true => WORD,
        false => SUBSTRING,
    }
}

fn class_of(name: &[u8], query: &[u8], folds: &[u8]) -> u8 {
    let Some(last) = name.len().checked_sub(query.len()) else {
        return 0;
    };
    let broadcast = LOW.wrapping_mul(query[0] as u64);
    let spread = LOW.wrapping_mul(folds[0] as u64);
    let mut best = 0;
    let mut at = 0;
    while at + 8 <= name.len() {
        let word = u64::from_le_bytes(name[at..at + 8].try_into().unwrap()) | spread;
        let diff = word ^ broadcast;
        let mut found = diff.wrapping_sub(LOW) & !diff & HIGH;
        while found != 0 {
            let position = at + (found.trailing_zeros() >> 3) as usize;
            found &= found - 1;
            if position > last {
                return best;
            }
            match class_at(name, position, query, folds) {
                PREFIX => return PREFIX,
                WORD => return WORD,
                class => best = best.max(class),
            }
        }
        at += 8;
    }
    for position in at..=last {
        match class_at(name, position, query, folds) {
            PREFIX => return PREFIX,
            WORD => return WORD,
            class => best = best.max(class),
        }
    }
    best
}

fn head_of(name: &[u8], query: &[u8], folds: &[u8]) -> u8 {
    match name.len() >= query.len() && matches_at(name, 0, query, folds) {
        true => PREFIX,
        false => 0,
    }
}

struct Word {
    bytes: Vec<u8>,
    folds: Vec<u8>,
}

impl Word {
    fn of(bytes: &[u8]) -> Word {
        Word {
            bytes: bytes.to_vec(),
            folds: bytes.iter().copied().map(fold_of).collect(),
        }
    }

    fn class_of(&self, name: &[u8]) -> u8 {
        class_of(name, &self.bytes, &self.folds)
    }

    fn head_of(&self, name: &[u8]) -> u8 {
        head_of(name, &self.bytes, &self.folds)
    }
}

fn classed(words: &[Word], name: &[u8], first: impl Fn(&Word, &[u8]) -> u8) -> u8 {
    let class = first(&words[0], name);
    if class == 0 {
        return 0;
    }
    if !words[1..].iter().all(|word| word.class_of(name) != 0) {
        return 0;
    }
    match class == PREFIX && words.len() == 1 && name.len() == words[0].bytes.len() {
        true => EXACT,
        false => class,
    }
}

type Found = (u32, u32, u32);

struct Query<'a> {
    tree: &'a Tree,
    lookup: &'a Lookup,
    bytes: &'a [u8],
    wanted: u64,
    home: Option<u32>,
    under: Option<u32>,
    limit: usize,
    cap: usize,
}

impl Query<'_> {
    fn name(&self, id: u32, mask: u64) -> Cow<'_, [u8]> {
        let bytes = self.tree.names.bytes(id);
        match mask & NEEDS_FOLD == 0 {
            true => Cow::Borrowed(bytes),
            false => folded(bytes),
        }
    }

    fn order(&self, a: &Found, b: &Found) -> std::cmp::Ordering {
        b.0.cmp(&a.0)
            .then_with(|| self.tree.names.bytes(a.1).cmp(self.tree.names.bytes(b.1)))
            .then_with(|| a.2.cmp(&b.2))
    }

    /// `None` where Finder's own search would not show it at all: a dot-file, or anything inside
    /// a dot-folder or a package, none of which Spotlight indexes.
    fn placed(&self, id: u32) -> Option<bool> {
        let masks = self.tree.names.masks();
        let dotted = |id: u32| masks[self.tree.name_of[id as usize] as usize] & DOTTED != 0;
        if dotted(id) {
            return None;
        }
        let mut home = false;
        let mut at = self.tree.nodes[id as usize].parent;
        while at != NONE && at != ROOT {
            let up = &self.tree.nodes[at as usize];
            if up.flags & PACKAGE != 0 || dotted(at) {
                return None;
            }
            home |= Some(at) == self.home;
            at = up.parent;
        }
        Some(home)
    }

    fn keep(&self, best: &mut Vec<Found>, cutoff: &mut u32, id: u32, name: u32, class: u8) {
        let node = &self.tree.nodes[id as usize];
        if node.deleted() {
            return;
        }
        let score = |home: bool| {
            (class as u32) << 28
                | (home as u32) << 27
                | (255 - node.depth as u32) << 19
                | (u16::MAX as u32
                    - self.tree.names.bytes(name).len().min(u16::MAX as usize) as u32)
        };
        if score(true) < *cutoff {
            return;
        }
        if self.under.is_some_and(|root| !self.tree.is_under(id, root)) {
            return;
        }
        let Some(home) = self.placed(id) else {
            return;
        };
        let score = score(home);
        if score < *cutoff {
            return;
        }
        best.push((score, name, id));
        if best.len() > self.cap {
            best.select_nth_unstable_by(self.limit - 1, |a, b| self.order(a, b));
            best.truncate(self.limit);
            *cutoff = best.iter().map(|found| found.0).min().unwrap_or(0);
        }
    }

    fn ids(&self, ids: &[u32], class: &(impl Fn(&[u8]) -> u8 + Sync)) -> Vec<Found> {
        let masks = self.tree.names.masks();
        let mut best = Vec::with_capacity(self.cap + 1);
        let mut cutoff = 0;
        for &id in ids {
            let mask = masks[id as usize];
            if mask & self.wanted != self.wanted {
                continue;
            }
            let class = class(&self.name(id, mask));
            if class == 0 {
                continue;
            }
            for &node in self.lookup.of(id) {
                self.keep(&mut best, &mut cutoff, node, id, class);
            }
        }
        best
    }

    fn tail(&self, class: &impl Fn(&[u8]) -> u8) -> Vec<Found> {
        let masks = self.tree.names.masks();
        let mut best = Vec::with_capacity(self.cap + 1);
        let mut cutoff = 0;
        let mut last = (u32::MAX, 0u8);
        for id in self.lookup.tail()..self.tree.name_of.len() {
            let name = self.tree.name_of[id];
            if last.0 != name {
                let mask = masks[name as usize];
                let class = match mask & self.wanted == self.wanted {
                    true => class(&self.name(name, mask)),
                    false => 0,
                };
                last = (name, class);
            }
            if last.1 != 0 {
                self.keep(&mut best, &mut cutoff, id as u32, name, last.1);
            }
        }
        best
    }

    fn spread(&self, ids: &[u32], class: impl Fn(&[u8]) -> u8 + Sync) -> Vec<Found> {
        let cores = thread::available_parallelism().map_or(1, |cores| cores.get());
        let threads = cores.min(ids.len().div_ceil(CHUNK)).max(1);
        let per = ids.len().div_ceil(threads).max(1);
        let class = &class;
        let mut best: Vec<Found> = thread::scope(|scope| {
            let handles: Vec<_> = ids
                .chunks(per)
                .map(|ids| scope.spawn(move || self.ids(ids, class)))
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .flatten()
                .collect()
        });
        let fresh: Vec<u32> =
            (self.lookup.every().len() as u32..self.tree.names.len() as u32).collect();
        best.extend(self.ids(&fresh, class));
        best.extend(self.tail(class));
        best.sort_by(|a, b| self.order(a, b));
        best.truncate(self.limit);
        best
    }

    fn heads(&self, class: impl Fn(&[u8]) -> u8 + Sync) -> Vec<Found> {
        self.spread(self.lookup.heads(self.bytes), class)
    }

    fn all(&self, class: impl Fn(&[u8]) -> u8 + Sync) -> Vec<Found> {
        self.spread(self.lookup.every(), class)
    }
}

pub fn search(
    tree: &Tree,
    lookup: &Lookup,
    query: &str,
    under: Option<u32>,
    limit: usize,
) -> Vec<Hit> {
    let folded = crate::sort::fold(query);
    let words: Vec<Word> = folded
        .split_ascii_whitespace()
        .map(|word| Word::of(word.as_bytes()))
        .collect();
    let Some(first) = words.first() else {
        return Vec::new();
    };
    if limit == 0 {
        return Vec::new();
    }
    let query = Query {
        tree,
        lookup,
        bytes: &first.bytes,
        wanted: words
            .iter()
            .fold(0, |mask, word| mask | mask_of(&word.bytes)),
        home: super::resolve(tree, &super::crawl::home()),
        under,
        limit,
        cap: limit.saturating_mul(4).max(64),
    };
    let mut best = query.heads(|name| classed(&words, name, Word::head_of));
    if best.len() < limit {
        best = query.all(|name| classed(&words, name, Word::class_of));
    }
    best.into_iter()
        .map(|(score, _, id)| Hit {
            path: tree.path(id),
            entry: tree.entry(id),
            score,
        })
        .collect()
}
