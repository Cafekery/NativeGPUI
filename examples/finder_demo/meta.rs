//! Spotlight's attributes for the row on show, asked for once per file off the main thread.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use finder_fs::meta::Meta;
use gpui::{App, Entity, SharedString};

use crate::app::FinderDemo;

thread_local! {
    /// Every file asked about: its attributes, or nothing while the request is still out.
    static META: RefCell<HashMap<SharedString, Option<Arc<Meta>>>> = RefCell::new(HashMap::new());
    static WANTED: RefCell<HashSet<SharedString>> = RefCell::new(HashSet::new());
}

/// Spotlight's answer for this file once it has come back, and a request for it until then.
pub(crate) fn meta(path: &SharedString) -> Option<Arc<Meta>> {
    if let Some(found) = META.with(|map| map.borrow().get(path).cloned()) {
        return found;
    }
    WANTED.with(|wanted| wanted.borrow_mut().insert(path.clone()));
    None
}

/// Ask for what the frames since the last call wanted: one Spotlight round trip per file.
pub(crate) fn fetch_wanted(demo: &Entity<FinderDemo>, cx: &mut App) {
    let wanted: Vec<SharedString> = WANTED.with(|wanted| wanted.borrow_mut().drain().collect());
    if wanted.is_empty() {
        return;
    }
    for path in &wanted {
        META.with(|map| map.borrow_mut().insert(path.clone(), None));
    }
    let demo = demo.downgrade();
    cx.spawn(async move |cx| {
        for path in wanted {
            let file = PathBuf::from(path.as_ref());
            let found = cx
                .background_executor()
                .spawn(async move { finder_fs::meta::meta(&file) })
                .await;
            if demo
                .update(cx, |_, cx| {
                    META.with(|map| map.borrow_mut().insert(path, Some(Arc::new(found))));
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
        }
    })
    .detach();
}
