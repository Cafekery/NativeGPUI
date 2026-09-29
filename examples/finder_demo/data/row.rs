use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex};

use finder_fs::entry::{Entry, EntryKind};
use finder_fs::index::Index;
use finder_fs::kind;
use finder_fs::volumes::{Standard, standard};
use gpui::SharedString;
use native_gpui::tokens::FileType;

use super::extra::{Extra, extra_of};
use super::face::{Face, face_of};
use super::format::{byte_string, reference_seconds};
use super::paths::{intern, intern_path, join};
use super::volumes::{special, volume_at};

const PACKAGES_HELD: usize = 4096;

#[derive(Clone)]
pub(crate) struct Line {
    pub(crate) key: SharedString,
    pub(crate) row: Arc<Row>,
    pub(crate) depth: usize,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Column {
    Name,
    Kind,
    Opened,
    Added,
    Modified,
    Created,
    Size,
    Tags,
    Version,
    Comments,
    /// The app that opens an item: a group key only.
    Application,
}

pub(crate) const SORT_KEYS: [Column; 8] = [
    Column::Name,
    Column::Kind,
    Column::Opened,
    Column::Added,
    Column::Modified,
    Column::Created,
    Column::Size,
    Column::Tags,
];

pub(crate) const SORT_TITLES: [&str; 8] = [
    "Name",
    "Kind",
    "Date Last Opened",
    "Date Added",
    "Date Modified",
    "Date Created",
    "Size",
    "Tags",
];

pub(crate) const GROUP_KEYS: [Column; 9] = [
    Column::Name,
    Column::Kind,
    Column::Application,
    Column::Opened,
    Column::Added,
    Column::Modified,
    Column::Created,
    Column::Size,
    Column::Tags,
];

/// Rows sort by their group's band first, the bands in the order their key sorts first.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Order {
    pub(crate) group: Option<Column>,
    pub(crate) sort: Column,
    pub(crate) ascending: bool,
}

impl Column {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Column::Name => "name",
            Column::Kind => "kind",
            Column::Opened => "dateLastOpened",
            Column::Added => "dateAdded",
            Column::Modified => "dateModified",
            Column::Created => "dateCreated",
            Column::Size => "size",
            Column::Tags => "label",
            Column::Version => "version",
            Column::Comments => "comments",
            Column::Application => "application",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Self> {
        (SORT_KEYS.into_iter())
            .chain([Column::Version, Column::Comments, Column::Application])
            .find(|column| column.id() == id)
    }

    pub(crate) fn first_ascending(self) -> bool {
        matches!(
            self,
            Column::Name
                | Column::Kind
                | Column::Tags
                | Column::Version
                | Column::Comments
                | Column::Application
        )
    }

    pub(crate) fn dated(self) -> bool {
        matches!(
            self,
            Column::Opened | Column::Added | Column::Modified | Column::Created
        )
    }

    pub(crate) fn extra(self) -> bool {
        matches!(
            self,
            Column::Opened | Column::Tags | Column::Version | Column::Comments
        )
    }
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
    pub(crate) added_at: Option<f64>,
    pub(crate) kind: SharedString,
    pub(crate) uti: SharedString,
    pub(crate) face: Face,
    pub(crate) hidden: bool,
    pub(crate) opens: Option<&'static str>,
    // Finder gives an alias no triangle, however its target opens.
    pub(crate) discloses: bool,
    pub(crate) dataless: bool,
    pub(crate) readable: bool,
    pub(crate) package: bool,
    pub(crate) artwork: Option<FileType<'static>>,
    pub(crate) extra: Option<Arc<Extra>>,
    pub(crate) app: Option<SharedString>,
}

impl Row {
    pub(crate) fn date(&self, column: Column) -> Option<f64> {
        match column {
            Column::Modified => Some(self.modified_at),
            Column::Created => Some(self.created_at),
            Column::Added => self.added_at,
            Column::Opened => self.extra.as_ref()?.opened,
            _ => None,
        }
    }

    pub(crate) fn words(&self, column: Column) -> Option<&str> {
        let extra = self.extra.as_deref();
        match column {
            Column::Kind => Some(&self.kind),
            Column::Tags => extra?.tags.first().map(|(name, _)| &**name),
            Column::Version => extra?.version.as_deref(),
            Column::Comments => Some(&extra?.comment),
            Column::Application => self.app.as_deref(),
            _ => None,
        }
    }
}

