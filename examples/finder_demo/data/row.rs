use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use finder_fs::entry::{Entry, EntryKind};
use finder_fs::index::Index;
use finder_fs::kind;
use finder_fs::volumes::{Standard, standard};
use gpui::SharedString;
use native_gpui::tokens::FileType;

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
    pub(crate) display: SharedString,
    pub(crate) path: SharedString,
    pub(crate) size: SharedString,
    pub(crate) bytes: u64,
    pub(crate) modified_at: f64,
    pub(crate) created_at: f64,
    pub(crate) kind: SharedString,
    pub(crate) uti: SharedString,
    pub(crate) face: Face,
    pub(crate) hidden: bool,
    pub(crate) opens: Option<&'static str>,
    /// Finder gives an alias no triangle, however its target opens.
    pub(crate) discloses: bool,
    pub(crate) dataless: bool,
    pub(crate) readable: bool,
    pub(crate) package: bool,
    pub(crate) artwork: Option<FileType<'static>>,
}

fn folder_full(entry: &Entry, path: &str, index: &Index) -> bool {
    entry.children.is_some_and(|held| held > 0)
        && index
            .visible_children(Path::new(path))
            .is_none_or(|seen| seen > 0)
}

fn artwork_of(entry: &Entry, path: &str, uti: &str, index: &Index) -> Option<FileType<'static>> {
    // `iconForFile:` badges a folder the user can't open with Finder's no-entry sign.
    if entry.package || entry.kind == EntryKind::Symlink || entry.alias || !entry.readable {
        return Some(FileType::Path(intern(path)));
    }
    if entry.kind == EntryKind::Directory {
        return Some(folder_kind(intern(path), folder_full(entry, path, index)));
    }
    Some(match entry.extension() {
        Some(extension) => FileType::Extension(intern(&extension.to_ascii_lowercase())),
        None => FileType::Uti(intern(uti)),
    })
}

pub(crate) fn is_trash(path: &str) -> bool {
    standard(Standard::Trash).is_some_and(|trash| trash == Path::new(path))
}

fn folder_kind(path: &'static str, full: bool) -> FileType<'static> {
    // `iconForFile:` on ~/.Trash answers with a plain folder.
    if is_trash(path) {
        return FileType::Named(if full { "NSTrashFull" } else { "NSTrashEmpty" });
    }
    match (special().contains(path), full) {
        (true, true) => FileType::FullPath(path),
        (true, false) => FileType::Path(path),
        (false, true) => FileType::FullFolder,
        (false, false) => FileType::Folder,
    }
}

pub(crate) fn folder_artwork(path: &'static str, holds: bool) -> FileType<'static> {
    if crate::custom::custom_icon(&SharedString::new_static(path)) {
        return FileType::Path(path);
    }
    let seen = crate::index().visible_children(Path::new(path));
    folder_kind(path, seen.map_or(holds, |seen| seen > 0))
}

/// Where a symlink leads: a folder it opens, or whether it is an app, which launches instead.
fn target_of(path: &str) -> (Option<&'static str>, bool) {
    let target = Path::new(path)
        .is_dir()
        .then(|| std::fs::canonicalize(path).ok())
        .flatten();
    let app = target
        .as_ref()
        .and_then(|target| target.file_name())
        .is_some_and(|name| kind::is_package_name(&name.to_string_lossy()));
    (
        target.filter(|_| !app).map(|target| intern_path(&target)),
        app,
    )
}

/// Finder weighs every package it lists, whatever "Calculate all sizes" says; the index has not
/// always weighed them.
pub(crate) fn weigh_packages(rows: &mut [Arc<Row>]) {
    for row in rows
        .iter_mut()
        .filter(|row| row.package && row.size == "--")
    {
        let bytes = finder_fs::ops::walk_size(Path::new(&*row.path));
        *row = Arc::new(Row {
            size: byte_string(bytes).into(),
            bytes,
            ..(**row).clone()
        });
    }
}

fn row_of(dir: &'static str, entry: &Entry, index: &Index) -> Row {
    let path = join(dir, &entry.name);
    let kind = match entry.kind {
        EntryKind::Directory | EntryKind::Symlink if !entry.package => kind::kind_of(entry),
        EntryKind::File if entry.alias => kind::kind_of(entry),
        _ => kind::kind_of_path(Path::new(&path)),
    };
    let bytes = match entry.kind == EntryKind::Directory {
        true => index
            .folder_size(Path::new(&path))
            .filter(|bytes| *bytes > 0 || entry.package),
        false => Some(entry.size),
    };
    let (opens, app) = match entry.kind {
        EntryKind::Symlink => target_of(&path),
        _ => (entry.is_dir().then(|| intern(&path)), false),
    };
    Row {
        name: entry.name.clone().into(),
        display: finder_fs::name::shown(Path::new(dir), &entry.name, entry.ext_hidden).into(),
        size: bytes.map_or("--".into(), |bytes| byte_string(bytes).into()),
        bytes: bytes.unwrap_or(0),
        modified_at: reference_seconds(entry.modified),
        created_at: reference_seconds(entry.created),
        kind: kind.label.to_string().into(),
        uti: kind.uti.to_string().into(),
        face: match app {
            true => Face::Program,
            false => face_of(&kind.uti),
        },
        hidden: entry.hidden,
        opens,
        discloses: entry.is_dir(),
        dataless: entry.dataless,
        readable: entry.readable,
        package: entry.package,
        artwork: artwork_of(entry, &path, &kind.uti, index),
        path: path.into(),
    }
}

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
    let found = String::from_utf8_lossy(&found.stdout);
    let mut rows = rows_at(found.lines().take(limit), index);
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

pub(crate) fn recent_rows(home: &Path, index: &Index, limit: usize) -> Vec<Arc<Row>> {
    const QUERY: &str = r#"(kMDItemLastUsedDate = "*") && ((kMDItemContentTypeTree = public.content) || (kMDItemContentTypeTree = "com.microsoft.*"cdw) || (kMDItemContentTypeTree = public.archive))"#;
    let Ok(found) = std::process::Command::new("/usr/bin/mdfind")
        .arg("-onlyin")
        .arg(home)
        .args(["-attr", "kMDItemLastUsedDate", QUERY])
        .output()
    else {
        return Vec::new();
    };
    let found = String::from_utf8_lossy(&found.stdout);
    // mdfind prints every date in UTC, so the text sorts as the time does.
    let mut used: Vec<(&str, &str)> = found
        .lines()
        .filter_map(|line| line.split_once("   kMDItemLastUsedDate = "))
        .collect();
    used.sort_by(|a, b| b.1.cmp(a.1));
    rows_at(used.into_iter().take(limit).map(|(path, _)| path), index)
}

pub(crate) fn rows_at<'a>(paths: impl Iterator<Item = &'a str>, index: &Index) -> Vec<Arc<Row>> {
    let mut wanted: HashMap<&'static str, HashSet<String>> = HashMap::new();
    for path in paths {
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
    rows
}

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
        uti: "public.volume".into(),
        face: Face::Volume,
        hidden: false,
        opens: Some(id),
        discloses: true,
        dataless: false,
        readable: true,
        package: false,
        artwork: Some(FileType::Path(id)),
    })
}

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
    let entry = finder_fs::list::entry_at(path).ok()?;
    let row = Arc::new(row_of(dir, &entry, crate::index()));
    Some(Line {
        key: row.path.clone(),
        row,
        depth: 0,
    })
}
