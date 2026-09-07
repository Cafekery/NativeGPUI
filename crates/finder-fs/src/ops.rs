//! File operations with Finder's semantics: copies and moves through NSFileManager (packages,
//! metadata and permissions intact), Trash with Put Back, and an undo journal.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSDictionary, NSError, NSFileAttributeKey, NSFileManager, NSFileNoSuchFileError,
    NSFileReadNoPermissionError, NSFileReadNoSuchFileError, NSFileWriteFileExistsError,
    NSFileWriteNoPermissionError, NSString, NSURL,
};

/// The most bytes a name can carry on the volumes Finder writes to.
const NAME_MAX: usize = 255;

#[derive(Clone, Debug)]
pub enum Op {
    Move {
        items: Vec<PathBuf>,
        into: PathBuf,
    },
    Copy {
        items: Vec<PathBuf>,
        into: PathBuf,
    },
    Trash(Vec<PathBuf>),
    DeleteImmediately(Vec<PathBuf>),
    Rename {
        path: PathBuf,
        name: String,
    },
    Duplicate(Vec<PathBuf>),
    NewFolder {
        into: PathBuf,
        name: String,
    },
    NewFile {
        into: PathBuf,
        name: String,
    },
    /// Puts each item back at the path it came from, over whatever stands there now.
    PutBack(Vec<Restore>),
    /// Several operations as one step, in order.
    Batch(Vec<Op>),
}

/// An item and the path it belongs at.
#[derive(Clone, Debug)]
pub struct Restore {
    pub item: PathBuf,
    pub to: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// `name 2`, `name 3`, … like Duplicate.
    KeepBoth,
    Replace,
    /// Folders merge, files inside follow `Replace`.
    Merge,
    Skip,
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub current: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Done {
    /// Where each item ended up, in input order.
    pub results: Vec<PathBuf>,
    pub undo: Option<Op>,
}

pub fn perform(op: Op, conflict: Conflict, mut progress: impl FnMut(Progress)) -> io::Result<Done> {
    run(op, conflict, &mut progress)
}

fn run(op: Op, conflict: Conflict, progress: &mut dyn FnMut(Progress)) -> io::Result<Done> {
    match op {
        Op::Move { items, into } => move_or_copy(items, into, conflict, true, progress),
        Op::Copy { items, into } => move_or_copy(items, into, conflict, false, progress),
        Op::Trash(items) => trash(items, progress),
        Op::DeleteImmediately(items) => delete(items, progress),
        Op::Rename { path, name } => rename(path, name),
        Op::Duplicate(items) => duplicate(items, progress),
        Op::NewFolder { into, name } => new_item(into, name, true),
        Op::NewFile { into, name } => new_item(into, name, false),
        Op::PutBack(items) => put_back(items, progress),
        Op::Batch(ops) => batch(ops, conflict, progress),
    }
}

fn url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

fn path_from(url: &NSURL) -> PathBuf {
    url.path()
        .map(|s| PathBuf::from(s.to_string()))
        .unwrap_or_default()
}

fn io_error(err: Retained<NSError>) -> io::Error {
    let message = err.localizedDescription().to_string();
    let domain = err.domain().to_string();
    let code = err.code();
    let kind = if domain == "NSCocoaErrorDomain" {
        if code == NSFileNoSuchFileError || code == NSFileReadNoSuchFileError {
            io::ErrorKind::NotFound
        } else if code == NSFileReadNoPermissionError || code == NSFileWriteNoPermissionError {
            io::ErrorKind::PermissionDenied
        } else if code == NSFileWriteFileExistsError {
            io::ErrorKind::AlreadyExists
        } else {
            io::ErrorKind::Other
        }
    } else if domain == "NSPOSIXErrorDomain" {
        match code as i32 {
            libc::ENOENT => io::ErrorKind::NotFound,
            libc::EACCES | libc::EPERM => io::ErrorKind::PermissionDenied,
            libc::EEXIST => io::ErrorKind::AlreadyExists,
            _ => io::ErrorKind::Other,
        }
    } else {
        io::ErrorKind::Other
    };
    io::Error::new(kind, message)
}

fn transfer(fm: &NSFileManager, src: &Path, dst: &Path, is_move: bool) -> io::Result<()> {
    let (su, du) = (url(src), url(dst));
    if is_move {
        fm.moveItemAtURL_toURL_error(&su, &du)
    } else {
        fm.copyItemAtURL_toURL_error(&su, &du)
    }
    .map_err(io_error)
}

fn remove_path(fm: &NSFileManager, path: &Path) -> io::Result<()> {
    fm.removeItemAtURL_error(&url(path)).map_err(io_error)
}

/// Finder's name field: one component of the enclosing folder, never a path.
fn check_name(name: &str) -> io::Result<()> {
    let refuse = |advice: &str| {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("The name \u{201c}{name}\u{201d} can't be used.{advice}"),
        ))
    };
    if name.len() > NAME_MAX {
        return refuse(" Try using a name with fewer characters.");
    }
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        return refuse("");
    }
    Ok(())
}

