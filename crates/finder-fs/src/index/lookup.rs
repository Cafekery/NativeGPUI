use super::store::{DOTTED, NONE, PACKAGE, ROOT, Tree};

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

/// The indices of `keys` grouped by key, and where each key's group starts.
fn group(keys: &[u32], buckets: usize) -> (Vec<u32>, Vec<u32>) {
    let mut starts = vec![0u32; buckets + 1];
    for &key in keys {
        starts[key as usize + 1] += 1;
    }
    for key in 0..buckets {
        starts[key + 1] += starts[key];
    }
    let mut members = vec![0u32; keys.len()];
    let mut next = starts.clone();
    for (id, &key) in keys.iter().enumerate() {
        let slot = &mut next[key as usize];
        members[*slot as usize] = id as u32;
        *slot += 1;
    }
    (members, starts)
}

#[derive(Default)]
pub struct Lookup {
    of: Vec<u32>,
    at: Vec<u32>,
    heads: Vec<u32>,
    head_at: Vec<u32>,
    sig: Vec<u64>,
    place: Vec<u8>,
}

impl Lookup {
    pub fn build(tree: &Tree) -> Lookup {
        let (of, at) = group(&tree.name_of, tree.names.len());
        let (keys, sig): (Vec<u32>, Vec<u64>) = (0..tree.names.len() as u32)
            .map(|id| {
                let name = tree.names.folded(id);
                (head(&name) as u32, signature(&name))
            })
            .unzip();
        let (heads, head_at) = group(&keys, BUCKETS);
        let masks = tree.names.masks();
        let home = super::resolve(tree, &super::home());
        let mut place = vec![0u8; tree.nodes.len()];
        for (id, node) in tree.nodes.iter().enumerate() {
            let mut bits = match node.parent {
                NONE | ROOT => 0,
                parent => {
                    let packaged = tree.nodes[parent as usize].flags & PACKAGE != 0;
                    place[parent as usize] | u8::from(packaged) * PACKED
                }
            };
            if masks[tree.name_of[id] as usize] & DOTTED != 0 {
                bits |= HIDDEN;
            }
            if Some(node.parent) == home {
                bits |= HOMED;
            }
            place[id] = bits;
        }
        Lookup {
            of,
            at,
            heads,
            head_at,
            sig,
            place,
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
        self.sig.len()
    }

    pub fn holds(&self, name: u32, wanted: u64) -> bool {
        self.sig
            .get(name as usize)
            .is_none_or(|sig| sig & wanted == wanted)
    }

    pub fn stale(&self, tree: &Tree) -> bool {
        let outgrown =
            |len: usize, built: usize| len.checked_sub(built).is_none_or(|new| new > TAIL);
        outgrown(tree.names.len(), self.names()) || outgrown(tree.name_of.len(), self.tail())
    }

    pub fn of(&self, name: u32) -> &[u32] {
        let name = name as usize;
        match self.at.get(name + 1) {
            Some(&end) => &self.of[self.at[name] as usize..end as usize],
            None => &[],
        }
    }

    pub fn tail(&self) -> usize {
        self.place.len()
    }

    /// `None` for a node newer than the table, `Some(None)` inside a package, else (hidden, homed).
    pub fn placed(&self, id: u32) -> Option<Option<(bool, bool)>> {
        let place = *self.place.get(id as usize)?;
        Some((place & PACKED == 0).then_some((place & HIDDEN != 0, place & HOMED != 0)))
    }
}
