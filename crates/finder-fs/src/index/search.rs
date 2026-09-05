//! Name search: the interned names are scanned once for the query, then the nodes are ranked
//! against a rising cutoff so only the best `limit` are ever kept.

use super::Hit;
use super::store::{Tree, letters};

const SUBSTRING: u8 = 1;
const WORD: u8 = 2;
const PREFIX: u8 = 3;

const LOW: u64 = 0x0101_0101_0101_0101;
const HIGH: u64 = 0x8080_8080_8080_8080;

/// A query byte matches case-insensitively as `byte | fold == query`: only the two letter
/// ranges can land on a lowercase letter, so the fold needs no branch and no table.
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

fn class_at(name: &[u8], at: usize, query: &[u8], folds: &[u8]) -> u8 {
    let matched = name[at..at + query.len()]
        .iter()
        .zip(query.iter().zip(folds))
        .all(|(&byte, (&want, &fold))| byte | fold == want);
    if !matched {
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

/// Every position of `query` in `name`, eight bytes at a time, best class first.
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

pub fn search(tree: &Tree, query: &str, under: Option<u32>, limit: usize) -> Vec<Hit> {
    let query = query.to_lowercase();
    let query = query.as_bytes();
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }
    let wanted = letters(query);
    let folds: Vec<u8> = query.iter().copied().map(fold_of).collect();
    let mut class = vec![0u8; tree.names.len()];
    let mut any = false;
    for (id, mask) in tree.names.masks().iter().enumerate() {
        if mask & wanted != wanted {
            continue;
        }
        class[id] = class_of(tree.names.bytes(id as u32), query, &folds);
        any |= class[id] != 0;
    }
    if !any {
        return Vec::new();
    }

    let order = |a: &(u32, u32, u32), b: &(u32, u32, u32)| {
        b.0.cmp(&a.0)
            .then_with(|| tree.names.bytes(a.1).cmp(tree.names.bytes(b.1)))
    };
    let cap = limit.saturating_mul(4).max(64);
    let mut best: Vec<(u32, u32, u32)> = Vec::with_capacity(cap + 1);
    let mut cutoff = 0;
    for id in 0..tree.nodes.len() as u32 {
        let name = tree.name_of[id as usize];
        let class = class[name as usize];
        if class == 0 {
            continue;
        }
        let node = &tree.nodes[id as usize];
        if node.deleted() {
            continue;
        }
        let score = (class as u32) << 8 | (255 - node.depth as u32);
        if score < cutoff {
            continue;
        }
        if under.is_some_and(|root| !tree.is_under(id, root)) {
            continue;
        }
        best.push((score, name, id));
        if best.len() > cap {
            best.select_nth_unstable_by(limit - 1, order);
            best.truncate(limit);
            cutoff = best.iter().map(|found| found.0).min().unwrap_or(0);
        }
    }
    best.sort_by(order);
    best.truncate(limit);
    best.into_iter()
        .map(|(score, _, id)| Hit {
            path: tree.path(id),
            entry: tree.entry(id),
            score,
        })
        .collect()
}
