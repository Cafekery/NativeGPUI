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
        old_children: Vec<(Option<u64>, Vec<u64>)>,
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
        structural = !old.child_keys(None).eq(new.child_keys(None));
        for key in new.preorder() {
            let (Some(old_node), Some(new_node)) = (old.node(key), new.node(key)) else {
                structural = true;
                break;
            };
            if old_node.content() != new_node.content() {
                content.push(key);
            }
            structural = structural || !old.child_keys(Some(key)).eq(new.child_keys(Some(key)));
        }
    }

    if structural {
        let mut old_children = vec![(None, old.child_keys(None).collect())];
        old_children.extend(
            old.preorder()
                .into_iter()
                .filter(|&key| {
                    new.node(key).is_some() && old.parent_key(key) == new.parent_key(key)
                })
                .map(|key| (Some(key), old.child_keys(Some(key)).collect())),
        );
        return Plan::Structural {
            content,
            old_children,
        };
    }
    if content.is_empty() {
        Plan::Nothing
    } else {
        Plan::Content(content)
    }
}

pub(super) fn apply_diff(state: &SidebarState, plan: Plan) -> SidebarDiff {
    let outline = &state.shared.outline;
    let shared = &state.shared;
    let mut diff = SidebarDiff::default();

    let content = match plan {
        Plan::Nothing => {
            diff.unchanged = true;
            return diff;
        }
        Plan::Reload => {
            outline.reloadData();
            expand_groups(state);
            shared.wrote(1);
            diff.full_reloads = 1;
            return diff;
        }
        Plan::Content(content) => content,
        Plan::Structural {
            content,
            old_children,
        } => {
            apply_structure(state, &old_children, &mut diff);
            content
        }
    };

    for key in content {
        unsafe { outline.reloadItem(Some(&shared.item(key))) };
        diff.reloaded_items += 1;
    }
    shared.wrote(diff.reloaded_items as u64);
    diff
}

fn apply_structure(
    state: &SidebarState,
    old_children: &[(Option<u64>, Vec<u64>)],
    diff: &mut SidebarDiff,
) {
    let outline = &state.shared.outline;
    let shared = &state.shared;
    let saved = SavedState::capture(state);
    outline.beginUpdates();
    for (parent, was) in old_children {
        diff_children(state, *parent, was, diff);
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

fn diff_children(state: &SidebarState, parent: Option<u64>, was: &[u64], diff: &mut SidebarDiff) {
    let outline = &state.shared.outline;
    let wanted: Vec<u64> = state.shared.model.borrow().child_keys(parent).collect();
    if was == wanted {
        return;
    }
    let parent_item = parent.map(|key| state.shared.item(key));
    let parent_item = parent_item.as_deref();

    let doomed = NSMutableIndexSet::new();
    let mut removed = 0;
    for (index, key) in was.iter().enumerate() {
        if !wanted.contains(key) {
            doomed.addIndex(index);
            removed += 1;
        }
    }
    if removed > 0 {
        unsafe {
            outline.removeItemsAtIndexes_inParent_withAnimation(
                &doomed,
                parent_item,
                NSTableViewAnimationOptions::EffectNone,
            )
        };
    }

    let mut current: Vec<u64> = was
        .iter()
        .copied()
        .filter(|key| wanted.contains(key))
        .collect();
    let survivors: Vec<u64> = wanted
        .iter()
        .copied()
        .filter(|key| was.contains(key))
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
                parent_item,
                target as NSInteger,
                parent_item,
            )
        };
        let key = current.remove(from);
        current.insert(target, key);
        moved += 1;
    }

    let fresh = NSMutableIndexSet::new();
    let mut inserted = 0;
    for (index, key) in wanted.iter().enumerate() {
        if !was.contains(key) {
            fresh.addIndex(index);
            inserted += 1;
        }
    }
    if inserted > 0 {
        unsafe {
            outline.insertItemsAtIndexes_inParent_withAnimation(
                &fresh,
                parent_item,
                NSTableViewAnimationOptions::EffectNone,
            )
        };
    }

    diff.inserted += inserted;
    diff.removed += removed;
    diff.moved += moved;
}

pub(super) fn reveal(state: &SidebarState, key: u64) {
    let outline = &state.shared.outline;
    let ancestors: Vec<u64> = {
        let model = state.shared.model.borrow();
        std::iter::successors(model.parent_key(key), |&parent| model.parent_key(parent)).collect()
    };
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
    let outline = &state.shared.outline;
    let keys: Vec<u64> = state
        .shared
        .model
        .borrow()
        .nodes
        .iter()
        .filter(|node| node.group && !node.children.is_empty())
        .map(|node| node.key)
        .collect();
    for key in keys {
        let item = state.shared.item(key);
        if !unsafe { outline.isItemExpanded(Some(&item)) } {
            unsafe { outline.expandItem(Some(&item)) };
        }
    }
}

fn expected_rows(state: &SidebarState) -> NSInteger {
    let outline = &state.shared.outline;
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
        let outline = &state.shared.outline;
        let row = outline.selectedRow();
        let selected = (row >= 0)
            .then(|| outline.itemAtRow(row))
            .flatten()
            .map(|item| Shared::key_of_item(&item));
        let expanded = state
            .shared
            .model
            .borrow()
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
        let outline = &state.shared.outline;
        let shared = &state.shared;
        let exists = |key: u64| shared.model.borrow().node(key).is_some();
        for &key in &self.expanded {
            if exists(key) {
                unsafe { outline.expandItem(Some(&shared.item(key))) };
            }
        }
        if let Some(key) = self.selected.filter(|&key| exists(key)) {
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
