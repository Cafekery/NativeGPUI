use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, sel};
use objc2_app_kit::{NSMenu, NSView};
use objc2_foundation::{MainThreadMarker, NSInteger};

use super::Shared;
use super::cell::tint_rows;

pub(super) fn children(shared: &Shared, item: Option<&AnyObject>) -> NSInteger {
    shared
        .model
        .borrow()
        .children_of(item.map(Shared::key_of_item))
        .len() as NSInteger
}

pub(super) fn child(
    shared: &Shared,
    index: NSInteger,
    item: Option<&AnyObject>,
) -> Retained<AnyObject> {
    let key = shared
        .model
        .borrow()
        .child_keys(item.map(Shared::key_of_item))
        .nth(index as usize)
        .unwrap_or(0);
    shared.item(key)
}

pub(super) fn expandable(shared: &Shared, item: &AnyObject) -> bool {
    shared.node_is(Shared::key_of_item(item), |node| !node.children.is_empty())
}

pub(super) fn is_group(shared: &Shared, item: &AnyObject) -> bool {
    shared.node_is(Shared::key_of_item(item), |node| node.group)
}

pub(super) fn selectable(shared: &Shared, item: &AnyObject) -> bool {
    shared.node_is(Shared::key_of_item(item), |node| !node.group)
}

pub(super) fn selection_changed(shared: &Shared) {
    tint_rows(&shared.outline);
    if shared.suppress.get() > 0 {
        return;
    }
    let Some(handler) = shared.on_select.borrow().clone() else {
        return;
    };
    let id = shared.selected_id();
    shared.defer(move |window, app| handler(&id, window, app));
}

pub(super) fn menu_needs_update(shared: &Shared, menu: &NSMenu, target: &AnyObject) {
    let mtm = MainThreadMarker::new().expect("outline view callbacks are main-thread");
    let row = shared.id_at_row(shared.outline.clickedRow());
    let entries = match (&row, shared.on_menu.borrow().as_ref()) {
        (Some(row), Some((entries, _))) => entries(row),
        _ => Vec::new(),
    };
    *shared.menu_row.borrow_mut() = row;
    crate::controls::menu::sync_menu(
        mtm,
        menu,
        &entries,
        Some((target, sel!(ngSidebarMenuPick:))),
    );
}

pub(super) fn menu_picked(shared: &Shared, sender: &AnyObject) {
    let index: NSInteger = unsafe { msg_send![sender, tag] };
    let (Some(row), Some((_, picked))) = (
        shared.menu_row.borrow().clone(),
        shared.on_menu.borrow().clone(),
    ) else {
        return;
    };
    shared.defer(move |window, app| picked(&row, index.max(0) as usize, window, app));
}

pub(super) fn eject_clicked(shared: &Shared, sender: &NSView) {
    let Some(id) = shared.id_at_row(shared.outline.rowForView(sender)) else {
        return;
    };
    let Some(handler) = shared.on_eject.borrow().clone() else {
        return;
    };
    shared.defer(move |window, app| handler(&id, window, app));
}
