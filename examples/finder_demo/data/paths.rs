//! Ids: every path the demo carries is interned once and lives as long as the app.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{OnceLock, RwLock};

use gpui::SharedString;

pub(crate) fn intern(text: &str) -> &'static str {
    static POOL: OnceLock<RwLock<HashSet<&'static str>>> = OnceLock::new();
    let pool = POOL.get_or_init(Default::default);
    // Almost every id asked for is one already kept, so only a new one takes the write lock.
    if let Some(known) = pool.read().unwrap().get(text).copied() {
        return known;
    }
    let mut pool = pool.write().unwrap();
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

/// `dir/name`, with the root's single separator.
pub(crate) fn join(dir: &str, name: &str) -> String {
    match dir {
        "/" => format!("/{name}"),
        _ => format!("{dir}/{name}"),
    }
}

/// The folder a row's key names, and the row's name inside it.
pub(crate) fn split_key(key: &str) -> Option<(&'static str, &str)> {
    let (dir, name) = key.rsplit_once('/')?;
    Some((intern(if dir.is_empty() { "/" } else { dir }), name))
}

/// The folder a key sits in, which Finder's search results show in place of Kind.
pub(crate) fn enclosing_name(key: &str) -> SharedString {
    match Path::new(key).parent().and_then(Path::file_name) {
        Some(name) => finder_fs::name::display(&name.to_string_lossy())
            .into_owned()
            .into(),
        None => "/".into(),
    }
}
