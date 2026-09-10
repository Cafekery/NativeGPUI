use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gpui::{App, Entity, SharedString};

use crate::app::FinderDemo;

thread_local! {
    static SEEN: RefCell<HashMap<SharedString, bool>> = RefCell::new(HashMap::new());
    static WANTED: RefCell<HashSet<SharedString>> = RefCell::new(HashSet::new());
}

/// Whether the folder wears an icon of its own. The answer is a `getxattr` against a path the
/// kernel has usually never resolved, which costs milliseconds, so a row draws the plain folder
/// until the answer lands.
pub(crate) fn custom_icon(path: &SharedString) -> bool {
    if let Some(&found) = SEEN.with(|seen| seen.borrow().get(path).copied()).as_ref() {
        return found;
    }
    WANTED.with(|wanted| wanted.borrow_mut().insert(path.clone()));
    false
}

pub(crate) fn fetch_wanted(demo: &Entity<FinderDemo>, cx: &mut App) {
    let wanted: Vec<SharedString> = WANTED.with(|wanted| wanted.borrow_mut().drain().collect());
    if wanted.is_empty() {
        return;
    }
    for path in &wanted {
        SEEN.with(|seen| seen.borrow_mut().insert(path.clone(), false));
    }
    let demo = demo.downgrade();
    cx.spawn(async move |cx| {
        let found = cx
            .background_executor()
            .spawn(async move {
                wanted
                    .into_iter()
                    .map(|path| {
                        let custom =
                            native_gpui::tokens::has_custom_icon(&PathBuf::from(path.as_ref()));
                        (path, custom)
                    })
                    .collect::<Vec<_>>()
            })
            .await;
        let any = found.iter().any(|(_, custom)| *custom);
        demo.update(cx, |_, cx| {
            SEEN.with(|seen| seen.borrow_mut().extend(found));
            if any {
                cx.notify();
            }
        })
        .ok();
    })
    .detach();
}
