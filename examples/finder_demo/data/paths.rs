//! Ids: every path the demo carries is interned once and lives as long as the app.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use gpui::SharedString;

/// Every id the demo carries is one of these: an absolute path that lives as long as the app.
pub(crate) fn intern(text: &str) -> &'static str {
    static POOL: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut pool = POOL.get_or_init(Default::default).lock().unwrap();
    if let Some(known) = pool.get(text) {
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

/// The name of the folder a search result's key sits in, the way Finder's search results show a
/// path in place of Kind.
pub(crate) fn enclosing_name(key: &str) -> SharedString {
    match Path::new(key).parent().and_then(Path::file_name) {
        Some(name) => name.to_string_lossy().into_owned().into(),
        None => "/".into(),
    }
}
