use super::store::{DOTTED, NEEDS_FOLD, NONE, PACKAGE, ROOT, Tree, folded};

/// What a hit needs to know about where a node sits, which is otherwise a walk to the root:
/// whether Spotlight would hide it, and whether it is under the user's home.
const HIDDEN: u8 = 1;
const HOMED: u8 = 2;

/// A name's adjacent byte pairs, hashed into one bit each. A query whose pairs are not all here
/// cannot be a substring of the name, and the check costs one load against a name's bytes.
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
    /// Node ids grouped by the name they wear, `at[name] as usize..at[name + 1] as usize`.
    of: Vec<u32>,
    at: Vec<u32>,
    /// Name ids grouped by their first two folded bytes.
    heads: Vec<u32>,
    head_at: Vec<u32>,
    names: usize,
    /// Every name's [`signature`], by name id.
    sig: Vec<u64>,
    /// `HIDDEN` and `HOMED` per node, taken from its parents once instead of per query.
    place: Vec<u8>,
    built: usize,
}

impl Lookup {
    pub fn build(tree: &Tree) -> Lookup {
        let names = tree.names.len();
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
            match tree.names.masks()[id as usize] & NEEDS_FOLD == 0 {
                true => std::borrow::Cow::Borrowed(bytes),
                false => std::borrow::Cow::Owned(folded(bytes).into_owned()),
            }
        };
        let mut head_at = vec![0u32; BUCKETS + 1];
        for id in 0..names as u32 {
            head_at[head(&folded_name(id)) + 1] += 1;
        }
        for key in 0..BUCKETS {
            head_at[key + 1] += head_at[key];
        }
        let mut sig = vec![0u64; names];
        for (id, slot) in sig.iter_mut().enumerate() {
            let id = id as u32;
            let bytes = tree.names.bytes(id);
            *slot = match tree.names.masks()[id as usize] & NEEDS_FOLD == 0 {
                true => signature(bytes),
                false => signature(&folded(bytes)),
            };
        }
        let mut heads = vec![0u32; names];
        let mut next_head = head_at.clone();
        for id in 0..names as u32 {
            let slot = &mut next_head[head(&folded_name(id))];
            heads[*slot as usize] = id;
            *slot += 1;
        }
        let mut place = vec![0u8; tree.name_of.len()];
        let home = super::resolve(tree, &super::crawl::home());
        let masks = tree.names.masks();
        for id in 0..tree.name_of.len() {
            let node = &tree.nodes[id];
            let dotted = masks[tree.name_of[id] as usize] & DOTTED != 0;
            let (up, mine) = match node.parent {
                NONE | ROOT => (0, 0),
                parent if (parent as usize) < id => {
                    let up = place[parent as usize];
                    let packaged = tree.nodes[parent as usize].flags & PACKAGE != 0;
                    (up, u8::from(packaged) * HIDDEN)
                }
                _ => (0, 0),
            };
            place[id] = up | mine | u8::from(dotted) * HIDDEN;
            if Some(node.parent) == home || (up & HOMED != 0 && node.parent != NONE) {
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

    fn bucket<'a>(ids: &'a [u32], at: &[u32], query: &[u8]) -> &'a [u32] {
        let key = head(query);
        let (from, to) = match query.len() {
            1 => (key, key + 256),
            _ => (key, key + 1),
        };
        match (at.get(from), at.get(to)) {
            (Some(&from), Some(&to)) => &ids[from as usize..to as usize],
            _ => &[],
        }
    }

    pub fn heads(&self, query: &[u8]) -> &[u32] {
        Self::bucket(&self.heads, &self.head_at, query)
    }

    pub fn names(&self) -> usize {
        self.names
    }

    /// Whether the name could hold every pair the query has. Names the table has not taken in
    /// yet answer yes, and are read.
    pub fn holds(&self, name: u32, wanted: u64) -> bool {
        match self.sig.get(name as usize) {
            Some(sig) => sig & wanted == wanted,
            None => true,
        }
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

    /// `Some(under the home)` for a node a search may show, `None` for one Spotlight hides.
    /// Nodes the table has not taken in yet answer `None` here and are walked instead.
    pub fn placed(&self, id: u32) -> Option<Option<bool>> {
        let place = *self.place.get(id as usize)?;
        match place & HIDDEN != 0 {
            true => Some(None),
            false => Some(Some(place & HOMED != 0)),
        }
    }
}
