//! Path completion for the Go to Folder sheet: instant filesystem completion, falling back to
//! the whole-index fuzzy name search once the typed text has no `/` left to anchor a directory.

use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::entry::Entry;
use crate::index::Index;
use crate::list::list_dir;

const TIMEOUT: Duration = Duration::from_millis(50);

pub struct Completion {
    pub path: PathBuf,
    pub display: String,
}

/// Completions for what the user has typed so far, best first, at most `limit`.
/// `index` may be `None` (no index yet): then only direct children of the typed directory are
/// offered.
pub fn complete(typed: &str, index: Option<&Index>, limit: usize) -> Vec<Completion> {
    if typed.is_empty() || limit == 0 {
        return Vec::new();
    }
    let expanded = expand_tilde(typed);
    if !expanded.contains('/') {
        return by_name(&expanded, index, limit);
    }
    let (parent, prefix) = match expanded.strip_suffix('/') {
        Some(trimmed) => (trimmed, ""),
        None => expanded.rsplit_once('/').unwrap(),
    };
    let parent = if parent.is_empty() {
        Path::new("/")
    } else {
        Path::new(parent)
    };
    in_dir(parent, prefix, index, limit)
}

fn by_name(typed: &str, index: Option<&Index>, limit: usize) -> Vec<Completion> {
    let Some(index) = index else {
        return Vec::new();
    };
    let mut hits = index.search(typed, None, limit.saturating_mul(4).max(64));
    hits.retain(|hit| hit.entry.is_dir());
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.path.as_os_str().len().cmp(&b.path.as_os_str().len()))
    });
    hits.truncate(limit);
    hits.into_iter()
        .map(|hit| to_completion(hit.path, true))
        .collect()
}

fn in_dir(dir: &Path, prefix: &str, index: Option<&Index>, limit: usize) -> Vec<Completion> {
    let Some(entries) = list_dir_with_timeout(dir).or_else(|| index.and_then(|i| i.children(dir)))
    else {
        return Vec::new();
    };
    let needle = prefix.to_lowercase();
    let mut prefix_hits: Vec<&Entry> = Vec::new();
    let mut fuzzy_hits: Vec<&Entry> = Vec::new();
    let mut file_hit = None;
    for entry in &entries {
        let name = entry.name.to_lowercase();
        if entry.is_dir() {
            if prefix.is_empty() || name.starts_with(&needle) {
                prefix_hits.push(entry);
            } else if subsequence(&needle, &name) {
                fuzzy_hits.push(entry);
            }
        } else if !prefix.is_empty() && file_hit.is_none() && name == needle {
            file_hit = Some(entry);
        }
    }
    prefix_hits.sort_by_key(|e| {
        let name = e.name.to_lowercase();
        (name != needle, e.name.len(), name)
    });
    fuzzy_hits.sort_by_key(|e| (e.name.len(), e.name.to_lowercase()));
    prefix_hits
        .into_iter()
        .chain(file_hit)
        .chain(fuzzy_hits)
        .take(limit)
        .map(|entry| to_completion(dir.join(&entry.name), entry.is_dir()))
        .collect()
}

/// `list_dir`; on a network mount it is abandoned (not cancelled: the thread finishes on its own)
/// if the volume has not answered within `TIMEOUT`.
fn list_dir_with_timeout(dir: &Path) -> Option<Vec<Entry>> {
    if is_local(dir) {
        return list_dir(dir).ok();
    }
    let (tx, rx) = mpsc::channel();
    let dir = dir.to_path_buf();
    thread::spawn(move || {
        let _ = tx.send(list_dir(&dir));
    });
    rx.recv_timeout(TIMEOUT).ok()?.ok()
}

fn is_local(dir: &Path) -> bool {
    let Ok(path) = CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `stat` is a valid out-pointer for the call.
    if unsafe { libc::statfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return false;
    }
    // SAFETY: statfs returned 0, so the struct is initialised.
    let stat = unsafe { stat.assume_init() };
    stat.f_flags & libc::MNT_LOCAL as u32 != 0
}

fn subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    needle.chars().all(|c| chars.by_ref().any(|h| h == c))
}

fn to_completion(path: PathBuf, is_dir: bool) -> Completion {
    let display = display_string(&path, is_dir);
    Completion { path, display }
}

