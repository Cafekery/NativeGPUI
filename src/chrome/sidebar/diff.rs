use objc2_app_kit::NSTableViewAnimationOptions;
use objc2_foundation::{NSIndexSet, NSInteger, NSMutableIndexSet, NSPoint};

use super::model::Model;
use super::{Shared, SidebarDiff, SidebarState};

pub(super) enum Plan {
    Nothing,
    Reload,
    Content(Vec<u64>),
    Structural {
        content: Vec<u64>,
        /// The *old* model's child lists; the data source already answers from the new one.
        parents: Vec<(Option<u64>, Vec<u64>)>,
    },
}

pub(super) fn plan_diff(old: &Model, new: &Model) -> Plan {
    if old.nodes.is_empty() {
        return if new.nodes.is_empty() {
            Plan::Nothing
        } else {
            Plan::Reload
        };
    }

    let mut content = Vec::new();
    let mut structural = old.roots.len() != new.roots.len() || old.nodes.len() != new.nodes.len();

    if !structural {
        for key in new.preorder() {
            let (Some(old_node), Some(new_node)) = (old.node(key), new.node(key)) else {
                structural = true;
                break;
            };
            if old_node.content() != new_node.content() {
                content.push(key);
            }
        }
    }
    if !structural {
        structural = std::iter::once(None)
            .chain(new.preorder().into_iter().map(Some))
            .any(|parent| old.children_keys(parent) != new.children_keys(parent));
    }

    if structural {
        let mut parents = vec![(None, old.children_keys(None))];
        parents.extend(
            old.preorder()
                .into_iter()
                .filter(|&key| {
                    new.node(key).is_some() && old.parent_key(key) == new.parent_key(key)
                })
                .map(|key| (Some(key), old.children_keys(Some(key)))),
        );
        return Plan::Structural { content, parents };
    }
    if content.is_empty() {
        Plan::Nothing
    } else {
        Plan::Content(content)
    }
}

pub(super) fn apply_diff(state: &SidebarState, plan: Plan) -> SidebarDiff {
    let outline = &state.views.outline;
    let shared = &state.shared;
    let mut diff = SidebarDiff::default();

    let (content, parents) = match plan {
        Plan::Nothing => {
            return SidebarDiff {
                unchanged: true,
                ..Default::default()
            };
        }
        Plan::Reload => {
            outline.reloadData();
            expand_groups(state);
            shared.wrote(1);
            return SidebarDiff {
                full_reloads: 1,
                ..Default::default()
            };
        }
        Plan::Content(content) => (content, Vec::new()),
        Plan::Structural { content, parents } => (content, parents),
    };

    if !parents.is_empty() {
        let saved = SavedState::capture(state);
        outline.beginUpdates();
        for (parent, was) in &parents {
            let (inserted, removed, moved) = diff_children(state, *parent, was);
            diff.inserted += inserted;
            diff.removed += removed;
            diff.moved += moved;
        }
        outline.endUpdates();
        shared.wrote((diff.inserted + diff.removed + diff.moved) as u64);
        expand_groups(state);

        if outline.numberOfRows() != expected_rows(state) {
            outline.reloadData();
            expand_groups(state);
            saved.restore(state);
            diff.full_reloads += 1;
            shared.wrote(1);
        }
    }

    for key in content {
        let item = shared.item(key);
        unsafe { outline.reloadItem(Some(&item)) };
        diff.reloaded_items += 1;
    }
    shared.wrote(diff.reloaded_items as u64);

    diff
}

fn diff_children(state: &SidebarState, parent: Option<u64>, was: &[u64]) -> (usize, usize, usize) {
    let outline = &state.views.outline;
    let shared = &state.shared;
    let parent_item = parent.map(|key| shared.item(key));
    let parent_ref = parent_item.as_deref();

    let mut current: Vec<u64> = was.to_vec();
    let wanted = shared.model.borrow().children_keys(parent);
    if current == wanted {
        return (0, 0, 0);
    }

    let doomed: Vec<usize> = current
        .iter()
        .enumerate()
        .filter(|(_, key)| !wanted.contains(key))
        .map(|(index, _)| index)
        .collect();
    let removed = doomed.len();
    if removed > 0 {
        let set = NSMutableIndexSet::new();
        for &index in &doomed {
            set.addIndex(index);
        }
        unsafe {
            outline.removeItemsAtIndexes_inParent_withAnimation(
                &set,
                parent_ref,
                NSTableViewAnimationOptions::EffectNone,
            )
        };
        for &index in doomed.iter().rev() {
            current.remove(index);
        }
    }

    let survivors: Vec<u64> = wanted
        .iter()
        .copied()
        .filter(|k| current.contains(k))
        .collect();
    let mut moved = 0;
    for (target, key) in survivors.iter().enumerate() {
        if current.get(target) == Some(key) {
            continue;
        }
        let Some(from) = current.iter().position(|k| k == key) else {
            continue;
        };
        unsafe {
            outline.moveItemAtIndex_inParent_toIndex_inParent(
                from as NSInteger,
                parent_ref,
                target as NSInteger,
                parent_ref,
            )
        };
        let key = current.remove(from);
        current.insert(target, key);
        moved += 1;
    }

    let mut inserted = 0;
    let fresh = NSMutableIndexSet::new();
    for (index, key) in wanted.iter().enumerate() {
        if !current.contains(key) {
            fresh.addIndex(index);
            current.insert(index.min(current.len()), *key);
            inserted += 1;
        }
    }
    if inserted > 0 {
        unsafe {
            outline.insertItemsAtIndexes_inParent_withAnimation(
                &fresh,
                parent_ref,
                NSTableViewAnimationOptions::EffectNone,
            )
        };
    }

    (inserted, removed, moved)
}