/// Splits `name` the way Finder does: no extension for dot-files or a trailing dot.
fn split_name(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    }
}

fn with_ext(stem: &str, ext: Option<&str>) -> String {
    match ext {
        Some(ext) => format!("{stem}.{ext}"),
        None => stem.to_string(),
    }
}

/// `name 2`, `name 3`, … beside `path`, whichever is free.
fn keep_both_path(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or_else(|| Path::new(""));
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let (stem, ext) = split_name(&name);
    let mut n = 2;
    loop {
        let candidate = dir.join(with_ext(&format!("{stem} {n}"), ext));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// `name copy`, `name copy 2`, … beside `path`, whichever is free.
fn duplicate_path(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or_else(|| Path::new(""));
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let (stem, ext) = split_name(&name);
    let first = dir.join(with_ext(&format!("{stem} copy"), ext));
    if !first.exists() {
        return first;
    }
    let mut n = 2;
    loop {
        let candidate = dir.join(with_ext(&format!("{stem} copy {n}"), ext));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the path contains a NUL"))
}

/// A free sibling of `dest`: a transfer lands here first, so `dest` is never missing.
fn staging_path(dest: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = dest.parent().unwrap_or_else(|| Path::new("."));
    dir.join(format!(
        ".finder-fs-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Swaps two paths in one step, or in two renames on a volume without `RENAME_SWAP`; `staged`
/// holds what stood at `dest` either way.
fn exchange(staged: &Path, dest: &Path) -> io::Result<()> {
    let (from, to) = (c_path(staged)?, c_path(dest)?);
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_SWAP) } == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if !matches!(err.raw_os_error(), Some(libc::ENOTSUP) | Some(libc::EINVAL)) {
        return Err(err);
    }
    let aside = staging_path(dest);
    std::fs::rename(dest, &aside)?;
    if let Err(err) = std::fs::rename(staged, dest) {
        let _ = std::fs::rename(&aside, dest);
        return Err(err);
    }
    std::fs::rename(&aside, staged)
}

fn trash_one(fm: &NSFileManager, path: &Path) -> io::Result<PathBuf> {
    let mut resulting: Option<Retained<NSURL>> = None;
    fm.trashItemAtURL_resultingItemURL_error(&url(path), Some(&mut resulting))
        .map_err(io_error)?;
    Ok(resulting
        .map(|u| path_from(&u))
        .unwrap_or_else(|| path.to_path_buf()))
}

/// Where an item landed, and what it displaced getting there.
struct Placed {
    dest: Option<PathBuf>,
    displaced: Vec<Restore>,
}

impl Placed {
    fn at(dest: PathBuf) -> Placed {
        Placed {
            dest: Some(dest),
            displaced: Vec::new(),
        }
    }
}

/// Lands `src` where `dest` already stands without `dest` ever going missing: the transfer goes
/// to a sibling and the two are exchanged, so a transfer that fails leaves `dest` untouched. What
/// stood there goes to the Trash, and comes back with the undo.
fn replace_at(fm: &NSFileManager, src: &Path, dest: PathBuf, is_move: bool) -> io::Result<Placed> {
    let staged = staging_path(&dest);
    if let Err(err) = transfer(fm, src, &staged, is_move) {
        let _ = remove_path(fm, &staged);
        return Err(err);
    }
    if let Err(err) = exchange(&staged, &dest) {
        let _ = remove_path(fm, &staged);
        return Err(err);
    }
    let displaced = match trash_one(fm, &staged) {
        Ok(item) => vec![Restore {
            item,
            to: dest.clone(),
        }],
        Err(_) => {
            let _ = remove_path(fm, &staged);
            Vec::new()
        }
    };
    Ok(Placed {
        dest: Some(dest),
        displaced,
    })
}

/// Places `src` at `dest`, resolving a conflict at `dest` if one already exists there; no
/// destination when the conflict was skipped and nothing moved.
fn place_one(
    fm: &NSFileManager,
    src: &Path,
    dest: PathBuf,
    conflict: Conflict,
    is_move: bool,
) -> io::Result<Placed> {
    if !dest.exists() {
        transfer(fm, src, &dest, is_move)?;
        return Ok(Placed::at(dest));
    }
    match conflict {
        Conflict::Skip => Ok(Placed {
            dest: None,
            displaced: Vec::new(),
        }),
        Conflict::KeepBoth => {
            let dest = keep_both_path(&dest);
            transfer(fm, src, &dest, is_move)?;
            Ok(Placed::at(dest))
        }
        Conflict::Replace => replace_at(fm, src, dest, is_move),
        Conflict::Merge if src.is_dir() && dest.is_dir() => {
            let mut displaced = Vec::new();
            for entry in std::fs::read_dir(src)? {
                let entry = entry?;
                let child_dest = dest.join(entry.file_name());
                let placed = place_one(fm, &entry.path(), child_dest, Conflict::Merge, is_move)?;
                displaced.extend(placed.displaced);
            }
            if is_move {
                std::fs::remove_dir(src)?;
            }
            Ok(Placed {
                dest: Some(dest),
                displaced,
            })
        }
        Conflict::Merge => replace_at(fm, src, dest, is_move),
    }
}

fn common_parent(items: &[PathBuf]) -> Option<PathBuf> {
    let first = items.first()?.parent()?;
    items
        .iter()
        .all(|p| p.parent() == Some(first))
        .then(|| first.to_path_buf())
}

fn move_or_copy(
    items: Vec<PathBuf>,
    into: PathBuf,
    conflict: Conflict,
    is_move: bool,
    progress: &mut dyn FnMut(Progress),
) -> io::Result<Done> {
    let fm = NSFileManager::defaultManager();
    let total = items.len();
    let mut results = Vec::with_capacity(total);
    let mut displaced = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let name = item
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "item has no file name"))?;
        let dest = into.join(name);
        let placed = place_one(&fm, item, dest, conflict, is_move)?;
        results.extend(placed.dest);
        displaced.extend(placed.displaced);
        progress(Progress {
            done: i + 1,
            total,
            current: Some(item.clone()),
        });
    }
    let undo = undo_of_transfer(is_move, &items, &results, displaced);
    Ok(Done { results, undo })
}

/// What takes a finished transfer back: what landed goes home, and then whatever a Replace
/// displaced goes back on top of it.
fn undo_of_transfer(
    is_move: bool,
    items: &[PathBuf],
    results: &[PathBuf],
    displaced: Vec<Restore>,
) -> Option<Op> {
    let landed = if results.is_empty() {
        None
    } else if is_move {
        common_parent(items).map(|orig| Op::Move {
            items: results.to_vec(),
            into: orig,
        })
    } else {
        Some(Op::Trash(results.to_vec()))
    };
    let restored = (!displaced.is_empty()).then(|| Op::PutBack(displaced));
    match (landed, restored) {
        (Some(landed), Some(restored)) => Some(Op::Batch(vec![landed, restored])),
        (landed, restored) => landed.or(restored),
    }
}

fn put_back(items: Vec<Restore>, progress: &mut dyn FnMut(Progress)) -> io::Result<Done> {
    let fm = NSFileManager::defaultManager();
    let total = items.len();
    let mut results = Vec::with_capacity(total);
    for (i, restore) in items.iter().enumerate() {
        let placed = place_one(
            &fm,
            &restore.item,
            restore.to.clone(),
            Conflict::Replace,
            true,
        )?;
        results.extend(placed.dest);
        progress(Progress {
            done: i + 1,
            total,
            current: Some(restore.item.clone()),
        });
    }
    Ok(Done {
        results,
        undo: None,
    })
}

fn batch(ops: Vec<Op>, conflict: Conflict, progress: &mut dyn FnMut(Progress)) -> io::Result<Done> {
    let mut results = Vec::new();
    for op in ops {
        results.extend(run(op, conflict, progress)?.results);
    }
    Ok(Done {
        results,
        undo: None,
    })
}

fn trash(items: Vec<PathBuf>, progress: &mut dyn FnMut(Progress)) -> io::Result<Done> {
    let fm = NSFileManager::defaultManager();
    let total = items.len();
    let mut results = Vec::with_capacity(total);
    for (i, item) in items.iter().enumerate() {
        results.push(trash_one(&fm, item)?);
        progress(Progress {
            done: i + 1,
            total,
            current: Some(item.clone()),
        });
    }
    let undo = common_parent(&items).map(|orig| Op::Move {
        items: results.clone(),
        into: orig,
    });
    Ok(Done { results, undo })
}

fn delete(items: Vec<PathBuf>, progress: &mut dyn FnMut(Progress)) -> io::Result<Done> {
    let fm = NSFileManager::defaultManager();
    let total = items.len();
    for (i, item) in items.iter().enumerate() {
        remove_path(&fm, item)?;
        progress(Progress {
            done: i + 1,
            total,
            current: Some(item.clone()),
        });
    }
    Ok(Done {
        results: items,
        undo: None,
    })
}

fn rename(path: PathBuf, name: String) -> io::Result<Done> {
    check_name(&name)?;
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "item has no parent"))?;
    let dest = dir.join(&name);
    if dest == path {
        return Ok(Done {
            results: vec![path],
            undo: None,
        });
    }
    let fm = NSFileManager::defaultManager();
    transfer(&fm, &path, &dest, true)?;
    let old_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    Ok(Done {
        results: vec![dest.clone()],
        undo: Some(Op::Rename {
            path: dest,
            name: old_name,
        }),
    })
}

