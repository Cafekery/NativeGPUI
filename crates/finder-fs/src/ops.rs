//! File operations with Finder's semantics: copies and moves through NSFileManager (packages,
//! metadata and permissions intact), Trash with Put Back, and an undo journal.

use std::ffi::CString;
use std::io;
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSByteCountFormatter, NSByteCountFormatterCountStyle, NSDictionary, NSError,
    NSFileAttributeKey, NSFileManager, NSFileNoSuchFileError, NSFileReadNoPermissionError,
    NSFileReadNoSuchFileError, NSFileWriteFileExistsError, NSFileWriteInvalidFileNameError,
    NSFileWriteNoPermissionError, NSFileWriteOutOfSpaceError, NSFileWriteVolumeReadOnlyError,
    NSString, NSURL,
};

use crate::volumes;

mod deep;

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
        Op::Rename { path, name } => rename(path, name, conflict),
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

/// The bytes an item takes, itself and everything under it.
pub(crate) fn walk_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| walk_size(&entry.path()))
                .sum()
        })
        .unwrap_or(0)
}

/// What a volume, rather than the item, refused.
#[derive(Clone, Copy)]
enum Refusal {
    ReadOnly,
    NoSpace,
}

fn refusal_of(err: &NSError) -> Option<Refusal> {
    if err.domain().to_string() != "NSCocoaErrorDomain" {
        return None;
    }
    match err.code() {
        code if code == NSFileWriteVolumeReadOnlyError => Some(Refusal::ReadOnly),
        code if code == NSFileWriteOutOfSpaceError => Some(Refusal::NoSpace),
        _ => None,
    }
}

fn byte_string(bytes: u64) -> String {
    let formatter = NSByteCountFormatter::new();
    formatter.setCountStyle(NSByteCountFormatterCountStyle::File);
    formatter.stringFromByteCount(bytes as i64).to_string()
}

/// The volume `path` is on: the deepest mount point it sits under.
fn volume_of(path: &Path) -> Option<volumes::Volume> {
    volumes::mounted()
        .into_iter()
        .filter(|volume| path.starts_with(&volume.path) && !volume.name.is_empty())
        .max_by_key(|volume| volume.path.as_os_str().len())
}

/// Finder names the volume the item was going to and how much more it would have taken;
/// `NSFileManager`'s own string names the boot volume whatever the destination was.
fn refusal_message(
    refusal: Refusal,
    item: &str,
    dest: &Path,
    is_move: bool,
    needed: u64,
) -> Option<String> {
    let volume = volume_of(dest)?;
    let name = volume.name;
    Some(match refusal {
        Refusal::ReadOnly => {
            let verb = if is_move { "moved" } else { "copied" };
            format!(
                "The item \u{201c}{item}\u{201d} can't be {verb} because the volume \u{201c}{name}\u{201d} is read only."
            )
        }
        Refusal::NoSpace => {
            let verb = if is_move { "move" } else { "copy" };
            let short = match needed.saturating_sub(volume.available) {
                0 => String::new(),
                short => format!(" You need an additional {}.", byte_string(short)),
            };
            format!(
                "There isn't enough space on \u{201c}{name}\u{201d} to {verb} \u{201c}{item}\u{201d}.{short}"
            )
        }
    })
}

/// Whether the file system refused a path rather than the item at it: `NSFileManager` builds an
/// absolute path per item and gives up once one passes `PATH_MAX`.
fn past_path_max(err: &NSError) -> bool {
    let too_long = |err: &NSError| {
        err.domain().to_string() == "NSPOSIXErrorDomain" && err.code() as i32 == libc::ENAMETOOLONG
    };
    err.domain().to_string() == "NSCocoaErrorDomain"
        && err.code() == NSFileWriteInvalidFileNameError
        && err.underlyingErrors().iter().any(|under| too_long(&under))
}

