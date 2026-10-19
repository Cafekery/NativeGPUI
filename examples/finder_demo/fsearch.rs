use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::fsearch::{Engine, Found, Grep, Options, Query, Totals, walk};
use finder_fs::entry::{Entry, EntryKind};
use finder_fs::privacy::off_limits;

use crate::data::Facts;

const READY_POLL: Duration = Duration::from_millis(50);
/// fseventsd answers a history query only after several milliseconds.
const PROBE_WAIT: Duration = Duration::from_millis(25);

pub(crate) fn start() {
    engine();
}

fn engine() -> Option<&'static Engine> {
    static ENGINE: OnceLock<Option<Engine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            if std::env::var_os("NG_NO_FSEARCH").is_some() {
                return None;
            }
            let home = crate::home();
            let home_str = home.to_string_lossy().into_owned();
            // Opening a consent-gated folder this app hasn't been let into puts up a privacy prompt.
            let skip = ::fsearch::gated(&home_str)
                .into_iter()
                .map(|p| PathBuf::from(OsStr::from_bytes(&p)))
                .filter(|p| !let_in(p, &home))
                .collect();
            Engine::start(Options {
                dir: crate::store().join("fsearch"),
                home: home_str,
                skip: Some(skip),
            })
            .ok()
        })
        .as_ref()
}

pub(crate) fn wait_ready() -> bool {
    wait_ready_unless(|| false)
}

pub(crate) fn wait_ready_unless(gone: impl Fn() -> bool) -> bool {
    let Some(engine) = engine() else {
        return false;
    };
    while !engine.status().ready {
        if gone() {
            return false;
        }
        std::thread::sleep(READY_POLL);
    }
    true
}

pub(crate) fn loading() -> bool {
    engine().is_some_and(|engine| !engine.status().ready)
}

pub(crate) fn totals(path: &Path) -> Option<Totals> {
    engine()?.totals(path)
}

pub(crate) fn hardlinks(path: &Path) -> Vec<PathBuf> {
    engine().map_or_else(Vec::new, |engine| engine.hardlinks(path))
}

pub(crate) fn facts(path: &Path) -> Facts {
    let Some(engine) = engine() else {
        return (None, None);
    };
    if let Some(held) = engine.totals(path) {
        return (Some(held.bytes), Some(held.visible as usize));
    }
    let real = std::fs::canonicalize(path).ok();
    let held = real.and_then(|real| engine.totals(&real));
    (held.map(|held| held.bytes), None)
}

/// Asked mid-frame, so never of the disk: a share that stopped answering would stall it.
pub(crate) fn visible(path: &Path) -> Option<usize> {
    Some(totals(path)?.visible as usize)
}

pub(crate) fn folders_in(dir: &Path) -> HashMap<String, Facts> {
    let Some(engine) = engine() else {
        return HashMap::new();
    };
    let found = engine
        .folder_totals(dir)
        .map(|found| (found, true))
        .or_else(|| {
            let real = std::fs::canonicalize(dir).ok()?;
            Some((engine.folder_totals(&real)?, false))
        });
    let Some((found, exact)) = found else {
        return HashMap::new();
    };
    found
        .into_iter()
        .filter_map(|(path, held)| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            let seen = exact.then_some(held.visible as usize);
            Some((name, (Some(held.bytes), seen)))
        })
        .collect()
}

pub(crate) fn modified(dir: &Path) -> Option<SystemTime> {
    Some(time_of(engine()?.entry(dir)?.mtime))
}

pub(crate) fn children(dir: &Path) -> Option<Vec<Entry>> {
    let held = engine()?.children(dir)?;
    Some(held.into_iter().map(entry_of).collect())
}

pub(crate) fn current_children(dir: &Path) -> Option<Vec<Entry>> {
    let engine = engine()?;
    let real = std::fs::canonicalize(dir).ok()?;
    let applied = engine.status().event_id;
    let held = engine.children(dir)?;
    let changed = finder_fs::watch::changed_since(&real, applied, PROBE_WAIT);
    (!changed).then(|| held.into_iter().map(entry_of).collect())
}

fn entry_of(found: Found) -> Entry {
    let name = (found.path.file_name().unwrap_or_default())
        .to_string_lossy()
        .into_owned();
    let kind = match found.kind & 3 {
        walk::KIND_FILE => EntryKind::File,
        walk::KIND_DIR => EntryKind::Directory,
        walk::KIND_LINK => EntryKind::Symlink,
        _ => EntryKind::Other,
    };
    let hidden = name.starts_with('.') || found.kind & walk::FLAG_HIDDEN != 0;
    let package = kind == EntryKind::Directory && finder_fs::kind::is_package_name(&name);
    Entry {
        name,
        kind,
        size: found.size,
        modified: time_of(found.mtime),
        created: UNIX_EPOCH,
        added: None,
        hidden,
        package,
        ext_hidden: false,
        alias: false,
        custom_icon: false,
        locked: false,
        dataless: false,
        readable: true,
        executable: false,
        file_id: 0,
        children: None,
    }
}

fn time_of(seconds: u32) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds.into())
}

pub(crate) fn note_changed(dirs: &[PathBuf]) {
    if let Some(engine) = engine() {
        engine.refresh(dirs);
    }
}