pub(crate) type Facts = (Option<u64>, Option<usize>);

fn indexed_row(dir: &'static str, entry: &Entry, index: &Index) -> Row {
    let facts = match entry.kind {
        EntryKind::Directory => {
            let path = Path::new(dir).join(&entry.name);
            (index.folder_size(&path), index.visible_children(&path))
        }
        _ => (None, None),
    };
    row_of(dir, entry, facts)
}

// `iconForFile:` badges a folder the user can't open with the no-entry sign.
fn own_icon(entry: &Entry) -> bool {
    entry.package
        || entry.kind == EntryKind::Symlink
        || entry.alias
        || entry.custom_icon
        || !entry.readable
}

/// A row's art where its entry alone tells, before its kind is read: none for a plain folder.
pub(crate) fn early_artwork(path: &str, entry: &Entry) -> Option<FileType<'static>> {
    if own_icon(entry) {
        return Some(FileType::Path(intern(path)));
    }
    if entry.kind == EntryKind::Directory {
        return None;
    }
    let extension = entry.extension()?.to_ascii_lowercase();
    Some(FileType::Extension(intern(&extension)))
}

fn artwork_of(entry: &Entry, path: &str, uti: &str, seen: Option<usize>) -> FileType<'static> {
    if let Some(art) = early_artwork(path, entry) {
        return art;
    }
    if entry.kind == EntryKind::Directory {
        let full = entry.children.is_some_and(|held| held > 0) && seen.is_none_or(|seen| seen > 0);
        return folder_kind(intern(path), full);
    }
    FileType::Uti(intern(uti))
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

// Finder weighs every package it lists, whatever "Calculate all sizes" says.
pub(crate) fn weigh_packages(rows: &mut [Arc<Row>]) {
    static WEIGHED: LazyLock<Mutex<HashMap<(SharedString, u64), u64>>> =
        LazyLock::new(Mutex::default);
    for row in rows
        .iter_mut()
        .filter(|row| row.package && row.size == "--")
    {
        let key = (row.path.clone(), row.modified_at.to_bits());
        let known = WEIGHED.lock().unwrap().get(&key).copied();
        let bytes = known.unwrap_or_else(|| {
            let bytes = finder_fs::ops::walk_size(Path::new(&*row.path));
            let mut weighed = WEIGHED.lock().unwrap();
            if weighed.len() >= PACKAGES_HELD {
                weighed.clear();
            }
            weighed.insert(key, bytes);
            bytes
        });
        *row = Arc::new(Row {
            size: byte_string(bytes).into(),
            bytes,
            ..(**row).clone()
        });
    }
}

pub(crate) fn kind_at(path: &str, entry: &Entry) -> kind::Kind {
    match entry.kind {
        EntryKind::Directory | EntryKind::Symlink if !entry.package => kind::kind_of(entry),
        EntryKind::File if entry.alias => kind::kind_of(entry),
        _ => kind::kind_of_path(Path::new(path)),
    }
}

pub(crate) fn bytes_of(entry: &Entry, size: Option<u64>) -> Option<u64> {
    match entry.kind == EntryKind::Directory {
        true => size.filter(|bytes| *bytes > 0 || entry.package),
        false => Some(entry.size),
    }
}

pub(crate) fn modified_at(entry: &Entry) -> f64 {
    reference_seconds(entry.modified)
}

pub(crate) fn facts_of(entry: &Entry, folders: &HashMap<String, Facts>) -> Facts {
    match entry.kind {
        EntryKind::Directory => folders.get(&entry.name).copied().unwrap_or_default(),
        _ => (None, None),
    }
}

fn row_of(dir: &'static str, entry: &Entry, (size, seen): Facts) -> Row {
    let path = join(dir, &entry.name);
    let kind = kind_at(&path, entry);
    let bytes = bytes_of(entry, size);
    let (opens, app) = match entry.kind {
        EntryKind::Symlink => target_of(&path),
        _ => (entry.is_dir().then(|| intern(&path)), false),
    };
    Row {
        name: entry.name.clone().into(),
        display: finder_fs::name::shown(Path::new(dir), &entry.name, entry.ext_hidden).into(),
        size: bytes.map_or("--".into(), |bytes| byte_string(bytes).into()),
        bytes: bytes.unwrap_or(0),
        modified_at: modified_at(entry),
        created_at: reference_seconds(entry.created),
        added_at: entry.added.map(reference_seconds),
        kind: (&kind.label).into(),
        uti: (&kind.uti).into(),
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
        artwork: Some(artwork_of(entry, &path, &kind.uti, seen)),
        extra: None,
        app: None,
        path: path.into(),
    }
}