/// A transfer only ever lands on a free name, so a half-landed tree is ours to clear away and
/// try again for against directory descriptors, which no depth of tree can overrun.
fn transfer(fm: &NSFileManager, src: &Path, dst: &Path, is_move: bool) -> io::Result<()> {
    let (su, du) = (url(src), url(dst));
    let landed = if is_move {
        fm.moveItemAtURL_toURL_error(&su, &du)
    } else {
        fm.copyItemAtURL_toURL_error(&su, &du)
    };
    match landed {
        Ok(()) => Ok(()),
        Err(err) if past_path_max(&err) => {
            let _ = deep::remove(dst);
            deep::transfer(src, dst, is_move)
        }
        Err(err) => Err(volume_refusal(&err, src, dst, is_move).unwrap_or_else(|| io_error(err))),
    }
}

fn volume_refusal(err: &NSError, src: &Path, dest: &Path, is_move: bool) -> Option<io::Error> {
    let item = src.file_name().unwrap_or_default().to_string_lossy();
    let message = refusal_message(refusal_of(err)?, &item, dest, is_move, walk_size(src))?;
    Some(io::Error::other(message))
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

/// Whether the item carries Finder's Locked flag, which stops it being trashed or deleted.
/// Finder asks before it clears one; `Op::Trash` and `Op::DeleteImmediately` clear it themselves.
pub fn locked(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.st_flags() & libc::UF_IMMUTABLE != 0)
}

