//! One listing row, built off the main thread from a directory entry.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use finder_fs::entry::{Entry, EntryKind};
use finder_fs::index::Index;
use finder_fs::kind;
use gpui::SharedString;
use native_gpui::tokens::{FileType, has_custom_icon};

use super::face::{Face, face_of};
use super::format::{byte_string, reference_seconds};
use super::paths::{intern, intern_path, join};
use super::volumes::special;

#[derive(Clone)]
pub(crate) struct Line {
    pub(crate) key: SharedString,
    pub(crate) row: Arc<Row>,
    pub(crate) depth: usize,
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum Column {
    Name,
    Modified,
    Size,
    Kind,
}

#[derive(Clone)]
pub(crate) struct Row {
    pub(crate) name: SharedString,
    /// The file's absolute path: the row's identity everywhere the demo names it.
    pub(crate) path: SharedString,
    pub(crate) size: SharedString,
    pub(crate) bytes: u64,
    pub(crate) modified_at: f64,
    pub(crate) created_at: f64,
    /// Finder's localized kind, e.g. `PDF document`.
    pub(crate) kind: SharedString,
    pub(crate) face: Face,
    pub(crate) hidden: bool,
    /// The folder this row opens: a directory the user can walk into, never a package.
    pub(crate) opens: Option<&'static str>,
    pub(crate) artwork: Option<FileType<'static>>,
}

fn artwork_of(entry: &Entry, path: &str, uti: &str) -> Option<FileType<'static>> {
    if entry.package {
        return Some(FileType::Path(intern(path)));
    }
    if entry.kind == EntryKind::Directory {
        if special().contains(path) || has_custom_icon(Path::new(path)) {
            return Some(FileType::Path(intern(path)));
        }
        return Some(FileType::Folder);
    }
    Some(match entry.extension() {
        Some(extension) => FileType::Extension(intern(&extension.to_ascii_lowercase())),
        None => FileType::Uti(intern(uti)),
    })
}

fn row_of(dir: &'static str, entry: &Entry, index: &Index) -> Row {
    let path = join(dir, &entry.name);
    let kind = match entry.kind {
        EntryKind::Directory | EntryKind::Symlink if !entry.package => kind::kind_of(entry),
        _ => kind::kind_of_path(Path::new(&path)),
    };
    // A package is a directory too: its bytes are the index's to know, not the listing's.
    let directory = entry.kind == EntryKind::Directory;
    let bytes = match directory {
        true => index.folder_size(Path::new(&path)).unwrap_or(0),
        false => entry.size,
    };
    Row {
        name: entry.name.clone().into(),
        size: match directory && bytes == 0 {
            true => "--".into(),
            false => byte_string(bytes).into(),
        },
        bytes,
        modified_at: reference_seconds(entry.modified),
        created_at: reference_seconds(entry.created),
        kind: kind.label.to_string().into(),
        face: face_of(&kind.uti),
        hidden: entry.hidden,
        opens: entry.is_dir().then(|| intern(&path)),
        artwork: artwork_of(entry, &path, &kind.uti),
        path: path.into(),
    }
}

/// One directory's entries as rows. Runs off the main thread, so nothing here may touch a view.
pub(crate) fn rows_of(dir: &'static str, entries: &[Entry], index: &Index) -> Vec<Arc<Row>> {
    entries
        .iter()
        .map(|entry| Arc::new(row_of(dir, entry, index)))
        .collect()
}

/// One row for a file the index found somewhere else on the disk.
pub(crate) fn hit_row(path: &Path, entry: &Entry, index: &Index) -> Arc<Row> {
    let dir = path.parent().map_or("/", intern_path);
    Arc::new(row_of(dir, entry, index))
}

/// Every file Spotlight says carries `tag`, as rows: `mdfind`'s answer read back a folder at a
/// time, so each row is built the way a listing builds it. An empty tag asks for every tagged file.
pub(crate) fn tagged_rows(tag: &str, index: &Index, limit: usize) -> Vec<Arc<Row>> {
    let query = match tag.is_empty() {
        true => "kMDItemUserTags == '*'".to_string(),
        false => format!("kMDItemUserTags == '{tag}'"),
    };
    let Ok(found) = std::process::Command::new("/usr/bin/mdfind")
        .args(["-onlyin", "/"])
        .arg(query)
        .output()
    else {
        return Vec::new();
    };
    let mut wanted: HashMap<&'static str, HashSet<String>> = HashMap::new();
    for path in String::from_utf8_lossy(&found.stdout).lines().take(limit) {
        let path = Path::new(path);
        if let (Some(dir), Some(name)) = (path.parent(), path.file_name()) {
            wanted
                .entry(intern_path(dir))
                .or_default()
                .insert(name.to_string_lossy().into_owned());
        }
    }
    let mut rows: Vec<Arc<Row>> = Vec::new();
    for (dir, names) in wanted {
        let Ok(entries) = finder_fs::list::list_dir(Path::new(dir)) else {
            continue;
        };
        rows.extend(
            entries
                .iter()
                .filter(|entry| names.contains(&entry.name))
                .map(|entry| Arc::new(row_of(dir, entry, index))),
        );
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

/// One line for the item at `id`, for the menus that act on a folder rather than on a row.
pub(crate) fn path_line(id: &'static str) -> Option<Line> {
    let path = Path::new(id);
    let dir = path.parent().map_or("/", intern_path);
    let name = path.file_name()?.to_string_lossy().into_owned();
    let entry = finder_fs::list::list_dir(Path::new(dir))
        .ok()?
        .into_iter()
        .find(|entry| entry.name == name)?;
    let row = Arc::new(row_of(dir, &entry, crate::index()));
    Some(Line {
        key: row.path.clone(),
        row,
        depth: 0,
    })
}
