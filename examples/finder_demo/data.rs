//! The real file system as the demo sees it: interned absolute paths for ids, cached listings, and
//! the system formatters that turn entries into rows.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use finder_fs::entry::{Entry, EntryKind};
use finder_fs::index::Index;
use finder_fs::{kind, volumes};
use gpui::SharedString;
use native_gpui::tokens::{FileType, SymbolColor, SystemColor, has_custom_icon};
use objc2::rc::Retained;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{
    NSByteCountFormatter, NSByteCountFormatterCountStyle, NSCalendar, NSDate, NSDateFormatter,
    NSDateFormatterStyle, NSFileManager, NSString,
};

use self::Face as F;

/// How many listings stay in memory behind the one on screen.
const CACHED: usize = 64;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Face {
    Folder,
    Volume,
    Pdf,
    Text,
    Code,
    Sheet,
    Pages,
    Slides,
    Image,
    Movie,
    Audio,
    Archive,
    Disk,
    Program,
    Web,
}

impl Face {
    pub(crate) fn symbol(self) -> &'static str {
        match self {
            F::Folder => "folder.fill",
            F::Volume => "internaldrive.fill",
            F::Pdf => "doc.richtext.fill",
            F::Text => "doc.text.fill",
            F::Code => "chevron.left.forwardslash.chevron.right",
            F::Sheet => "tablecells.fill",
            F::Pages => "doc.fill",
            F::Slides => "rectangle.on.rectangle.fill",
            F::Image => "photo.fill",
            F::Movie => "film.fill",
            F::Audio => "waveform",
            F::Archive => "shippingbox.fill",
            F::Disk => "externaldrive.fill",
            F::Program => "app.fill",
            F::Web => "safari.fill",
        }
    }

    pub(crate) fn tint(self) -> SymbolColor {
        let color = match self {
            F::Folder => SystemColor::SystemBlue,
            F::Volume => SystemColor::SystemGray,
            F::Pdf => SystemColor::SystemRed,
            F::Text => SystemColor::SecondaryLabel,
            F::Code => SystemColor::SystemGreen,
            F::Sheet => SystemColor::SystemGreen,
            F::Pages => SystemColor::SystemOrange,
            F::Slides => SystemColor::SystemIndigo,
            F::Image => SystemColor::SystemTeal,
            F::Movie => SystemColor::SystemPurple,
            F::Audio => SystemColor::SystemPink,
            F::Archive => SystemColor::SystemBrown,
            F::Disk => SystemColor::SystemGray,
            F::Program => SystemColor::SystemBlue,
            F::Web => SystemColor::SystemCyan,
        };
        SymbolColor::Hierarchical(color)
    }
}

/// The face a type wears, by what its uniform type identifier conforms to, most specific first.
const FACES: &[(&str, Face)] = &[
    ("public.folder", F::Folder),
    ("public.volume", F::Volume),
    ("com.apple.application", F::Program),
    ("com.adobe.pdf", F::Pdf),
    ("public.image", F::Image),
    ("public.movie", F::Movie),
    ("public.audio", F::Audio),
    ("public.archive", F::Archive),
    ("public.disk-image", F::Disk),
    ("public.spreadsheet", F::Sheet),
    ("public.presentation", F::Slides),
    ("com.apple.iwork.pages.pages", F::Pages),
    ("public.source-code", F::Code),
    ("public.html", F::Web),
    ("public.text", F::Text),
];

// `NSWorkspace` is the only type conformance AppKit exposes; `UTType` is in a framework this
// example does not link. The lookup is LaunchServices', not AppKit state: fine off the main thread.
#[allow(deprecated)]
fn face_of(uti: &str) -> Face {
    static SEEN: OnceLock<RwLock<HashMap<String, Face>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    if let Some(face) = seen.read().unwrap().get(uti) {
        return *face;
    }
    let workspace = NSWorkspace::sharedWorkspace();
    let mine = NSString::from_str(uti);
    let face = FACES
        .iter()
        .find(|(parent, _)| workspace.type_conformsToType(&mine, &NSString::from_str(parent)))
        .map_or(F::Text, |(_, face)| *face);
    seen.write().unwrap().insert(uti.to_owned(), face);
    face
}

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