/// Clears the Locked flag from the item and, for a folder, from everything under it: a locked
/// child refuses the folder it sits in.
fn unlock(path: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    let flags = meta.st_flags();
    // `chflags` follows a link, so the lock a link carries itself is left where it is.
    if flags & libc::UF_IMMUTABLE != 0 && !meta.is_symlink() {
        let name = c_path(path)?;
        if unsafe { libc::chflags(name.as_ptr(), flags & !libc::UF_IMMUTABLE) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    if meta.is_dir() {
        for entry in std::fs::read_dir(path)? {
            unlock(&entry?.path())?;
        }
    }
    Ok(())
}

/// Whether a name is in use: a broken symlink holds its name as firmly as a file does, which
/// `Path::exists` does not see because it follows the link.
fn taken(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Whether both paths name the one item: a rename that only changes a name's case finds the item
/// itself standing at the destination on a case-insensitive volume.
fn same_item(one: &Path, other: &Path) -> bool {
    let id = |path: &Path| {
        std::fs::symlink_metadata(path)
            .map(|meta| (meta.dev(), meta.ino()))
            .ok()
    };
    matches!((id(one), id(other)), (Some(one), Some(other)) if one == other)
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
        if !taken(&candidate) {
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
    if !taken(&first) {
        return first;
    }
    let mut n = 2;
    loop {
        let candidate = dir.join(with_ext(&format!("{stem} copy {n}"), ext));
        if !taken(&candidate) {
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

fn trash_at(fm: &NSFileManager, path: &Path) -> io::Result<PathBuf> {
    let mut resulting: Option<Retained<NSURL>> = None;
    fm.trashItemAtURL_resultingItemURL_error(&url(path), Some(&mut resulting))
        .map_err(io_error)?;
    Ok(resulting
        .map(|u| path_from(&u))
        .unwrap_or_else(|| path.to_path_buf()))
}

/// A refusal a lock explains is retried once the lock is off, the way Finder carries on after
/// its "the item is locked" question.
fn past_the_lock<T>(
    path: &Path,
    refused: io::Error,
    again: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    if refused.kind() != io::ErrorKind::PermissionDenied || unlock(path).is_err() {
        return Err(refused);
    }
    again()
}

fn trash_one(fm: &NSFileManager, path: &Path) -> io::Result<PathBuf> {
    match trash_at(fm, path) {
        Err(refused) => past_the_lock(path, refused, || trash_at(fm, path)),
        landed => landed,
    }
}

fn delete_one(fm: &NSFileManager, path: &Path) -> io::Result<()> {
    match remove_path(fm, path) {
        Err(refused) => past_the_lock(path, refused, || remove_path(fm, path)),
        gone => gone,
    }
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
    if !taken(&dest) {
        transfer(fm, src, &dest, is_move)?;
        return Ok(Placed::at(dest));
    }
    // The item standing at the destination is the item going there: Finder leaves a move where it
    // is and makes a copy into the same folder a Duplicate.
    if same_item(src, &dest) {
        if is_move {
            return Ok(Placed::at(dest));
        }
        let dest = duplicate_path(&dest);
        transfer(fm, src, &dest, false)?;
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

/// Finder refuses before it starts: nothing goes inside itself, however deep the descendant.
fn refuse_into_itself(item: &Path, into: &Path, is_move: bool) -> io::Result<()> {
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !real(into).starts_with(real(item)) {
        return Ok(());
    }
    let name = item.file_name().unwrap_or_default().to_string_lossy();
    let verb = if is_move { "moved" } else { "copied" };
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("The item \u{201c}{name}\u{201d} can't be {verb} into itself."),
    ))
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
    let mut home = Vec::with_capacity(total);
    let mut displaced = Vec::new();
    for item in &items {
        refuse_into_itself(item, &into, is_move)?;
    }
    for (i, item) in items.iter().enumerate() {
        let name = item
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "item has no file name"))?;
        let dest = into.join(name);
        let placed = place_one(&fm, item, dest, conflict, is_move)?;
        if let Some(dest) = placed.dest {
            home.push(Restore {
                item: dest.clone(),
                to: item.clone(),
            });
            results.push(dest);
        }
        displaced.extend(placed.displaced);
        progress(Progress {
            done: i + 1,
            total,
            current: Some(item.clone()),
        });
    }
    let undo = undo_of_transfer(is_move, home, &results, displaced);
    Ok(Done { results, undo })
}

/// What takes a finished transfer back: each item to the path it came from, or the Trash for a
/// copy, and then whatever a Replace displaced goes back on top of it.
fn undo_of_transfer(
    is_move: bool,
    home: Vec<Restore>,
    results: &[PathBuf],
    displaced: Vec<Restore>,
) -> Option<Op> {
    let mut put_back = if is_move { home } else { Vec::new() };
    put_back.extend(displaced);
    let landed = (!is_move && !results.is_empty()).then(|| Op::Trash(results.to_vec()));
    let restored = (!put_back.is_empty()).then_some(Op::PutBack(put_back));
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
    let mut home = Vec::with_capacity(total);
    for (i, item) in items.iter().enumerate() {
        let trashed = trash_one(&fm, item)?;
        home.push(Restore {
            item: trashed.clone(),
            to: item.clone(),
        });
        results.push(trashed);
        progress(Progress {
            done: i + 1,
            total,
            current: Some(item.clone()),
        });
    }
    let undo = (!home.is_empty()).then_some(Op::PutBack(home));
    Ok(Done { results, undo })
}

fn delete(items: Vec<PathBuf>, progress: &mut dyn FnMut(Progress)) -> io::Result<Done> {
    let fm = NSFileManager::defaultManager();
    let total = items.len();
    for (i, item) in items.iter().enumerate() {
        delete_one(&fm, item)?;
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

fn rename(path: PathBuf, name: String, conflict: Conflict) -> io::Result<Done> {
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
    let placed = if same_item(&path, &dest) {
        transfer(&fm, &path, &dest, true)?;
        Placed::at(dest)
    } else {
        place_one(&fm, &path, dest, conflict, true)?
    };
    let Some(dest) = placed.dest else {
        return Ok(Done {
            results: Vec::new(),
            undo: None,
        });
    };
    let old_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let back = Op::Rename {
        path: dest.clone(),
        name: old_name,
    };
    let undo = match placed.displaced.is_empty() {
        true => back,
        false => Op::Batch(vec![back, Op::PutBack(placed.displaced)]),
    };
    Ok(Done {
        results: vec![dest],
        undo: Some(undo),
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
    let undo = (!results.is_empty()).then(|| Op::Trash(results.clone()));
    Ok(Done { results, undo })
}

fn new_item(into: PathBuf, name: String, is_dir: bool) -> io::Result<Done> {
    check_name(&name)?;
    let fm = NSFileManager::defaultManager();
    let mut dest = into.join(&name);
    if taken(&dest) {
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
    fn moving_an_item_onto_itself_changes_nothing() {
        let dir = temp_dir("onto-itself");
        let file = dir.join("a.txt");
        touch(&file, "one");

        let done = perform(
            Op::Move {
                items: vec![file.clone()],
                into: dir.clone(),
            },
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert_eq!(done.results, vec![file.clone()]);
        assert_eq!(fs::read_to_string(&file).unwrap(), "one");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn copying_into_its_own_folder_duplicates() {
        let dir = temp_dir("own-folder");
        let file = dir.join("a.txt");
        touch(&file, "one");

        let done = perform(
            Op::Copy {
                items: vec![file.clone()],
                into: dir.clone(),
            },
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert_eq!(done.results, vec![dir.join("a copy.txt")]);
        assert_eq!(fs::read_to_string(&file).unwrap(), "one");
        assert_eq!(fs::read_to_string(dir.join("a copy.txt")).unwrap(), "one");

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
    fn a_dangling_link_holds_the_name_it_sits_on() {
        let dir = temp_dir("dangling");
        let into = dir.join("dest");
        fs::create_dir_all(&into).unwrap();
        touch(&dir.join("a.txt"), "one");
        for name in ["a.txt", "a 2.txt"] {
            std::os::unix::fs::symlink(into.join("gone"), into.join(name)).unwrap();
        }

        let done = perform(
            Op::Copy {
                items: vec![dir.join("a.txt")],
                into: into.clone(),
            },
            Conflict::KeepBoth,
            no_progress,
        )
        .unwrap();
        assert_eq!(done.results, vec![into.join("a 3.txt")]);
        assert_eq!(fs::read_to_string(into.join("a 3.txt")).unwrap(), "one");
        assert!(
            fs::symlink_metadata(into.join("a.txt"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link that was there is still there"
        );

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
    fn a_rename_onto_a_taken_name_resolves_like_a_move() {
        let dir = temp_dir("rename-conflict");
        let source = dir.join("a.txt");
        let taken = dir.join("b.txt");
        touch(&source, "new");
        touch(&taken, "MY ONLY COPY");

        let skipped = perform(
            Op::Rename {
                path: source.clone(),
                name: "b.txt".into(),
            },
            Conflict::Skip,
            no_progress,
        )
        .unwrap();
        assert!(skipped.results.is_empty() && skipped.undo.is_none());
        assert_eq!(fs::read_to_string(&taken).unwrap(), "MY ONLY COPY");
        assert_eq!(fs::read_to_string(&source).unwrap(), "new");

        let done = perform(
            Op::Rename {
                path: source.clone(),
                name: "b.txt".into(),
            },
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert_eq!(done.results, vec![taken.clone()]);
        assert_eq!(fs::read_to_string(&taken).unwrap(), "new");
        assert!(!source.exists());

        let undo = done.undo.expect("a rename that replaced can be undone");
        perform(undo, Conflict::Replace, no_progress).unwrap();
        assert_eq!(
            fs::read_to_string(&taken).unwrap(),
            "MY ONLY COPY",
            "undo put the replaced item back"
        );
        assert_eq!(fs::read_to_string(&source).unwrap(), "new");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rename_that_only_changes_case_keeps_the_item() {
        let dir = temp_dir("rename-case");
        let file = dir.join("a.txt");
        touch(&file, "mine");

        let done = perform(
            Op::Rename {
                path: file.clone(),
                name: "A.TXT".into(),
            },
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert_eq!(done.results, vec![dir.join("A.TXT")]);
        assert_eq!(fs::read_to_string(dir.join("A.TXT")).unwrap(), "mine");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_refuses_to_go_inside_itself() {
        let dir = temp_dir("into-itself");
        let folder = dir.join("Folder");
        let inner = folder.join("inner");
        fs::create_dir_all(&inner).unwrap();
        touch(&folder.join("a.txt"), "one");

        for (is_move, wording) in [
            (false, "can't be copied into itself"),
            (true, "can't be moved into itself"),
        ] {
            let items = vec![folder.clone()];
            let into = inner.clone();
            let op = if is_move {
                Op::Move { items, into }
            } else {
                Op::Copy { items, into }
            };
            let err = perform(op, Conflict::KeepBoth, no_progress).expect_err(wording);
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert!(err.to_string().contains(wording), "{err}");
        }
        assert_eq!(
            fs::read_dir(&inner).unwrap().count(),
            0,
            "nothing was written into the descendant"
        );

        let err = perform(
            Op::Copy {
                items: vec![folder.clone()],
                into: folder.clone(),
            },
            Conflict::KeepBoth,
            no_progress,
        )
        .expect_err("a folder is inside itself");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

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
    fn a_move_from_two_folders_undoes_to_both() {
        let dir = temp_dir("two-parents");
        let one = dir.join("one");
        let two = dir.join("two");
        let into = dir.join("dest");
        for folder in [&one, &two, &into] {
            fs::create_dir_all(folder).unwrap();
        }
        touch(&one.join("a.txt"), "a");
        touch(&two.join("b.txt"), "b");

        let done = perform(
            Op::Move {
                items: vec![one.join("a.txt"), two.join("b.txt")],
                into: into.clone(),
            },
            Conflict::KeepBoth,
            no_progress,
        )
        .unwrap();
        assert_eq!(done.results.len(), 2);

        let undo = done.undo.expect("a move from two folders is undoable");
        perform(undo, Conflict::Replace, no_progress).unwrap();
        assert_eq!(fs::read_to_string(one.join("a.txt")).unwrap(), "a");
        assert_eq!(fs::read_to_string(two.join("b.txt")).unwrap(), "b");
        assert_eq!(fs::read_dir(&into).unwrap().count(), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn put_back_restores_the_name_the_trash_took() {
        let dir = temp_dir("put-back");
        let file = dir.join("collide.txt");
        touch(&file, "first");
        let first = perform(
            Op::Trash(vec![file.clone()]),
            Conflict::Replace,
            no_progress,
        )
        .unwrap();

        touch(&file, "second");
        let second = perform(
            Op::Trash(vec![file.clone()]),
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert_ne!(
            second.results[0].file_name(),
            file.file_name(),
            "the Trash renames what collides in it"
        );

        let undo = second.undo.expect("trash undoes with a put back");
        let back = perform(undo, Conflict::Replace, no_progress).unwrap();
        assert_eq!(back.results, vec![file.clone()]);
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "second",
            "put back restores the name the item had"
        );

        purge(&first.results);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Runs `script` in the directory `levels` down a `deep_tree`, which no path can name.
    fn deep_sh(root: &Path, levels: usize, script: &str) -> String {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "cd \"$1\" && for i in $(seq 1 $2); do cd d$i || exit 1; done && {script}"
            ))
            .arg("sh")
            .arg(root)
            .arg(levels.to_string())
            .output()
            .unwrap();
        assert!(out.status.success(), "{script}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A tree whose leaf is past `PATH_MAX`, built by a shell that walks in as it goes.
    fn deep_tree(root: &Path, levels: usize) {
        let ok = std::process::Command::new("sh")
            .arg("-c")
            .arg(
                "mkdir -p \"$1\" && cd \"$1\" && for i in $(seq 1 $2); do mkdir d$i && cd d$i || exit 1; done && echo leaf > leaf.txt",
            )
            .arg("sh")
            .arg(root)
            .arg(levels.to_string())
            .status()
            .unwrap();
        assert!(ok.success(), "could not build the fixture");
    }

    /// The open-file limit a launchd process gets, put back on the way out: a walk that holds a
    /// descriptor per level runs out of them long before it runs out of path.
    struct FewDescriptors(libc::rlimit);

    impl FewDescriptors {
        fn new() -> FewDescriptors {
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
            let few = libc::rlimit {
                rlim_cur: limit.rlim_cur.min(128),
                rlim_max: limit.rlim_max,
            };
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &few) };
            FewDescriptors(limit)
        }
    }

    impl Drop for FewDescriptors {
        fn drop(&mut self) {
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &self.0) };
        }
    }

    #[test]
    fn a_tree_deeper_than_a_path_transfers_whole() {
        let dir = temp_dir("deep");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let tree = src.join("Deep");
        deep_tree(&tree, 300);
        touch(&tree.join("beside.txt"), "beside");
        std::os::unix::fs::symlink("beside.txt", tree.join("link")).unwrap();

        let few = FewDescriptors::new();
        let done = perform(
            Op::Copy {
                items: vec![tree.clone()],
                into: dst.clone(),
            },
            Conflict::KeepBoth,
            no_progress,
        )
        .unwrap();
        assert_eq!(done.results, vec![dst.join("Deep")]);
        assert_eq!(deep_sh(&dst.join("Deep"), 300, "cat leaf.txt"), "leaf");
        assert_eq!(
            fs::read_link(dst.join("Deep").join("link")).unwrap(),
            Path::new("beside.txt"),
            "the link came over as a link"
        );
        assert_eq!(
            deep_sh(&tree, 300, "cat leaf.txt"),
            "leaf",
            "the source is still whole"
        );

        perform(
            Op::Rename {
                path: dst.join("Deep"),
                name: "Moved".into(),
            },
            Conflict::KeepBoth,
            no_progress,
        )
        .unwrap();
        assert_eq!(deep_sh(&dst.join("Moved"), 300, "cat leaf.txt"), "leaf");
        drop(few);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_deep_copy_that_fails_leaves_nothing_behind() {
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("skipping: root walks into a directory whatever its mode says");
            return;
        }
        let dir = temp_dir("deep-fails");
        let dst = dir.join("dst");
        fs::create_dir_all(&dst).unwrap();
        let tree = dir.join("Deep");
        deep_tree(&tree, 300);
        deep_sh(&tree, 249, "chmod 000 d250");

        perform(
            Op::Copy {
                items: vec![tree.clone()],
                into: dst.clone(),
            },
            Conflict::KeepBoth,
            no_progress,
        )
        .expect_err("a directory that cannot be read cannot be copied");
        assert!(
            !taken(&dst.join("Deep")),
            "a copy that failed left a part of the tree behind"
        );

        deep_sh(&tree, 249, "chmod 700 d250");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_volume_refusal_names_the_volume_it_was_aimed_at() {
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("skipping: root is not refused by a volume's mode");
            return;
        }
        let dir = temp_dir("volume");
        let file = dir.join("m.txt");
        touch(&file, "x");
        let boot = volume_of(Path::new("/")).expect("the boot volume is mounted");

        let err = perform(
            Op::Copy {
                items: vec![file.clone()],
                into: PathBuf::from("/"),
            },
            Conflict::KeepBoth,
            no_progress,
        )
        .expect_err("the system volume is read only");
        assert_eq!(
            err.to_string(),
            format!(
                "The item \u{201c}m.txt\u{201d} can't be copied because the volume \u{201c}{}\u{201d} is read only.",
                boot.name
            )
        );
        assert!(!Path::new("/m.txt").exists());

        let full = refusal_message(Refusal::NoSpace, "m.txt", &dir, false, u64::MAX)
            .expect("the temporary folder is on a mounted volume");
        assert!(
            full.starts_with(&format!(
                "There isn't enough space on \u{201c}{}\u{201d} to copy \u{201c}m.txt\u{201d}.",
                volume_of(&dir).unwrap().name
            )) && full.contains("You need an additional "),
            "{full}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_locked_item_is_unlocked_and_removed() {
        let dir = temp_dir("locked");
        let lock = |path: &Path| {
            assert!(
                std::process::Command::new("/usr/bin/chflags")
                    .arg("uchg")
                    .arg(path)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        let file = dir.join("locked.txt");
        touch(&file, "locked");
        lock(&file);
        assert!(locked(&file));

        let done = perform(
            Op::Trash(vec![file.clone()]),
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert!(!taken(&file), "the locked item left its folder");
        let undo = done.undo.expect("a trash undoes with a put back");
        let back = perform(undo, Conflict::Replace, no_progress).unwrap();
        assert_eq!(back.results, vec![file.clone()]);

        let folder = dir.join("Folder");
        fs::create_dir_all(&folder).unwrap();
        touch(&folder.join("child.txt"), "child");
        lock(&folder.join("child.txt"));
        perform(
            Op::DeleteImmediately(vec![folder.clone()]),
            Conflict::Replace,
            no_progress,
        )
        .unwrap();
        assert!(!folder.exists(), "a locked child stopped nothing");

        let _ = std::process::Command::new("/usr/bin/chflags")
            .args(["-R", "nouchg"])
            .arg(&dir)
            .status();
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
