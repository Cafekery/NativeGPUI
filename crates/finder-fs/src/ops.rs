//! File operations with Finder's semantics: copies and moves through NSFileManager (packages,
//! metadata and permissions intact), Trash with Put Back, and an undo journal.
//!
//! `perform` runs on the caller's thread — callers spawn; progress is reported once per item in
//! `items`; conflicts follow `Conflict`; every `Done` carries the operation that undoes it when
//! one exists.

use std::io;
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSDictionary, NSError, NSFileAttributeKey, NSFileManager, NSFileNoSuchFileError,
    NSFileReadNoPermissionError, NSFileReadNoSuchFileError, NSFileWriteFileExistsError,
    NSFileWriteNoPermissionError, NSString, NSURL,
};

#[derive(Clone, Debug)]
pub enum Op {
    Move { items: Vec<PathBuf>, into: PathBuf },
    Copy { items: Vec<PathBuf>, into: PathBuf },
    Trash(Vec<PathBuf>),
    DeleteImmediately(Vec<PathBuf>),
    Rename { path: PathBuf, name: String },
    Duplicate(Vec<PathBuf>),
    NewFolder { into: PathBuf, name: String },
    NewFile { into: PathBuf, name: String },
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
    match op {
        Op::Move { items, into } => move_or_copy(items, into, conflict, true, &mut progress),
        Op::Copy { items, into } => move_or_copy(items, into, conflict, false, &mut progress),
        Op::Trash(items) => trash(items, &mut progress),
        Op::DeleteImmediately(items) => delete(items, &mut progress),
        Op::Rename { path, name } => rename(path, name),
        Op::Duplicate(items) => duplicate(items, &mut progress),
        Op::NewFolder { into, name } => new_item(into, name, true),
        Op::NewFile { into, name } => new_item(into, name, false),
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

/// Places `src` at `dest`, resolving a conflict at `dest` if one already exists there.
fn place_one(
    fm: &NSFileManager,
    src: &Path,
    dest: PathBuf,
    conflict: Conflict,
    is_move: bool,
) -> io::Result<PathBuf> {
    if !dest.exists() {
        transfer(fm, src, &dest, is_move)?;
        return Ok(dest);
    }
    match conflict {
        Conflict::Skip => Ok(src.to_path_buf()),
        Conflict::KeepBoth => {
            let dest = keep_both_path(&dest);
            transfer(fm, src, &dest, is_move)?;
            Ok(dest)
        }
        Conflict::Replace => {
            remove_path(fm, &dest)?;
            transfer(fm, src, &dest, is_move)?;
            Ok(dest)
        }
        Conflict::Merge if src.is_dir() && dest.is_dir() => {
            for entry in std::fs::read_dir(src)? {
                let entry = entry?;
                let child_dest = dest.join(entry.file_name());
                place_one(fm, &entry.path(), child_dest, Conflict::Merge, is_move)?;
            }
            if is_move {
                std::fs::remove_dir(src)?;
            }
            Ok(dest)
        }
        Conflict::Merge => {
            remove_path(fm, &dest)?;
            transfer(fm, src, &dest, is_move)?;
            Ok(dest)
        }
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
    progress: &mut impl FnMut(Progress),
) -> io::Result<Done> {
    let fm = NSFileManager::defaultManager();
    let total = items.len();
    let mut results = Vec::with_capacity(total);
    for (i, item) in items.iter().enumerate() {
        let name = item
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "item has no file name"))?;
        let dest = into.join(name);
        results.push(place_one(&fm, item, dest, conflict, is_move)?);
        progress(Progress {
            done: i + 1,
            total,
            current: Some(item.clone()),
        });
    }
    let undo = if is_move {
        common_parent(&items).map(|orig| Op::Move {
            items: results.clone(),
            into: orig,
        })
    } else {
        Some(Op::Trash(results.clone()))
    };
    Ok(Done { results, undo })
}

fn trash(items: Vec<PathBuf>, progress: &mut impl FnMut(Progress)) -> io::Result<Done> {
    let fm = NSFileManager::defaultManager();
    let total = items.len();
    let mut results = Vec::with_capacity(total);
    for (i, item) in items.iter().enumerate() {
        let mut resulting: Option<Retained<NSURL>> = None;
        fm.trashItemAtURL_resultingItemURL_error(&url(item), Some(&mut resulting))
            .map_err(io_error)?;
        results.push(
            resulting
                .map(|u| path_from(&u))
                .unwrap_or_else(|| item.clone()),
        );
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

fn delete(items: Vec<PathBuf>, progress: &mut impl FnMut(Progress)) -> io::Result<Done> {
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

fn duplicate(items: Vec<PathBuf>, progress: &mut impl FnMut(Progress)) -> io::Result<Done> {
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
        assert_eq!(done.results, vec![dir.join("a.txt")]);
        assert_eq!(fs::read_to_string(into.join("a.txt")).unwrap(), "existing");

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