/// Every mounted volume: mount point, name, and the bytes it has free.
fn volumes_seen() -> &'static RwLock<Vec<(String, &'static str, u64)>> {
    static SEEN: OnceLock<RwLock<Vec<(String, &'static str, u64)>>> = OnceLock::new();
    SEEN.get_or_init(Default::default)
}

/// Re-reads the mounted volumes. Enumerating them costs milliseconds, so nothing on a frame's
/// path may call it: the path bar, the sidebar and the status bar read this snapshot.
pub(crate) fn refresh_volumes() {
    let mounted = volumes::mounted()
        .iter()
        .map(|volume| {
            (
                volume.path.to_string_lossy().into_owned(),
                intern(&volume.name),
                volume.available,
            )
        })
        .collect();
    *volumes_seen().write().unwrap() = mounted;
}

fn boot_name() -> &'static str {
    static NAME: OnceLock<&'static str> = OnceLock::new();
    NAME.get_or_init(|| {
        let name = NSFileManager::defaultManager()
            .displayNameAtPath(&NSString::from_str("/"))
            .to_string();
        intern(&name)
    })
}

/// The mount point `path` sits on, and that volume's name.
fn volume_of(path: &str) -> (&'static str, &'static str) {
    let under =
        |mount: &str| mount == "/" || path == mount || path.starts_with(&format!("{mount}/"));
    volumes_seen()
        .read()
        .unwrap()
        .iter()
        .filter(|(mount, ..)| under(mount))
        .max_by_key(|(mount, ..)| mount.len())
        .map(|(mount, name, _)| (intern(mount), *name))
        .unwrap_or(("/", boot_name()))
}

/// The volumes' mount points, root first, for the sidebar's Locations.
pub(crate) fn volume_roots() -> Vec<&'static str> {
    volumes_seen()
        .read()
        .unwrap()
        .iter()
        .map(|(mount, ..)| intern(mount))
        .collect()
}

/// The bytes free on the volume `id` sits on, as of the last [`refresh_volumes`].
pub(crate) fn available_bytes(id: &str) -> u64 {
    let (mount, _) = volume_of(id);
    volumes_seen()
        .read()
        .unwrap()
        .iter()
        .find(|(seen, ..)| seen == mount)
        .map_or(0, |(.., available)| *available)
}

/// The folders whose artwork is their own, so they draw as Finder draws them.
fn special() -> &'static HashSet<String> {
    static PLACES: OnceLock<HashSet<String>> = OnceLock::new();
    PLACES.get_or_init(|| {
        use finder_fs::volumes::Standard::*;
        let mut places: HashSet<String> = [
            Home,
            Desktop,
            Documents,
            Downloads,
            Applications,
            Movies,
            Music,
            Pictures,
            Trash,
        ]
        .into_iter()
        .filter_map(volumes::standard)
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
        let home = volumes::standard(Home).unwrap_or_default();
        for leaf in ["Public", "Library", "Developer", "Sites", "Applications"] {
            places.insert(home.join(leaf).to_string_lossy().into_owned());
        }
        for path in [
            "/",
            "/Library",
            "/System",
            "/Users",
            "/Users/Shared",
            "/Volumes",
        ] {
            places.insert(path.to_string());
        }
        places
    })
}

const UNIX_TO_REFERENCE: f64 = 978_307_200.;

fn reference_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0., |since| since.as_secs_f64() - UNIX_TO_REFERENCE)
}