pub(crate) fn rows_of<'a>(
    dir: &'static str,
    entries: impl IntoIterator<Item = &'a Entry>,
    folders: &HashMap<String, Facts>,
    extras: bool,
) -> Vec<Arc<Row>> {
    let mut comments = match extras {
        true => finder_fs::ds_store::comments(Path::new(dir)),
        false => HashMap::new(),
    };
    entries
        .into_iter()
        .map(|entry| {
            let mut row = row_of(dir, entry, facts_of(entry, folders));
            if extras {
                let comment = comments.remove(&entry.name);
                row.extra = Some(extra_of(&row.path, row.package, comment));
            }
            Arc::new(row)
        })
        .collect()
}

pub(crate) fn read_extras(rows: &mut [Arc<Row>]) {
    let mut comments: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();
    for row in rows.iter_mut().filter(|row| row.extra.is_none()) {
        let path = Path::new(&*row.path);
        let comment = path.parent().zip(path.file_name()).and_then(|(dir, name)| {
            if !comments.contains_key(dir) {
                comments.insert(dir.to_path_buf(), finder_fs::ds_store::comments(dir));
            }
            comments.get_mut(dir)?.remove(name.to_str()?)
        });
        *row = Arc::new(Row {
            extra: Some(extra_of(&row.path, row.package, comment)),
            ..(**row).clone()
        });
    }
}

pub(crate) fn hit_row(path: &Path, entry: &Entry, index: &Index) -> Arc<Row> {
    Arc::new(indexed_row(
        path.parent().map_or("/", intern_path),
        entry,
        index,
    ))
}

fn mdfind(args: &[&OsStr]) -> String {
    match Command::new("/usr/bin/mdfind").args(args).output() {
        Ok(found) => String::from_utf8_lossy(&found.stdout).into_owned(),
        Err(_) => String::new(),
    }
}

pub(crate) fn tagged_rows(tag: &str, index: &Index, limit: usize) -> Vec<Arc<Row>> {
    let query = match tag.is_empty() {
        true => "kMDItemUserTags == '*'".to_string(),
        false => format!("kMDItemUserTags == '{tag}'"),
    };
    let found = mdfind(&["-onlyin".as_ref(), "/".as_ref(), query.as_ref()]);
    let mut rows = rows_at(found.lines().take(limit), index);
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

pub(crate) fn recent_rows(home: &Path, index: &Index, limit: usize) -> Vec<Arc<Row>> {
    const QUERY: &str = r#"(kMDItemLastUsedDate = "*") && ((kMDItemContentTypeTree = public.content) || (kMDItemContentTypeTree = "com.microsoft.*"cdw) || (kMDItemContentTypeTree = public.archive))"#;
    let found = mdfind(&[
        "-onlyin".as_ref(),
        home.as_ref(),
        "-attr".as_ref(),
        "kMDItemLastUsedDate".as_ref(),
        QUERY.as_ref(),
    ]);
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
    wanted
        .into_iter()
        .flat_map(|(dir, names)| {
            let entries = finder_fs::list::list_dir(Path::new(dir)).unwrap_or_default();
            entries
                .into_iter()
                .filter(move |entry| names.contains(&entry.name))
                .map(move |entry| Arc::new(indexed_row(dir, &entry, index)))
        })
        .collect()
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
        added_at: None,
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
        extra: None,
        app: None,
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
            let mut row = indexed_row(dir, &entry, index);
            row.name = folder.name.clone().into();
            row.display = folder.name.into();
            Some(Arc::new(row))
        })
        .collect()
}

pub(crate) fn path_line(id: &'static str) -> Option<Line> {
    let row = match volume_row(id) {
        Some(row) => Arc::new(row),
        None => {
            let path = Path::new(id);
            hit_row(path, &finder_fs::list::entry_at(path).ok()?, crate::index())
        }
    };
    Some(Line {
        key: row.path.clone(),
        row,
        depth: 0,
    })
}