pub(crate) fn extensions(limit: usize) -> Vec<PathBuf> {
    engine().map_or_else(Vec::new, |engine| engine.extensions(limit))
}

/// The index never walks into other volumes.
pub(crate) fn indexes(dir: &Path) -> bool {
    !dir.starts_with("/Volumes")
        && !(crate::data::network_roots().iter()).any(|root| dir.starts_with(root))
}

/// `None` while fsearch can't answer: still loading, or a query it doesn't parse.
pub(crate) fn search(query: &str, under: &[PathBuf], limit: usize) -> Option<Vec<PathBuf>> {
    let engine = engine()?;
    let mut found = Vec::new();
    for root in under
        .iter()
        .map(Some)
        .chain(under.is_empty().then_some(None))
    {
        let mut q = Query::parse(query, engine.home()).ok()?;
        q.limit = limit;
        if let Some(root) = root {
            q.filter("in", &root.to_string_lossy(), engine.home())
                .ok()?;
        }
        match q.grep.take() {
            Some(pattern) => {
                let grep = Grep::new(&pattern, q.grep_mode).ok()?;
                let (r, _) = engine.grep(&q, &grep).ok()?;
                found.extend(
                    r.files
                        .into_iter()
                        .map(|f| PathBuf::from(OsStr::from_bytes(&f.path))),
                );
            }
            None => found.extend(engine.search(&q).ok()?.into_iter().map(|f| f.path)),
        }
    }
    Some(reachable_only(found))
}

pub(crate) fn folders_named(text: &str, limit: usize) -> Vec<PathBuf> {
    let ranked = || {
        let engine = engine()?;
        let mut query = Query::parse(text, engine.home()).ok()?;
        query.filter("kind", "dir", engine.home()).ok()?;
        query.limit = limit.saturating_mul(4).max(64);
        engine.search(&query).ok()
    };
    let folders = (ranked().unwrap_or_default().into_iter())
        .map(|found| found.path)
        .filter(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            !finder_fs::kind::is_package_name(&name)
        })
        .collect();
    let mut folders = reachable_only(folders);
    folders.truncate(limit);
    folders
}

pub(crate) fn reachable_only(found: Vec<PathBuf>) -> Vec<PathBuf> {
    let home = crate::home();
    let mut folders = HashMap::new();
    found
        .into_iter()
        .filter(|path| reachable(path, &home, &mut folders))
        .collect()
}

fn reachable(path: &Path, home: &Path, folders: &mut HashMap<PathBuf, bool>) -> bool {
    (!off_limits(path, home) || let_in(path, home))
        && path
            .parent()
            .is_none_or(|dir| folder_reachable(dir, home, folders))
}

fn folder_reachable(dir: &Path, home: &Path, folders: &mut HashMap<PathBuf, bool>) -> bool {
    if let Some(&ok) = folders.get(dir) {
        return ok;
    }
    let ok = reachable(dir, home, folders);
    folders.insert(dir.to_path_buf(), ok);
    ok
}

fn let_in(gate: &Path, home: &Path) -> bool {
    let service = match gate.strip_prefix(home).ok().and_then(Path::to_str) {
        Some("Desktop") => "kTCCServiceSystemPolicyDesktopFolder",
        Some("Documents") => "kTCCServiceSystemPolicyDocumentsFolder",
        Some("Downloads") => "kTCCServiceSystemPolicyDownloadsFolder",
        _ => return false,
    };
    granted(service)
}

#[cfg(feature = "private-api")]
fn granted(service: &'static str) -> bool {
    use parking_lot::Mutex;
    use std::time::Instant;
    static SEEN: Mutex<Vec<(&str, bool, Instant)>> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock();
    if let Some(&(_, granted, at)) = seen.iter().find(|(known, ..)| *known == service) {
        if granted || at.elapsed() < Duration::from_secs(3) {
            return granted;
        }
    }
    let granted = preflight(service);
    seen.retain(|(known, ..)| *known != service);
    seen.push((service, granted, Instant::now()));
    granted
}

/// Without TCC's private preflight there is no asking without a prompt.
#[cfg(not(feature = "private-api"))]
fn granted(_: &'static str) -> bool {
    false
}

#[cfg(feature = "private-api")]
fn preflight(service: &str) -> bool {
    use objc2::rc::Retained;
    use objc2_foundation::NSString;
    use std::ffi::{c_char, c_int, c_void};
    unsafe extern "C" {
        fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }
    type Preflight = unsafe extern "C" fn(*const NSString, *const c_void) -> c_int;
    const RTLD_LAZY: c_int = 1;
    let tcc = c"/System/Library/PrivateFrameworks/TCC.framework/TCC";
    let tcc = unsafe { dlopen(tcc.as_ptr(), RTLD_LAZY) };
    if tcc.is_null() {
        return false;
    }
    let symbol = unsafe { dlsym(tcc, c"TCCAccessPreflight".as_ptr()) };
    if symbol.is_null() {
        return false;
    }
    let preflight: Preflight = unsafe { std::mem::transmute(symbol) };
    let service = NSString::from_str(service);
    unsafe { preflight(Retained::as_ptr(&service), std::ptr::null()) == 0 }
}