/// Finder's date column, formatted once a minute and shared by every row in it: `NSDateFormatter`
/// costs tens of microseconds a call, which 20 000 rows cannot pay.
pub(crate) fn date_text(seconds: f64) -> SharedString {
    thread_local! {
        static FORMATTER: Retained<NSDateFormatter> = {
            let formatter = NSDateFormatter::new();
            formatter.setDateStyle(NSDateFormatterStyle::ShortStyle);
            formatter.setTimeStyle(NSDateFormatterStyle::ShortStyle);
            formatter.setDoesRelativeDateFormatting(true);
            formatter
        };
        static SEEN: RefCell<HashMap<i64, SharedString>> = RefCell::new(HashMap::new());
    }
    if seconds == 0. {
        return "--".into();
    }
    let minute = (seconds / 60.).floor() as i64;
    SEEN.with(|seen| {
        if let Some(text) = seen.borrow().get(&minute) {
            return text.clone();
        }
        let text = FORMATTER.with(|formatter| {
            SharedString::from(
                formatter
                    .stringFromDate(&NSDate::dateWithTimeIntervalSinceReferenceDate(seconds))
                    .to_string(),
            )
        });
        seen.borrow_mut().insert(minute, text.clone());
        text
    })
}

/// The band Finder files a row under while Use Groups is on.
pub(crate) fn group_of(row: &Row, sort: Column) -> SharedString {
    match sort {
        Column::Name => match row.name.chars().next() {
            Some(first) if first.is_ascii_digit() => "0 \u{2013} 9".into(),
            Some(first) => SharedString::from(first.to_uppercase().to_string()),
            None => "\u{2014}".into(),
        },
        Column::Kind => plural(&row.kind),
        Column::Size if row.face == Face::Folder => "Folders".into(),
        Column::Size => match row.bytes {
            0 => "Zero KB".into(),
            bytes if bytes < 1_000_000 => "Under 1 MB".into(),
            bytes if bytes < 10_000_000 => "1 MB to 10 MB".into(),
            bytes if bytes < 100_000_000 => "10 MB to 100 MB".into(),
            bytes if bytes < 1_000_000_000 => "100 MB to 1 GB".into(),
            _ => "Over 1 GB".into(),
        },
        Column::Modified => match days_ago(row.modified_at) {
            0 => "Today".into(),
            1 => "Yesterday".into(),
            2..=7 => "Previous 7 Days".into(),
            8..=30 => "Previous 30 Days".into(),
            _ => month_string(row.modified_at),
        },
    }
}

fn plural(kind: &str) -> SharedString {
    match kind.ends_with('s') {
        true => SharedString::from(kind.to_string()),
        false => SharedString::from(format!("{kind}s")),
    }
}

fn days_ago(seconds: f64) -> i64 {
    let now = NSDate::date().timeIntervalSinceReferenceDate();
    let calendar = NSCalendar::currentCalendar();
    let then = NSDate::dateWithTimeIntervalSinceReferenceDate(seconds);
    let start_of = |date: &NSDate| {
        calendar
            .startOfDayForDate(date)
            .timeIntervalSinceReferenceDate()
    };
    let gap = start_of(&NSDate::dateWithTimeIntervalSinceReferenceDate(now)) - start_of(&then);
    (gap / 86_400.0).round() as i64
}

fn month_string(seconds: f64) -> SharedString {
    let formatter = NSDateFormatter::new();
    formatter.setDateFormat(Some(&NSString::from_str("LLLL yyyy")));
    SharedString::from(
        formatter
            .stringFromDate(&NSDate::dateWithTimeIntervalSinceReferenceDate(seconds))
            .to_string(),
    )
}

pub(crate) fn day_string(seconds: f64) -> String {
    let formatter = NSDateFormatter::new();
    formatter.setDateStyle(NSDateFormatterStyle::MediumStyle);
    formatter.setTimeStyle(NSDateFormatterStyle::NoStyle);
    formatter
        .stringFromDate(&NSDate::dateWithTimeIntervalSinceReferenceDate(seconds))
        .to_string()
}

