use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{LazyLock, RwLock};

use gpui::SharedString;

const SEEN_MAX: usize = 4096;

pub(crate) fn intern(text: &str) -> &'static str {
    static POOL: LazyLock<RwLock<HashSet<&'static str>>> = LazyLock::new(Default::default);
    if let Some(known) = POOL.read().unwrap().get(text).copied() {
        return known;
    }
    let mut pool = POOL.write().unwrap();
    if let Some(known) = pool.get(text).copied() {
        return known;
    }
    let kept: &'static str = Box::leak(text.to_owned().into_boxed_str());
    pool.insert(kept);
    kept
}

pub(crate) fn intern_path(path: &Path) -> &'static str {
    intern(&path.to_string_lossy())
}

pub(crate) fn join(dir: &str, name: &str) -> String {
    match dir {
        "/" => format!("/{name}"),
        _ => format!("{dir}/{name}"),
    }
}

pub(crate) fn split_key(key: &str) -> Option<(&'static str, &str)> {
    let (dir, name) = key.rsplit_once('/')?;
    Some((intern(if dir.is_empty() { "/" } else { dir }), name))
}

pub(crate) fn enclosing_name(key: &str) -> SharedString {
    thread_local! {
        static SEEN: RefCell<HashMap<String, SharedString>> = RefCell::new(HashMap::new());
    }
    let parent = Path::new(key).parent().unwrap_or(Path::new("/"));
    let text = parent.to_string_lossy();
    SEEN.with_borrow_mut(|seen| {
        if let Some(name) = seen.get(text.as_ref()) {
            return name.clone();
        }
        if seen.len() >= SEEN_MAX {
            seen.clear();
        }
        let name = SharedString::from(finder_fs::name::of_path(parent));
        seen.insert(text.into_owned(), name.clone());
        name
    })
}
