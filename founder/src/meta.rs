use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use finder_fs::meta::Meta;
use gpui::{App, Entity, SharedString};

use crate::app::FinderDemo;

thread_local! {
    static META: RefCell<HashMap<SharedString, Option<Arc<Meta>>>> = RefCell::new(HashMap::new());
    static WANTED: RefCell<HashSet<SharedString>> = RefCell::new(HashSet::new());
}

pub(crate) fn meta(path: &SharedString) -> Option<Arc<Meta>> {
    if crate::remote::is_remote(path) {
        return None;
    }
    if let Some(found) = META.with_borrow(|map| map.get(path).cloned()) {
        return found;
    }
    WANTED.with_borrow_mut(|wanted| wanted.insert(path.clone()));
    None
}

pub(crate) fn fetch_wanted(demo: &Entity<FinderDemo>, cx: &mut App) {
    let wanted = WANTED.take();
    if wanted.is_empty() {
        return;
    }
    META.with_borrow_mut(|map| map.extend(wanted.iter().map(|path| (path.clone(), None))));
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
                    META.with_borrow_mut(|map| map.insert(path, Some(Arc::new(found))));
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