pub(crate) fn byte_string(bytes: u64) -> String {
    thread_local! {
        static FORMATTER: Retained<NSByteCountFormatter> = {
            let formatter = NSByteCountFormatter::new();
            formatter.setCountStyle(NSByteCountFormatterCountStyle::File);
            formatter
        };
    }
    FORMATTER.with(|formatter| formatter.stringFromByteCount(bytes as i64).to_string())
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
    pub(crate) dimensions: Option<SharedString>,
    pub(crate) duration: Option<SharedString>,
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
        dimensions: None,
        duration: None,
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

/// The name of the folder a search result's key sits in, the way Finder's search results show a
/// path in place of Kind.
pub(crate) fn enclosing_name(key: &str) -> SharedString {
    match Path::new(key).parent().and_then(Path::file_name) {
        Some(name) => name.to_string_lossy().into_owned().into(),
        None => "/".into(),
    }
}

pub(crate) struct Folder {
    pub(crate) title: &'static str,
    pub(crate) parent: Option<&'static str>,
    /// Finder's path bar: the volume's name, then every folder down to this one.
    pub(crate) crumbs: Vec<&'static str>,
    pub(crate) rows: Vec<Arc<Row>>,
    /// Rows are still arriving from the disk.
    pub(crate) loading: bool,
}

/// The folder at `id`, named and placed the way Finder names and places it, with no rows yet.
pub(crate) fn folder_of(id: &'static str) -> Folder {
    let (mount, volume) = volume_of(id);
    let rest = id.strip_prefix(mount.trim_end_matches('/')).unwrap_or("");
    let mut crumbs = vec![volume];
    crumbs.extend(rest.split('/').filter(|part| !part.is_empty()).map(intern));
    Folder {
        title: crumbs.last().copied().unwrap_or(volume),
        parent: Path::new(id).parent().map(intern_path),
        crumbs,
        rows: Vec::new(),
        loading: true,
    }
}

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

/// The listings the demo has read, newest last, with the misses a render wants read next.
#[derive(Default)]
pub(crate) struct Folders {
    cache: HashMap<&'static str, Folder>,
    order: Vec<&'static str>,
    reading: HashSet<&'static str>,
    wanted: RefCell<Vec<&'static str>>,
}

impl Folders {
    /// The cached listing. A miss asks for a read at the next pump and draws nothing until it lands.
    pub(crate) fn get(&self, id: &'static str) -> Option<&Folder> {
        if let Some(folder) = self.cache.get(id) {
            return Some(folder);
        }
        if id.starts_with('/') && !self.reading.contains(id) {
            let mut wanted = self.wanted.borrow_mut();
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
        None
    }

    pub(crate) fn get_mut(&mut self, id: &'static str) -> Option<&mut Folder> {
        self.cache.get_mut(id)
    }

    /// Whether the listing is in memory, without asking for it.
    pub(crate) fn has(&self, id: &'static str) -> bool {
        self.cache.contains_key(id)
    }

    pub(crate) fn len(&self) -> usize {
        self.cache.len()
    }

    pub(crate) fn take_wanted(&mut self) -> Vec<&'static str> {
        self.wanted.take()
    }

    /// Claims a read of `id`, unless one is already running.
    pub(crate) fn start(&mut self, id: &'static str) -> bool {
        self.reading.insert(id)
    }

    pub(crate) fn finish(&mut self, id: &'static str) {
        self.reading.remove(id);
        if let Some(folder) = self.cache.get_mut(id) {
            folder.loading = false;
        }
    }

    /// Rows from a read: the first batch stands in for whatever was there, the rest add to it.
    pub(crate) fn deliver(&mut self, id: &'static str, rows: Vec<Arc<Row>>, replace: bool) {
        let folder = self.cache.entry(id).or_insert_with(|| {
            self.order.push(id);
            folder_of(id)
        });
        if replace {
            folder.rows.clear();
        }
        folder.rows.extend(rows);
        while self.order.len() > CACHED {
            let oldest = self.order.remove(0);
            if oldest != id && !self.reading.contains(oldest) {
                self.cache.remove(oldest);
            }
        }
    }

    pub(crate) fn cached(&self) -> Vec<&'static str> {
        self.order.clone()
    }
}
