//! Search through fsearch, linked in: a whole-disk name index answering in
//! about a millisecond, kept live from FSEvents in this process, and its
//! query language (`ext:rs`, `kind:dir`, `grep:text`, ...) typed straight
//! into the search field.

use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ::fsearch::{Engine, Grep, Options, Query};
use finder_fs::index::off_limits;

fn enabled() -> bool {
    std::env::var_os("NG_NO_FSEARCH").is_none()
}

/// Start the engine at launch, so the index is loaded and following changes
/// before the first keystroke. Its index lives in the app's store; a second
/// Found'er follows the first one's instead of building its own.
pub(crate) fn start() {
    engine();
}

fn engine() -> Option<&'static Engine> {
    static ENGINE: OnceLock<Option<Engine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            if !enabled() {
                return None;
            }
            let home = crate::home();
            let home_str = home.to_string_lossy().into_owned();
            // Never open a consent-gated folder this app hasn't been let into:
            // that puts a privacy prompt on screen.
            let skip = ::fsearch::gated(&home_str)
                .into_iter()
                .map(|p| PathBuf::from(std::ffi::OsStr::from_bytes(&p)))
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

/// Paths matching `query` under each of `under` (everywhere if empty), best
/// first, or `None` while fsearch can't answer (still loading, a query it
/// doesn't parse) or `NG_NO_FSEARCH` is set.
pub(crate) fn search(query: &str, under: &[PathBuf], limit: usize) -> Option<Vec<PathBuf>> {
    let engine = engine()?;
    let home = crate::home();
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
        let paths: Vec<PathBuf> = match q.grep.take() {
            Some(pattern) => {
                let grep = Grep::new(&pattern, q.grep_mode).ok()?;
                let (r, _) = engine.grep(&q, &grep).ok()?;
                r.files
                    .into_iter()
                    .map(|f| PathBuf::from(std::ffi::OsStr::from_bytes(&f.path)))
                    .collect()
            }
            None => engine
                .search(&q)
                .ok()?
                .into_iter()
                .map(|f| f.path)
                .collect(),
        };
        let mut folders = HashMap::new();
        found.extend(
            paths
                .into_iter()
                .filter(|path| reachable(path, &home, &mut folders)),
        );
    }
    Some(found)
}

/// The paths of `found` that are shown, by the rule the hits follow.
pub(crate) fn reachable_only(found: Vec<PathBuf>) -> Vec<PathBuf> {
    let home = crate::home();
    let mut folders = HashMap::new();
    found
        .into_iter()
        .filter(|path| reachable(path, &home, &mut folders))
        .collect()
}

/// Reading inside a folder macOS guards puts its consent prompt on screen, so
/// a result there is shown only once the user has already let Found'er in.
/// Hits share folders, so each folder's answer is kept in `folders`.
fn reachable(path: &Path, home: &Path, folders: &mut HashMap<PathBuf, bool>) -> bool {
    (!off_limits(path, home) || let_in(path, home))
        && path
            .parent()
            .is_none_or(|dir| folder_ok(dir, home, folders))
}

fn folder_ok(dir: &Path, home: &Path, folders: &mut HashMap<PathBuf, bool>) -> bool {
    if let Some(&ok) = folders.get(dir) {
        return ok;
    }
    let ok = (!off_limits(dir, home) || let_in(dir, home))
        && dir.parent().is_none_or(|up| folder_ok(up, home, folders));
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

/// Asking tccd costs about 4 ms, so a grant is kept for the session and
/// anything short of one is asked again at most every few seconds.
#[cfg(feature = "private-api")]
fn granted(service: &'static str) -> bool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static SEEN: Mutex<Vec<(&str, bool, Instant)>> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap();
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

/// TCC's answer for this app, which never puts a prompt up: 0 is granted.
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
