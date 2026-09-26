use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use gpui::{App, Entity, SharedString};

use crate::app::FinderDemo;

static SEEN: LazyLock<Mutex<HashMap<SharedString, bool>>> = LazyLock::new(Default::default);

thread_local! {
    static WANTED: RefCell<HashSet<SharedString>> = RefCell::new(HashSet::new());
}

pub(crate) fn custom_icon(path: &SharedString) -> bool {
    if let Some(found) = SEEN.lock().unwrap().get(path).copied() {
        return found;
    }
    WANTED.with(|wanted| wanted.borrow_mut().insert(path.clone()));
    false
}

/// Off the main thread: asks the file now.
pub(crate) fn custom_icon_now(path: &SharedString) -> bool {
    if let Some(found) = SEEN.lock().unwrap().get(path).copied() {
        return found;
    }
    let found = native_gpui::tokens::has_custom_icon(Path::new(path.as_ref()));
    SEEN.lock().unwrap().insert(path.clone(), found);
    found
}

pub(crate) fn fetch_wanted(demo: &Entity<FinderDemo>, cx: &mut App) {
    let wanted: Vec<SharedString> = WANTED.with(|wanted| wanted.borrow_mut().drain().collect());
    if wanted.is_empty() {
        return;
    }
    {
        let mut seen = SEEN.lock().unwrap();
        for path in &wanted {
            seen.entry(path.clone()).or_insert(false);
        }
    }
    let demo = demo.downgrade();
    cx.spawn(async move |cx| {
        let found = cx
            .background_executor()
            .spawn(async move {
                wanted
                    .into_iter()
                    .map(|path| {
                        let custom = native_gpui::tokens::has_custom_icon(Path::new(path.as_ref()));
                        (path, custom)
                    })
                    .collect::<Vec<_>>()
            })
            .await;
        let any = found.iter().any(|(_, custom)| *custom);
        demo.update(cx, |_, cx| {
            SEEN.lock().unwrap().extend(found);
            if any {
                cx.notify();
            }
        })
        .ok();
    })
    .detach();
}