fn display_string(path: &Path, is_dir: bool) -> String {
    let mut shown = home_dir()
        .and_then(|home| {
            if path == home {
                Some("~".to_string())
            } else {
                path.strip_prefix(&home)
                    .ok()
                    .map(|rest| format!("~/{}", rest.display()))
            }
        })
        .unwrap_or_else(|| path.display().to_string());
    if is_dir && !shown.ends_with('/') {
        shown.push('/');
    }
    shown
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// `~` to the caller's home, `~user` to that user's, both followed by the rest of the string
/// unchanged; anything else (including an unknown user) passes through untouched.
fn expand_tilde(typed: &str) -> String {
    let Some(rest) = typed.strip_prefix('~') else {
        return typed.to_string();
    };
    let (user, tail) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let home = if user.is_empty() {
        home_dir()
    } else {
        user_home(user)
    };
    match home {
        Some(home) => format!("{}{}", home.display(), tail),
        None => typed.to_string(),
    }
}

fn user_home(user: &str) -> Option<PathBuf> {
    let name = CString::new(user).ok()?;
    let pw = unsafe { libc::getpwnam(name.as_ptr()) };
    if pw.is_null() {
        return None;
    }
    let dir = unsafe { CStr::from_ptr((*pw).pw_dir) };
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{Index, Progress};
    use std::fs;
    use std::sync::Arc;
    use std::time::Instant;

    fn tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("finder-fs-complete-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Developer/NativeGPUI")).unwrap();
        fs::create_dir_all(root.join("DevOps")).unwrap();
        fs::create_dir_all(root.join("Documents")).unwrap();
        fs::write(root.join("Developer.txt"), b"x").unwrap();
        root
    }

    /// The first calls in a process pay for the OS spinning up its thread machinery.
    fn warm() {
        for _ in 0..5 {
            complete("/", None, 1);
        }
    }

    fn crawled(root: &Path) -> Arc<Index> {
        let index = Index::open(&root.join("index.bin"));
        let (tx, rx) = mpsc::channel();
        index.crawl(vec![root.to_path_buf()], move |p: Progress| {
            if p.done {
                let _ = tx.send(());
            }
        });
        rx.recv_timeout(Duration::from_secs(60)).unwrap();
        index
    }

    fn names(out: &[Completion]) -> Vec<String> {
        out.iter().map(|c| c.display.clone()).collect()
    }

    #[test]
    fn prefix_beats_fuzzy_case_insensitively() {
        let root = tree("prefix");
        let out = complete(&format!("{}/dev", root.display()), None, 10);
        assert_eq!(
            names(&out),
            [
                format!("{}/DevOps/", root.display()),
                format!("{}/Developer/", root.display()),
            ]
        );
    }

    #[test]
    fn fuzzy_subsequence_matches_out_of_order_letters() {
        let root = tree("fuzzy");
        let out = complete(&format!("{}/Developer/ntvgpui", root.display()), None, 10);
        assert_eq!(
            names(&out),
            [format!("{}/Developer/NativeGPUI/", root.display())]
        );
    }

    #[test]
    fn exact_file_included_only_on_exact_match() {
        let root = tree("file");
        assert!(complete(&format!("{}/Developer.t", root.display()), None, 10).is_empty());
        let out = complete(&format!("{}/Developer.txt", root.display()), None, 10);
        assert_eq!(out[0].display, format!("{}/Developer.txt", root.display()));
    }

    #[test]
    fn trailing_slash_lists_children_and_respects_limit() {
        let root = tree("slash");
        let out = complete(&format!("{}/", root.display()), None, 10);
        assert_eq!(out.len(), 3);
        assert_eq!(complete(&format!("{}/", root.display()), None, 1).len(), 1);
    }

    #[test]
    fn tilde_expands_to_home_and_user() {
        warm();
        let home = home_dir().unwrap();
        let out = complete("~/", None, 100);
        assert!(!out.is_empty());
        let user = home.file_name().unwrap().to_str().unwrap();
        let out = complete(&format!("~{user}"), None, 100);
        assert_eq!(out[0].display, "~/");
    }

    #[test]
    fn no_index_means_no_bare_name_search() {
        assert!(complete("nativegpui", None, 10).is_empty());
    }

    #[test]
    fn bare_name_search_uses_the_index_and_ranks_directories_first() {
        let root = tree("index-search");
        let index = crawled(&root);
        let out = complete("nativegpui", Some(&index), 10);
        assert_eq!(
            out[0].path,
            root.join("Developer/NativeGPUI").canonicalize().unwrap()
        );
    }

    #[test]
    fn falls_back_to_the_index_when_the_directory_is_gone() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tree("fallback");
        let index = crawled(&root);
        let sealed = root.join("Developer");
        fs::set_permissions(&sealed, fs::Permissions::from_mode(0o000)).unwrap();
        let out = complete(&format!("{}/Nat", sealed.display()), Some(&index), 10);
        fs::set_permissions(&sealed, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(out[0].display, format!("{}/NativeGPUI/", sealed.display()));
    }

    #[test]
    fn measured_on_this_machine() {
        warm();
        let root = tree("measured");
        let t0 = Instant::now();
        let a = complete(&format!("{}/Dev", root.display()), None, 10);
        let da = t0.elapsed();
        let t1 = Instant::now();
        let b = complete("~/", None, 10);
        let db = t1.elapsed();
        assert!(a.iter().any(|c| c.display.contains("Developer")));
        assert!(!b.is_empty());
        // Loose: the assertion only catches a fall back to the index, which costs 50ms.
        assert!(da < Duration::from_millis(40), "{da:?}");
        assert!(db < Duration::from_millis(40), "{db:?}");
    }
}