pub(super) fn reveal(state: &SidebarState, key: u64) {
    let outline = &state.views.outline;
    let mut ancestors = Vec::new();
    {
        let model = state.shared.model.borrow();
        let mut walk = model.parent_key(key);
        while let Some(parent) = walk {
            ancestors.push(parent);
            walk = model.parent_key(parent);
        }
    }
    for parent in ancestors.into_iter().rev() {
        let item = state.shared.item(parent);
        if !unsafe { outline.isItemExpanded(Some(&item)) } {
            state.shared.wrote(1);
            unsafe { outline.expandItem(Some(&item)) };
        }
    }
}

pub(super) fn expand_groups(state: &SidebarState) {
    if !state.shared.expand_groups.get() {
        return;
    }
    let outline = &state.views.outline;
    let keys: Vec<u64> = {
        let model = state.shared.model.borrow();
        model
            .nodes
            .iter()
            .filter(|node| node.group && !node.children.is_empty())
            .map(|node| node.key)
            .collect()
    };
    for key in keys {
        let item = state.shared.item(key);
        if !unsafe { outline.isItemExpanded(Some(&item)) } {
            unsafe { outline.expandItem(Some(&item)) };
        }
    }
}

fn expected_rows(state: &SidebarState) -> NSInteger {
    let outline = &state.views.outline;
    let model = state.shared.model.borrow();
    let mut total = 0;
    let mut stack: Vec<usize> = model.roots.iter().rev().copied().collect();
    while let Some(index) = stack.pop() {
        total += 1;
        let node = &model.nodes[index];
        if node.children.is_empty() {
            continue;
        }
        let item = state.shared.item(node.key);
        if unsafe { outline.isItemExpanded(Some(&item)) } {
            stack.extend(node.children.iter().rev());
        }
    }
    total
}

pub(super) struct SavedState {
    selected: Option<u64>,
    expanded: Vec<u64>,
    scroll: NSPoint,
}

impl SavedState {
    pub(super) fn capture(state: &SidebarState) -> Self {
        let outline = &state.views.outline;
        let model = state.shared.model.borrow();
        let row = outline.selectedRow();
        let selected = (row >= 0)
            .then(|| outline.itemAtRow(row))
            .flatten()
            .map(|item| Shared::key_of_item(&item));
        let expanded = model
            .preorder()
            .into_iter()
            .filter(|&key| {
                let item = state.shared.item(key);
                unsafe { outline.isItemExpanded(Some(&item)) }
            })
            .collect();
        Self {
            selected,
            expanded,
            scroll: state.views.scroll.contentView().bounds().origin,
        }
    }

    pub(super) fn restore(&self, state: &SidebarState) {
        let outline = &state.views.outline;
        let shared = &state.shared;
        for &key in &self.expanded {
            if shared.model.borrow().node(key).is_some() {
                unsafe { outline.expandItem(Some(&shared.item(key))) };
            }
        }
        let selected = self
            .selected
            .filter(|&key| shared.model.borrow().node(key).is_some());
        if let Some(key) = selected {
            let row = unsafe { outline.rowForItem(Some(&shared.item(key))) };
            if row >= 0 {
                shared.quietly(|| {
                    outline.selectRowIndexes_byExtendingSelection(
                        &NSIndexSet::indexSetWithIndex(row as usize),
                        false,
                    )
                });
            }
        }
        let clip = state.views.scroll.contentView();
        clip.scrollToPoint(self.scroll);
        state.views.scroll.reflectScrolledClipView(&clip);
    }
}
