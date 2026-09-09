use super::store::{NEEDS_FOLD, Tree, folded};

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
    /// Name ids grouped by their first two folded bytes, `heads[head_at[key]..head_at[key + 1]]`.
    heads: Vec<u32>,
    head_at: Vec<u32>,
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
        let mut head_at = vec![0u32; BUCKETS + 1];
        let key = |id: u32| {
            let bytes = tree.names.bytes(id);
            match tree.names.masks()[id as usize] & NEEDS_FOLD == 0 {
                true => head(bytes),
                false => head(&folded(bytes)),
            }
        };
        for id in 0..names as u32 {
            head_at[key(id) + 1] += 1;
        }
        for key in 0..BUCKETS {
            head_at[key + 1] += head_at[key];
        }
        let mut heads = vec![0u32; names];
        let mut next = head_at.clone();
        for id in 0..names as u32 {
            let slot = &mut next[key(id)];
            heads[*slot as usize] = id;
            *slot += 1;
        }
        Lookup {
            of,
            at,
            heads,
            head_at,
            built: tree.name_of.len(),
        }
    }

    pub fn heads(&self, query: &[u8]) -> &[u32] {
        let key = head(query);
        let (from, to) = match query.len() {
            1 => (key, key + 256),
            _ => (key, key + 1),
        };
        match (self.head_at.get(from), self.head_at.get(to)) {
            (Some(&from), Some(&to)) => &self.heads[from as usize..to as usize],
            _ => &[],
        }
    }

    pub fn every(&self) -> &[u32] {
        &self.heads
    }

    pub fn stale(&self, tree: &Tree) -> bool {
        let (names, nodes) = (self.heads.len(), self.built);
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
}
