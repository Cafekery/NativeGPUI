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
use super::volumes::{special, volume_at};

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
    /// The name as Finder draws it, which is not always the name the directory stores.
    pub(crate) display: SharedString,
    pub(crate) path: SharedString,
    pub(crate) size: SharedString,
    pub(crate) bytes: u64,
    pub(crate) modified_at: f64,
    pub(crate) created_at: f64,
    pub(crate) kind: SharedString,
    pub(crate) face: Face,
    pub(crate) hidden: bool,
    /// A directory the user can walk into, or an alias's target, never a package.
    pub(crate) opens: Option<&'static str>,
    /// Finder gives an alias no triangle, however its target opens.
    pub(crate) discloses: bool,
    /// Evicted to iCloud: Finder badges the row with a cloud and downloads it when it opens.
    pub(crate) dataless: bool,
    pub(crate) artwork: Option<FileType<'static>>,
}

fn folder_full(entry: &Entry, path: &str, index: &Index) -> bool {
    entry.children.is_some_and(|held| held > 0)
        && index
            .visible_children(Path::new(path))
            .is_none_or(|seen| seen > 0)
}

fn artwork_of(entry: &Entry, path: &str, uti: &str, index: &Index) -> Option<FileType<'static>> {
    // An alias wears the icon of what it points at, badged; the system draws that for the link.
    if entry.package || entry.kind == EntryKind::Symlink {
        return Some(FileType::Path(intern(path)));
    }
    if entry.kind == EntryKind::Directory {
        if special().contains(path) || has_custom_icon(Path::new(path)) {
            return Some(FileType::Path(intern(path)));
        }
        return Some(match folder_full(entry, path, index) {
            true => FileType::FullFolder,
            false => FileType::Folder,
        });
    }
    Some(match entry.extension() {
        Some(extension) => FileType::Extension(intern(&extension.to_ascii_lowercase())),
        None => FileType::Uti(intern(uti)),
    })
}

/// Finder opens an alias at the folder it points at, wherever that is.
fn target_folder(path: &str) -> Option<&'static str> {
    let path = Path::new(path);
    path.is_dir()
        .then(|| std::fs::canonicalize(path).ok())
        .flatten()
        .map(|target| intern_path(&target))
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
        display: finder_fs::name::display(&entry.name).into_owned().into(),
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
        opens: match entry.kind {
            EntryKind::Symlink => target_folder(&path),
            _ => entry.is_dir().then(|| intern(&path)),
        },
        discloses: entry.is_dir(),
        dataless: entry.dataless,
        artwork: artwork_of(entry, &path, &kind.uti, index),
        path: path.into(),
    }
}

/// Runs off the main thread, so nothing here may touch a view.
pub(crate) fn rows_of(dir: &'static str, entries: &[Entry], index: &Index) -> Vec<Arc<Row>> {
    entries
        .iter()
        .map(|entry| Arc::new(row_of(dir, entry, index)))
        .collect()
}

pub(crate) fn hit_row(path: &Path, entry: &Entry, index: &Index) -> Arc<Row> {
    let dir = path.parent().map_or("/", intern_path);
    Arc::new(row_of(dir, entry, index))
}

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

/// The boot volume is under no folder at all, and the rest wear a name the mount point need not
/// carry.
fn volume_row(id: &'static str) -> Option<Row> {
    let volume = volume_at(id)?;
    let meta = std::fs::metadata(id).ok()?;
    Some(Row {
        name: volume.name.into(),
        display: volume.name.into(),
        path: id.into(),
        size: byte_string(volume.capacity).into(),
        bytes: volume.capacity,
        modified_at: meta.modified().ok().map_or(0., reference_seconds),
        created_at: meta.created().ok().map_or(0., reference_seconds),
        kind: "Volume".into(),
        face: Face::Volume,
        hidden: false,
        opens: Some(id),
        discloses: true,
        dataless: false,
        artwork: Some(FileType::Path(id)),
    })
}

/// iCloud Drive's app folders, which Finder lists there and the directory does not hold.
pub(crate) fn icloud_rows(index: &Index) -> Vec<Arc<Row>> {
    finder_fs::icloud::app_folders()
        .into_iter()
        .filter_map(|folder| {
            let dir = intern_path(folder.documents.parent()?);
            let entry = finder_fs::list::list_dir(Path::new(dir))
                .ok()?
                .into_iter()
                .find(|entry| entry.name == "Documents")?;
            let mut row = row_of(dir, &entry, index);
            row.name = folder.name.clone().into();
            row.display = folder.name.into();
            Some(Arc::new(row))
        })
        .collect()
}

pub(crate) fn path_line(id: &'static str) -> Option<Line> {
    if let Some(row) = volume_row(id) {
        return Some(Line {
            key: row.path.clone(),
            row: Arc::new(row),
            depth: 0,
        });
    }
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
