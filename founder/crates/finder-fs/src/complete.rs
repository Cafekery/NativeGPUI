use std::borrow::Cow;
use std::ffi::{CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::entry::Entry;
use crate::list::list_dir;
use crate::volumes::is_local;

const TIMEOUT: Duration = Duration::from_millis(50);

pub struct Completion {
    pub path: PathBuf,
    pub display: String,
}

pub fn complete(
    typed: &str,
    named: impl FnOnce(&str, usize) -> Vec<PathBuf>,
    limit: usize,
) -> Vec<Completion> {
    if typed.is_empty() || limit == 0 {
        return Vec::new();
    }
    let expanded = expand_tilde(typed);
    if !expanded.contains('/') {
        let mut folders = named(&expanded, limit);
        folders.truncate(limit);
        return folders
            .into_iter()
            .map(|path| to_completion(path, true))
            .collect();
    }
    let (parent, prefix) = match expanded.strip_suffix('/') {
        Some(trimmed) => (trimmed, ""),
        None => expanded.rsplit_once('/').unwrap(),
    };
    let parent = Path::new(if parent.is_empty() { "/" } else { parent });
    in_dir(parent, prefix, limit)
}

fn in_dir(dir: &Path, prefix: &str, limit: usize) -> Vec<Completion> {
    let Some(entries) = list_dir_with_timeout(dir) else {
        return Vec::new();
    };
    let needle = prefix.to_lowercase();
    let hidden = needle.starts_with('.');
    let mut prefix_hits: Vec<(String, &Entry)> = Vec::new();
    let mut fuzzy_hits: Vec<(String, &Entry)> = Vec::new();
    let mut file_hit = None;
    for entry in entries
        .iter()
        .filter(|entry| hidden || !entry.name.starts_with('.'))
        .filter(|entry| entry.readable || !entry.is_dir())
    {
        let name = entry.name.to_lowercase();
        if entry.is_dir() {
            if prefix.is_empty() || name.starts_with(&needle) {
                prefix_hits.push((name, entry));
            } else if subsequence(&needle, &name) {
                fuzzy_hits.push((name, entry));
            }
        } else if !prefix.is_empty() && file_hit.is_none() && name == needle {
            file_hit = Some(entry);
        }
    }
    let rank = |(name, entry): &(String, &Entry)| {
        let len = if prefix.is_empty() {
            0
        } else {
            entry.name.len()
        };
        (*name != needle, len)
    };
    prefix_hits.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.0.cmp(&b.0)));
    fuzzy_hits.sort_by(|a, b| (a.1.name.len(), &a.0).cmp(&(b.1.name.len(), &b.0)));
    prefix_hits
        .into_iter()
        .map(|(_, entry)| entry)
        .chain(file_hit)
        .chain(fuzzy_hits.into_iter().map(|(_, entry)| entry))
        .take(limit)
        .map(|entry| to_completion(dir.join(&entry.name), entry.is_dir()))
        .collect()
}

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

fn subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    needle.chars().all(|c| chars.by_ref().any(|h| h == c))
}

fn to_completion(path: PathBuf, is_dir: bool) -> Completion {
    let home = crate::home();
    let mut display = match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    };
    if is_dir && !display.ends_with('/') {
        display.push('/');
    }
    Completion { path, display }
}

fn expand_tilde(typed: &str) -> Cow<'_, str> {
    let Some(rest) = typed.strip_prefix('~') else {
        return Cow::Borrowed(typed);
    };
    let (user, tail) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let home = if user.is_empty() {
        crate::home()
    } else {
        user_home(user)
    };
    match home {
        Some(home) => Cow::Owned(format!("{}{tail}", home.display())),
        None => Cow::Borrowed(typed),
    }
}

fn user_home(user: &str) -> Option<PathBuf> {
    let name = CString::new(user).ok()?;
    let pw = unsafe { libc::getpwnam(name.as_ptr()) };
    if pw.is_null() {
        return None;
    }
    let dir = unsafe { CStr::from_ptr((*pw).pw_dir) };
    Some(PathBuf::from(OsStr::from_bytes(dir.to_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
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

    fn unindexed(typed: &str, limit: usize) -> Vec<Completion> {
        complete(typed, |_, _| Vec::new(), limit)
    }

    fn under(root: &Path, typed: &str, limit: usize) -> Vec<Completion> {
        unindexed(&format!("{}/{typed}", root.display()), limit)
    }

    fn shown(root: &Path, rest: &str) -> String {
        format!("{}/{rest}", root.display())
    }

    fn warm() {
        for _ in 0..5 {
            unindexed("/", 1);
        }
    }

    fn names(out: &[Completion]) -> Vec<String> {
        out.iter().map(|c| c.display.clone()).collect()
    }

    #[test]
    fn prefix_beats_fuzzy_case_insensitively() {
        let root = tree("prefix");
        assert_eq!(
            names(&under(&root, "dev", 10)),
            [shown(&root, "DevOps/"), shown(&root, "Developer/")]
        );
    }

    #[test]
    fn hidden_folders_wait_for_a_typed_dot() {
        let root = tree("hidden");
        fs::create_dir_all(root.join(".config")).unwrap();
        let listed = names(&under(&root, "", 10));
        assert!(!listed.iter().any(|name| name.contains("/.config")));
        assert_eq!(names(&under(&root, ".c", 10)), [shown(&root, ".config/")]);
    }

    #[test]
    fn a_folder_the_user_cannot_list_is_never_offered() {
        let root = tree("locked");
        let locked = root.join("Devlocked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let offered = names(&under(&root, "Dev", 10));
        let exact = under(&root, "Devlocked", 10);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            offered,
            [shown(&root, "DevOps/"), shown(&root, "Developer/")]
        );
        assert!(exact.is_empty());
    }

    #[test]
    fn fuzzy_subsequence_matches_out_of_order_letters() {
        let root = tree("fuzzy");
        assert_eq!(
            names(&under(&root, "Developer/ntvgpui", 10)),
            [shown(&root, "Developer/NativeGPUI/")]
        );
    }

    #[test]
    fn exact_file_included_only_on_exact_match() {
        let root = tree("file");
        assert!(under(&root, "Developer.t", 10).is_empty());
        let out = under(&root, "Developer.txt", 10);
        assert_eq!(out[0].display, shown(&root, "Developer.txt"));
    }

    #[test]
    fn trailing_slash_lists_children_and_respects_limit() {
        let root = tree("slash");
        assert_eq!(under(&root, "", 10).len(), 3);
        assert_eq!(under(&root, "", 1).len(), 1);
    }

    #[test]
    fn tilde_expands_to_home_and_user() {
        warm();
        let home = crate::home().unwrap();
        assert!(!unindexed("~/", 100).is_empty());
        let user = home.file_name().unwrap().to_str().unwrap();
        let out = unindexed(&format!("~{user}"), 100);
        assert_eq!(out[0].display, "~/");
    }

    #[test]
    fn no_index_means_no_bare_name_search() {
        assert!(unindexed("nativegpui", 10).is_empty());
    }

    #[test]
    fn measured_on_this_machine() {
        warm();
        let root = tree("measured");
        let t0 = Instant::now();
        let a = under(&root, "Dev", 10);
        let da = t0.elapsed();
        let t1 = Instant::now();
        let b = unindexed("~/", 10);
        let db = t1.elapsed();
        assert!(a.iter().any(|c| c.display.contains("Developer")));
        assert!(!b.is_empty());
        assert!(da < Duration::from_millis(40), "{da:?}");
        assert!(db < Duration::from_millis(40), "{db:?}");
    }
}
