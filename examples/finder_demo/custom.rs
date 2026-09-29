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

fn has_custom_icon(path: &SharedString) -> bool {
    native_gpui::tokens::has_custom_icon(Path::new(path.as_ref()))
}

pub(crate) fn custom_icon(path: &SharedString) -> bool {
    if let Some(found) = SEEN.lock().unwrap().get(path).copied() {
        return found;
    }
    WANTED.with_borrow_mut(|wanted| wanted.insert(path.clone()));
    false
}

/// Off the main thread: asks the file now.
pub(crate) fn custom_icon_now(path: &SharedString) -> bool {
    if let Some(found) = SEEN.lock().unwrap().get(path).copied() {
        return found;
    }
    let found = has_custom_icon(path);
    SEEN.lock().unwrap().insert(path.clone(), found);
    found
}

pub(crate) fn set_custom_icon(path: &SharedString, custom: bool) {
    SEEN.lock().unwrap().insert(path.clone(), custom);
}

pub(crate) fn fetch_wanted(demo: &Entity<FinderDemo>, cx: &mut App) {
    let wanted = WANTED.take();
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
                        let custom = has_custom_icon(&path);
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
