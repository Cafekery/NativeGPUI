use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use gpui::SharedString;

use super::{SidebarIcon, SidebarItem};

fn key_of(id: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

pub(super) struct Node {
    pub(super) key: u64,
    pub(super) id: SharedString,
    pub(super) title: SharedString,
    pub(super) icon: Option<SidebarIcon>,
    pub(super) badge: Option<i64>,
    pub(super) eject: bool,
    pub(super) drops: bool,
    pub(super) muted: bool,
    pub(super) group: bool,
    pub(super) parent: Option<usize>,
    pub(super) children: Vec<usize>,
}

impl Node {
    pub(super) fn content(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.title.hash(&mut hasher);
        self.icon.hash(&mut hasher);
        self.badge.hash(&mut hasher);
        self.eject.hash(&mut hasher);
        self.drops.hash(&mut hasher);
        self.muted.hash(&mut hasher);
        self.group.hash(&mut hasher);
        self.children.is_empty().hash(&mut hasher);
        hasher.finish()
    }
}

#[derive(Default)]
pub(super) struct Model {
    pub(super) nodes: Vec<Node>,
    pub(super) roots: Vec<usize>,
    by_key: HashMap<u64, usize>,
}

impl Model {
    pub(super) fn build(items: &[SidebarItem]) -> Self {
        let mut model = Model::default();
        model.roots = model.push_all(items, None);
        model
    }

    fn push_all(&mut self, items: &[SidebarItem], parent: Option<usize>) -> Vec<usize> {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let index = self.nodes.len();
            let key = key_of(&item.id);
            self.nodes.push(Node {
                key,
                id: item.id.clone(),
                title: item.title.clone(),
                icon: item.icon.clone(),
                badge: item.badge,
                eject: item.eject,
                drops: item.drops,
                muted: item.muted,
                group: item.group,
                parent,
                children: Vec::new(),
            });
            debug_assert!(
                !self.by_key.contains_key(&key),
                "sidebar item ids must be unique across the whole tree; `{}` is repeated",
                item.id
            );
            self.by_key.insert(key, index);
            let children = self.push_all(&item.children, Some(index));
            self.nodes[index].children = children;
            out.push(index);
        }
        out
    }

    pub(super) fn node(&self, key: u64) -> Option<&Node> {
        self.by_key.get(&key).map(|&i| &self.nodes[i])
    }

    pub(super) fn children_of(&self, parent: Option<u64>) -> &[usize] {
        match parent {
            None => &self.roots,
            Some(key) => self
                .node(key)
                .map(|node| node.children.as_slice())
                .unwrap_or_default(),
        }
    }

    pub(super) fn children_keys(&self, parent: Option<u64>) -> Vec<u64> {
        self.children_of(parent)
            .iter()
            .map(|&i| self.nodes[i].key)
            .collect()
    }

    pub(super) fn parent_key(&self, key: u64) -> Option<u64> {
        self.node(key)?.parent.map(|p| self.nodes[p].key)
    }

    pub(super) fn move_child(&mut self, parent: Option<u64>, from: usize, to: usize) -> bool {
        let list = match parent {
            None => &mut self.roots,
            Some(key) => match self.by_key.get(&key) {
                Some(&index) => &mut self.nodes[index].children,
                None => return false,
            },
        };
        if from >= list.len() || to >= list.len() {
            return false;
        }
        let moved = list.remove(from);
        list.insert(to, moved);
        true
    }

    pub(super) fn preorder(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<usize> = self.roots.iter().rev().copied().collect();
        while let Some(index) = stack.pop() {
            out.push(self.nodes[index].key);
            stack.extend(self.nodes[index].children.iter().rev());
        }
        out
    }

    pub(super) fn id_for(&self, key: u64) -> Option<SharedString> {
        self.node(key).map(|n| n.id.clone())
    }

    pub(super) fn key_for_id(&self, id: &str) -> Option<u64> {
        let key = key_of(id);
        self.by_key.contains_key(&key).then_some(key)
    }
}