fn duplicate(items: Vec<PathBuf>, progress: &mut dyn FnMut(Progress)) -> io::Result<Done> {
    let fm = NSFileManager::defaultManager();
    let total = items.len();
    let mut results = Vec::with_capacity(total);
    for (i, item) in items.iter().enumerate() {
        let dest = duplicate_path(item);
        transfer(&fm, item, &dest, false)?;
        results.push(dest);
        progress(Progress {
            done: i + 1,
            total,
            current: Some(item.clone()),
        });
    }
    Ok(Done {
        results: results.clone(),
        undo: Some(Op::Trash(results)),
    })
}

fn new_item(into: PathBuf, name: String, is_dir: bool) -> io::Result<Done> {
    check_name(&name)?;
    let fm = NSFileManager::defaultManager();
    let mut dest = into.join(&name);
    if dest.exists() {
        dest = keep_both_path(&dest);
    }
    if is_dir {
        unsafe {
            fm.createDirectoryAtURL_withIntermediateDirectories_attributes_error(
                &url(&dest),
                false,
                None::<&NSDictionary<NSFileAttributeKey, AnyObject>>,
            )
        }
        .map_err(io_error)?;
    } else {
        let ok = unsafe {
            fm.createFileAtPath_contents_attributes(
                &NSString::from_str(&dest.to_string_lossy()),
                None,
                None::<&NSDictionary<NSFileAttributeKey, AnyObject>>,
            )
        };
        if !ok {
            return Err(io::Error::other("could not create file"));
        }
    }
    Ok(Done {
        results: vec![dest.clone()],
        undo: Some(Op::Trash(vec![dest])),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("finder-fs-ops-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
    }

    fn no_progress(_: Progress) {}

    /// The paths an op's undo would put back: what a Replace displaced into the Trash.
    fn displaced(op: &Op) -> Vec<PathBuf> {
        match op {
            Op::PutBack(items) => items.iter().map(|item| item.item.clone()).collect(),
            Op::Batch(ops) => ops.iter().flat_map(displaced).collect(),
            _ => Vec::new(),
        }
    }

    fn purge(paths: &[PathBuf]) {
        for path in paths {
            let _ = fs::remove_file(path);
            let _ = fs::remove_dir_all(path);
        }
    }

    #[test]
    fn copy_round_trip() {
        let dir = temp_dir("copy");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let file = src.join("a.txt");
        touch(&file, "hello");

        let done = perform(
            Op::Copy {
                items: vec![file.clone()],
                into: dst.clone(),
            },
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert_eq!(done.results, vec![dst.join("a.txt")]);
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "hello");
        assert!(file.exists(), "copy must leave the source in place");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn move_round_trip() {
        let dir = temp_dir("move");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let file = src.join("a.txt");
        touch(&file, "hello");

        let done = perform(
            Op::Move {
                items: vec![file.clone()],
                into: dst.clone(),
            },
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert_eq!(done.results, vec![dst.join("a.txt")]);
        assert!(!file.exists(), "move must remove the source");

        let undo = done.undo.expect("a move undoes back to its source folder");
        let back = perform(undo, Conflict::Replace, no_progress).unwrap();
        assert_eq!(back.results, vec![file.clone()]);
        assert!(file.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn keep_both_names_like_finder() {
        let dir = temp_dir("keepboth");
        touch(&dir.join("a.txt"), "one");
        let into = dir.join("dest");
        fs::create_dir_all(&into).unwrap();
        touch(&into.join("a.txt"), "existing");

        let done = perform(
            Op::Copy {
                items: vec![dir.join("a.txt")],
                into: into.clone(),
            },
            Conflict::KeepBoth,
            no_progress,
        )
        .unwrap();
        assert_eq!(done.results, vec![into.join("a 2.txt")]);
        assert_eq!(fs::read_to_string(into.join("a.txt")).unwrap(), "existing");
        assert_eq!(fs::read_to_string(into.join("a 2.txt")).unwrap(), "one");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_recurses_and_files_replace() {
        let dir = temp_dir("merge");
        let src = dir.join("Folder");
        let dst_parent = dir.join("dest");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::create_dir_all(&dst_parent).unwrap();
        touch(&src.join("keep.txt"), "from src");
        touch(&src.join("nested").join("child.txt"), "child");
        let dst = dst_parent.join("Folder");
        fs::create_dir_all(dst.join("nested")).unwrap();
        touch(&dst.join("keep.txt"), "from dst");
        touch(&dst.join("only-in-dst.txt"), "untouched");

        let done = perform(
            Op::Move {
                items: vec![src.clone()],
                into: dst_parent.clone(),
            },
            Conflict::Merge,
            no_progress,
        )
        .unwrap();
        assert_eq!(done.results, vec![dst.clone()]);
        assert!(!src.exists(), "a moved, merged folder disappears");
        assert_eq!(
            fs::read_to_string(dst.join("keep.txt")).unwrap(),
            "from src",
            "files follow Replace"
        );
        assert_eq!(
            fs::read_to_string(dst.join("only-in-dst.txt")).unwrap(),
            "untouched"
        );
        assert_eq!(
            fs::read_to_string(dst.join("nested").join("child.txt")).unwrap(),
            "child"
        );

        purge(&displaced(&done.undo.unwrap()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn skip_leaves_the_item_where_it_was() {
        let dir = temp_dir("skip");
        let into = dir.join("dest");
        fs::create_dir_all(&into).unwrap();
        touch(&dir.join("a.txt"), "one");
        touch(&into.join("a.txt"), "existing");

        let done = perform(
            Op::Copy {
                items: vec![dir.join("a.txt")],
                into: into.clone(),
            },
            Conflict::Skip,
            no_progress,
        )
        .unwrap();
        assert!(done.results.is_empty(), "nothing landed");
        assert_eq!(fs::read_to_string(into.join("a.txt")).unwrap(), "existing");
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "one");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_skipped_copy_has_nothing_to_undo() {
        let dir = temp_dir("skip-undo");
        let into = dir.join("dest");
        fs::create_dir_all(&into).unwrap();
        let source = dir.join("a.txt");
        touch(&source, "MY ONLY COPY");
        touch(&into.join("a.txt"), "existing");

        let done = perform(
            Op::Copy {
                items: vec![source.clone()],
                into: into.clone(),
            },
            Conflict::Skip,
            no_progress,
        )
        .unwrap();
        assert!(
            done.undo.is_none(),
            "undoing a skip would trash the source it never touched"
        );
        if let Some(undo) = done.undo {
            perform(undo, Conflict::Replace, no_progress).unwrap();
        }
        assert_eq!(fs::read_to_string(&source).unwrap(), "MY ONLY COPY");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_keeps_the_destination_when_the_transfer_fails() {
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("skipping: root reads a file whatever its mode says");
            return;
        }
        let dir = temp_dir("replace-fails");
        let into = dir.join("dest");
        fs::create_dir_all(&into).unwrap();
        let source = dir.join("a.txt");
        touch(&source, "new");
        touch(&into.join("a.txt"), "IRREPLACEABLE");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o000)).unwrap();

        perform(
            Op::Copy {
                items: vec![source.clone()],
                into: into.clone(),
            },
            Conflict::Replace,
            no_progress,
        )
        .expect_err("an unreadable source cannot be copied");
        assert_eq!(
            fs::read_to_string(into.join("a.txt")).unwrap(),
            "IRREPLACEABLE",
            "a replace that failed left the destination destroyed"
        );
        let left: Vec<_> = fs::read_dir(&into)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(left.len(), 1, "a staged transfer was left behind: {left:?}");

        let _ = fs::set_permissions(&source, fs::Permissions::from_mode(0o600));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_undoes_back_to_the_item_it_displaced() {
        let dir = temp_dir("replace-undo");
        let into = dir.join("dest");
        fs::create_dir_all(&into).unwrap();
        touch(&dir.join("a.txt"), "new");
        touch(&into.join("a.txt"), "MY ONLY COPY");

        let done = perform(
            Op::Copy {
                items: vec![dir.join("a.txt")],
                into: into.clone(),
            },
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert_eq!(done.results, vec![into.join("a.txt")]);
        assert_eq!(fs::read_to_string(into.join("a.txt")).unwrap(), "new");

        let undo = done.undo.expect("a replace can be undone");
        let back = perform(undo, Conflict::Replace, no_progress).unwrap();
        assert_eq!(
            fs::read_to_string(into.join("a.txt")).unwrap(),
            "MY ONLY COPY",
            "undo put the replaced item back"
        );

        purge(&back.results);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_round_trip() {
        let dir = temp_dir("rename");
        let file = dir.join("old.txt");
        touch(&file, "content");

        let done = perform(
            Op::Rename {
                path: file.clone(),
                name: "new.txt".into(),
            },
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        let renamed = dir.join("new.txt");
        assert_eq!(done.results, vec![renamed.clone()]);
        assert!(renamed.exists() && !file.exists());

        let undo = done.undo.unwrap();
        let back = perform(undo, Conflict::Replace, no_progress).unwrap();
        assert_eq!(back.results, vec![file.clone()]);
        assert!(file.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_takes_a_name_and_never_a_path() {
        let dir = temp_dir("rename-escape");
        let file = dir.join("a.txt");
        touch(&file, "mine");
        fs::create_dir_all(dir.join("sub")).unwrap();
        let outside = std::env::temp_dir().join(format!("ngv-escaped-{}.txt", std::process::id()));
        let _ = fs::remove_file(&outside);
        let long = "x".repeat(256);

        for name in [
            "sub/moved.txt",
            &outside.to_string_lossy(),
            "",
            ".",
            "..",
            "a\0b",
            &long,
        ] {
            let err = perform(
                Op::Rename {
                    path: file.clone(),
                    name: name.to_string(),
                },
                Conflict::Replace,
                no_progress,
            )
            .expect_err(&format!("{name:?} is not a name"));
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{name:?}");
        }
        assert!(file.exists(), "the item stayed where it was");
        assert!(!dir.join("sub").join("moved.txt").exists());
        assert!(!outside.exists(), "the item never left the folder");

        let done = perform(
            Op::Rename {
                path: file.clone(),
                name: "a:b.txt".into(),
            },
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert_eq!(done.results, vec![dir.join("a:b.txt")]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_names_beside_the_original() {
        let dir = temp_dir("duplicate");
        let file = dir.join("doc.txt");
        touch(&file, "content");

        let done = perform(
            Op::Duplicate(vec![file.clone()]),
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert_eq!(done.results, vec![dir.join("doc copy.txt")]);

        let again = perform(
            Op::Duplicate(vec![file.clone()]),
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert_eq!(again.results, vec![dir.join("doc copy 2.txt")]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_folder_and_new_file_round_trip() {
        let dir = temp_dir("new");
        let folder = perform(
            Op::NewFolder {
                into: dir.clone(),
                name: "untitled folder".into(),
            },
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert_eq!(folder.results, vec![dir.join("untitled folder")]);
        assert!(dir.join("untitled folder").is_dir());

        let again = perform(
            Op::NewFolder {
                into: dir.clone(),
                name: "untitled folder".into(),
            },
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert_eq!(again.results, vec![dir.join("untitled folder 2")]);

        let file = perform(
            Op::NewFile {
                into: dir.clone(),
                name: "untitled.txt".into(),
            },
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert!(dir.join("untitled.txt").is_file());
        assert_eq!(fs::metadata(&file.results[0]).unwrap().len(), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trash_and_put_back() {
        let dir = std::env::temp_dir().join(format!("finder-fs-ops-trash-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("throwaway.txt");
        touch(&file, "gone soon");

        let done = perform(
            Op::Trash(vec![file.clone()]),
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert!(!file.exists(), "the item left its folder");
        assert!(done.results[0].to_string_lossy().contains(".Trash"));

        let undo = done.undo.expect("trash undoes with a move back");
        let back = perform(undo, Conflict::Replace, no_progress).unwrap();
        assert!(file.exists(), "put back restores the original path");
        assert_eq!(back.results, vec![file.clone()]);

        let _ = fs::remove_file(&file);
        let _ = fs::remove_dir_all(&dir);
    }
}
