use std::borrow::Cow;

use super::store::{DOTTED, NEEDS_FOLD, NONE, PACKAGE, ROOT, Tree, folded};

const HIDDEN: u8 = 1;
const HOMED: u8 = 2;
const PACKED: u8 = 4;

pub fn signature(bytes: &[u8]) -> u64 {
    let mut sig = 0;
    for pair in bytes.windows(2) {
        let key = (pair[0] | 0x20) as u64 * 131 + (pair[1] | 0x20) as u64;
        sig |= 1 << (key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 58);
    }
    sig
}

const TAIL: usize = 1 << 17;

const BUCKETS: usize = 256 * 256;

fn head(bytes: &[u8]) -> usize {
    let key = |at: usize| match bytes.get(at) {
        Some(&byte) if byte.is_ascii_alphabetic() => (byte | 0x20) as usize,
        Some(&byte) => byte as usize,
        None => 0,
    };
    key(0) * 256 + key(1)
}

#[derive(Default)]
pub struct Lookup {
    of: Vec<u32>,
    at: Vec<u32>,
    heads: Vec<u32>,
    head_at: Vec<u32>,
    names: usize,
    sig: Vec<u64>,
    place: Vec<u8>,
    built: usize,
}

impl Lookup {
    pub fn build(tree: &Tree) -> Lookup {
        let names = tree.names.len();
        let masks = tree.names.masks();
        let mut at = vec![0u32; names + 1];
        for &name in &tree.name_of {
            at[name as usize + 1] += 1;
        }
        for id in 0..names {
            at[id + 1] += at[id];
        }
        let mut of = vec![0u32; tree.name_of.len()];
        let mut next = at.clone();
        for (id, &name) in tree.name_of.iter().enumerate() {
            let slot = &mut next[name as usize];
            of[*slot as usize] = id as u32;
            *slot += 1;
        }
        let folded_name = |id: u32| {
            let bytes = tree.names.bytes(id);
            match masks[id as usize] & NEEDS_FOLD == 0 {
                true => Cow::Borrowed(bytes),
                false => folded(bytes),
            }
        };
        let mut head_at = vec![0u32; BUCKETS + 1];
        for id in 0..names as u32 {
            head_at[head(&folded_name(id)) + 1] += 1;
        }
        for key in 0..BUCKETS {
            head_at[key + 1] += head_at[key];
        }
        let sig: Vec<u64> = (0..names as u32)
            .map(|id| signature(&folded_name(id)))
            .collect();
        let mut heads = vec![0u32; names];
        let mut next_head = head_at.clone();
        for id in 0..names as u32 {
            let slot = &mut next_head[head(&folded_name(id))];
            heads[*slot as usize] = id;
            *slot += 1;
        }
        let mut place = vec![0u8; tree.name_of.len()];
        let home = super::resolve(tree, &super::crawl::home());
        for id in 0..tree.name_of.len() {
            let node = &tree.nodes[id];
            let dotted = masks[tree.name_of[id] as usize] & DOTTED != 0;
            let (up, mine) = match node.parent {
                NONE | ROOT => (0, 0),
                parent if (parent as usize) < id => {
                    let up = place[parent as usize];
                    let packaged = tree.nodes[parent as usize].flags & PACKAGE != 0;
                    (up, u8::from(packaged) * PACKED)
                }
                _ => (0, 0),
            };
            place[id] = up | mine | (u8::from(dotted) * HIDDEN);
            if Some(node.parent) == home || up & HOMED != 0 {
                place[id] |= HOMED;
            }
        }
        Lookup {
            of,
            at,
            heads,
            head_at,
            names,
            sig,
            place,
            built: tree.name_of.len(),
        }
    }

    pub fn heads(&self, query: &[u8]) -> &[u32] {
        let from = head(query);
        let to = from + if query.len() == 1 { 256 } else { 1 };
        match (self.head_at.get(from), self.head_at.get(to)) {
            (Some(&from), Some(&to)) => &self.heads[from as usize..to as usize],
            _ => &[],
        }
    }

    pub fn names(&self) -> usize {
        self.names
    }

    pub fn holds(&self, name: u32, wanted: u64) -> bool {
        self.sig
            .get(name as usize)
            .is_none_or(|sig| sig & wanted == wanted)
    }

    pub fn stale(&self, tree: &Tree) -> bool {
        let (names, nodes) = (self.names, self.built);
        tree.names.len() < names
            || tree.name_of.len() < nodes
            || tree.names.len() - names > TAIL
            || tree.name_of.len() - nodes > TAIL
    }

    pub fn of(&self, name: u32) -> &[u32] {
        let name = name as usize;
        match self.at.get(name + 1) {
            Some(&end) => &self.of[self.at[name] as usize..end as usize],
            None => &[],
        }
    }

    pub fn tail(&self) -> usize {
        self.built
    }

    /// `None` for a node newer than the table, `Some(None)` inside a package, else (hidden, homed).
    pub fn placed(&self, id: u32) -> Option<Option<(bool, bool)>> {
        let place = *self.place.get(id as usize)?;
        Some((place & PACKED == 0).then_some((place & HIDDEN != 0, place & HOMED != 0)))
    }
}
