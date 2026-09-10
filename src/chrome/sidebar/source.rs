use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, sel};
use objc2_app_kit::{NSMenu, NSView};
use objc2_foundation::{MainThreadMarker, NSInteger};

use super::Shared;
use super::cell::tint_selected_rows;

pub(super) fn children(shared: &Rc<Shared>, item: Option<&AnyObject>) -> NSInteger {
    shared
        .model
        .borrow()
        .children_of(item.map(Shared::key_of_item))
        .len() as NSInteger
}

pub(super) fn child(
    shared: &Rc<Shared>,
    index: NSInteger,
    item: Option<&AnyObject>,
) -> Retained<AnyObject> {
    let key = shared
        .model
        .borrow()
        .children_keys(item.map(Shared::key_of_item))
        .get(index as usize)
        .copied()
        .unwrap_or(0);
    shared.item(key)
}

pub(super) fn expandable(shared: &Rc<Shared>, item: &AnyObject) -> bool {
    shared.node_is(Shared::key_of_item(item), |node| !node.children.is_empty())
}

pub(super) fn is_group(shared: &Rc<Shared>, item: &AnyObject) -> bool {
    shared.node_is(Shared::key_of_item(item), |node| node.group)
}

pub(super) fn selectable(shared: &Rc<Shared>, item: &AnyObject) -> bool {
    shared.node_is(Shared::key_of_item(item), |node| !node.group)
}

pub(super) fn selection_changed(shared: &Rc<Shared>) {
    if let Some(outline) = shared.outline.borrow().clone() {
        tint_selected_rows(&outline);
    }
    if shared.suppress.get() > 0 {
        return;
    }
    let Some(handler) = shared.on_select.borrow().clone() else {
        return;
    };
    let id = shared.selected_id();
    shared.defer(move |window, app| handler(&id, window, app));
}

pub(super) fn menu_needs_update(shared: &Rc<Shared>, menu: &NSMenu, target: &AnyObject) {
    let mtm = MainThreadMarker::new().expect("outline view callbacks are main-thread");
    let row = shared
        .outline
        .borrow()
        .clone()
        .and_then(|outline| shared.id_at_row(&outline, outline.clickedRow()));
    *shared.menu_row.borrow_mut() = row.clone();
    let entries = match (row, shared.on_menu.borrow().clone()) {
        (Some(row), Some((entries, _))) => entries(&row),
        _ => Vec::new(),
    };
    crate::controls::menu::sync_menu(
        mtm,
        menu,
        &entries,
        Some((target, sel!(ngSidebarMenuPick:))),
    );
}

pub(super) fn menu_picked(shared: &Rc<Shared>, sender: &AnyObject) {
    let index: NSInteger = unsafe { msg_send![sender, tag] };
    let Some(row) = shared.menu_row.borrow().clone() else {
        return;
    };
    let Some((_, picked)) = shared.on_menu.borrow().clone() else {
        return;
    };
    let index = index.max(0) as usize;
    shared.defer(move |window, app| picked(&row, index, window, app));
}

pub(super) fn eject_clicked(shared: &Rc<Shared>, sender: &NSView) {
    let Some(outline) = shared.outline.borrow().clone() else {
        return;
    };
    let Some(id) = shared.id_at_row(&outline, outline.rowForView(sender)) else {
        return;
    };
    let Some(handler) = shared.on_eject.borrow().clone() else {
        return;
    };
    shared.defer(move |window, app| handler(&id, window, app));
}
