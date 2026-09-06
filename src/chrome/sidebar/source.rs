//! The outline view's data source and delegate callbacks, one free function each: the delegate
//! classes are built by a macro and only forward to these.

use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, sel};
use objc2_app_kit::{NSMenu, NSView};
use objc2_foundation::{MainThreadMarker, NSInteger};

use super::Shared;
use super::cell::tint_selected_rows;

pub(super) fn children(shared: &Rc<Shared>, item: Option<&AnyObject>) -> NSInteger {
    let model = shared.model.borrow();
    match item {
        None => model.roots.len() as NSInteger,
        Some(item) => model
            .node(Shared::key_of_item(item))
            .map_or(0, |node| node.children.len() as NSInteger),
    }
}

pub(super) fn child(
    shared: &Rc<Shared>,
    index: NSInteger,
    item: Option<&AnyObject>,
) -> Retained<AnyObject> {
    let key = {
        let model = shared.model.borrow();
        let indices = match item {
            None => model.roots.clone(),
            Some(item) => model
                .node(Shared::key_of_item(item))
                .map(|node| node.children.clone())
                .unwrap_or_default(),
        };
        indices
            .get(index as usize)
            .map(|&i| model.nodes[i].key)
            .unwrap_or(0)
    };
    shared.item(key)
}

pub(super) fn expandable(shared: &Rc<Shared>, item: &AnyObject) -> bool {
    shared
        .model
        .borrow()
        .node(Shared::key_of_item(item))
        .is_some_and(|node| !node.children.is_empty())
}

pub(super) fn is_group(shared: &Rc<Shared>, item: &AnyObject) -> bool {
    shared
        .model
        .borrow()
        .node(Shared::key_of_item(item))
        .is_some_and(|node| node.group)
}

pub(super) fn selectable(shared: &Rc<Shared>, item: &AnyObject) -> bool {
    shared
        .model
        .borrow()
        .node(Shared::key_of_item(item))
        .is_some_and(|node| !node.group)
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
    let shared = shared.clone();
    shared
        .cx
        .borrow()
        .spawn(async move |cx| {
            let _ = cx.update(|window, app| handler(&id, window, app));
        })
        .detach();
}

/// Finder builds a sidebar row's menu when the click lands, not when the sidebar is made.
pub(super) fn menu_needs_update(shared: &Rc<Shared>, menu: &NSMenu, target: &AnyObject) {
    let mtm = MainThreadMarker::new().expect("outline view callbacks are main-thread");
    let row = shared
        .outline
        .borrow()
        .as_ref()
        .map(|outline| outline.clickedRow())
        .filter(|&row| row >= 0)
        .and_then(|row| {
            let outline = shared.outline.borrow().clone()?;
            let item = outline.itemAtRow(row)?;
            shared.model.borrow().id_for(Shared::key_of_item(&item))
        });
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
    shared
        .cx
        .borrow()
        .spawn(async move |cx| {
            let _ = cx.update(|window, app| picked(&row, index, window, app));
        })
        .detach();
}

pub(super) fn eject_clicked(shared: &Rc<Shared>, sender: &NSView) {
    let Some(outline) = shared.outline.borrow().clone() else {
        return;
    };
    let row = outline.rowForView(sender);
    let Some(item) = (row >= 0).then(|| outline.itemAtRow(row)).flatten() else {
        return;
    };
    let Some(id) = shared.model.borrow().id_for(Shared::key_of_item(&item)) else {
        return;
    };
    let Some(handler) = shared.on_eject.borrow().clone() else {
        return;
    };
    shared
        .cx
        .borrow()
        .spawn(async move |cx| {
            let _ = cx.update(|window, app| handler(&id, window, app));
        })
        .detach();
}
