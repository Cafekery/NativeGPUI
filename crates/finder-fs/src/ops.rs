use std::borrow::Cow;
use std::io;
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSByteCountFormatter, NSByteCountFormatterCountStyle, NSCocoaErrorDomain, NSDictionary,
    NSError, NSFileAttributeKey, NSFileManager, NSFileNoSuchFileError, NSFilePathErrorKey,
    NSFileReadNoPermissionError, NSFileReadNoSuchFileError, NSFileWriteFileExistsError,
    NSFileWriteInvalidFileNameError, NSFileWriteNoPermissionError, NSFileWriteOutOfSpaceError,
    NSFileWriteVolumeReadOnlyError, NSString, NSURL, NSURLBookmarkCreationOptions,
    NSURLBookmarkFileCreationOptions,
};

use crate::volumes;

mod deep;
mod quick;
pub mod transfer;

pub use quick::{ImageFormat, ImageSize, Quick};

use transfer::{End, Failure, Kind, OnError, Request, Transfer};

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
    /// A copy that also keeps each item's owner and group, as far as this process may set them.
    CopyExactly {
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
    /// A bookmark file beside each item, not a symlink.
    MakeAlias(Vec<PathBuf>),
    Compress(Vec<PathBuf>),
    NewFolder {
        into: PathBuf,
        name: String,
    },
    NewFile {
        into: PathBuf,
        name: String,
    },
    PutBack(Vec<Restore>),
    /// A transfer with every destination already chosen, as a retry runs it again.
    Retry(Request),
    Quick(Quick),
    Batch(Vec<Op>),
}

#[derive(Clone, Debug)]
pub struct Restore {
    pub item: PathBuf,
    pub to: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// `name 2`, `name 3`, …
    KeepBoth,
    Replace,
    /// Folders merge, files inside follow `Replace`.
    Merge,
    Skip,
}

#[derive(Clone, Debug, Default)]
pub struct Done {
    /// Where each item ended up, in input order.
    pub results: Vec<PathBuf>,
    pub undo: Option<Op>,
}

pub fn perform(op: Op, conflict: Conflict) -> io::Result<Done> {
    let op = match carry(op, conflict, OnError::Stop, &Transfer::default()) {
        Ok(carried) => return carried.into_done(),
        Err(op) => op,
    };
    match op {
        Op::Trash(items) => trash(items),
        Op::DeleteImmediately(items) => delete(items),
        Op::Rename { path, name } => rename(path, name, conflict),
        Op::MakeAlias(items) => make_alias(items),
        Op::Compress(items) => compress(items),
        Op::NewFolder { into, name } => new_item(into, name, true),
        Op::NewFile { into, name } => new_item(into, name, false),
        Op::Quick(quick) => quick::perform(quick),
        Op::Batch(ops) => batch(ops, conflict),
        Op::Move { .. }
        | Op::Copy { .. }
        | Op::CopyExactly { .. }
        | Op::Duplicate(_)
        | Op::PutBack(_)
        | Op::Retry(_) => unreachable!("carried above"),
    }
}

/// What a move, copy, duplicate or put back did, and the items it could not carry.
#[derive(Debug)]
pub struct Carried {
    pub done: Done,
    pub failures: Vec<Failure>,
    pub end: End,
    /// The failed items again, merged into what landed.
    pub left: Option<Op>,
}

impl Carried {
    fn into_done(self) -> io::Result<Done> {
        let cause = self
            .failures
            .into_iter()
            .next()
            .map(|failure| failure.error);
        let left = self.left.unwrap_or(Op::Batch(Vec::new()));
        Unfinished::check(self.done, left, cause)
    }
}

/// Runs a move, copy, duplicate or put back on the transfer engine, whose progress `transfer`
/// reports meanwhile; any other op comes back as it was.
pub fn carry(
    op: Op,
    conflict: Conflict,
    on_error: OnError,
    transfer: &Transfer,
) -> Result<Carried, Op> {
    let (kind, items, into) = match op {
        Op::Move { items, into } => (Kind::Move, items, into),
        Op::Copy { items, into } => (Kind::Copy, items, into),
        Op::CopyExactly { items, into } => (Kind::CopyExactly, items, into),
        Op::Duplicate(items) => return Ok(duplicate(items, on_error, transfer)),
        Op::PutBack(items) => return Ok(put_back(items, transfer)),
        Op::Retry(request) => return Ok(run_all(vec![request], transfer)),
        op => return Err(op),
    };
    let request = Request::new(kind, items, &into, conflict, on_error);
    let outcome = transfer.run(request);
    Ok(Carried {
        left: outcome.retry().map(Op::Retry),
        done: outcome.done,
        failures: outcome.failures,
        end: outcome.end,
    })
}

/// One request per destination volume, which the engine reads once; a destination that cannot
/// be read stands alone, so its refusal names it.
fn by_volume(
    kind: Kind,
    pairs: Vec<(PathBuf, PathBuf)>,
    conflict: Conflict,
    on_error: OnError,
) -> Vec<Request> {
    let mut groups: Vec<(Option<u64>, Vec<(PathBuf, PathBuf)>)> = Vec::new();
    for (source, dest) in pairs {
        let volume = std::fs::metadata(dir_of(&dest)).ok().map(|meta| meta.dev());
        match volume.and_then(|volume| groups.iter().position(|(at, _)| *at == Some(volume))) {
            Some(at) => groups[at].1.push((source, dest)),
            None => groups.push((volume, vec![(source, dest)])),
        }
    }
    groups
        .into_iter()
        .map(|(_, pairs)| Request::each(kind, pairs, conflict, on_error))
        .collect()
}

/// Runs each request in turn on the one handle; with more than one, the undo is the caller's.
fn run_all(requests: Vec<Request>, transfer: &Transfer) -> Carried {
    let mut carried = Carried {
        done: Done::default(),
        failures: Vec::new(),
        end: End::Finished,
        left: None,
    };
    let mut again = Vec::new();
    let mut requests = requests.into_iter();
    for request in requests.by_ref() {
        let outcome = transfer.run(request);
        again.extend(outcome.retry().map(Op::Retry));
        carried.done.results.extend(outcome.done.results);
        carried.done.undo = outcome.done.undo;
        carried.failures.extend(outcome.failures);
        carried.end = outcome.end;
        if outcome.end != End::Finished {
            break;
        }
    }
    if carried.end != End::Cancelled {
        again.extend(requests.map(Op::Retry));
    }
    carried.left = match again.len() {
        0 | 1 => again.pop(),
        _ => Some(Op::Batch(again)),
    };
    carried
}

#[derive(Debug)]
pub struct Unfinished {
    pub done: Done,
    pub left: Op,
    pub cause: io::Error,
}

impl std::fmt::Display for Unfinished {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.cause.fmt(f)
    }
}

impl std::error::Error for Unfinished {}

impl Unfinished {
    fn check(done: Done, left: Op, cause: Option<io::Error>) -> io::Result<Done> {
        match cause {
            None => Ok(done),
            Some(cause) => Err(io::Error::new(
                cause.kind(),
                Unfinished { done, left, cause },
            )),
        }
    }

    pub fn from_error(error: io::Error) -> Result<Unfinished, io::Error> {
        error.downcast()
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

fn name_of(path: &Path) -> Cow<'_, str> {
    path.file_name().unwrap_or_default().to_string_lossy()
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

pub fn walk_size(path: &Path) -> u64 {
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

fn volume_of(path: &Path) -> Option<volumes::Volume> {
    volumes::holding(path).filter(|volume| !volume.name.is_empty())
}

/// `NSFileManager`'s own message names the boot volume whatever the destination was. Without an
/// item it speaks for several.
fn refusal_message(
    refusal: Refusal,
    item: Option<&str>,
    dest: &Path,
    is_move: bool,
    needed: u64,
) -> Option<String> {
    let volume = volume_of(dest)?;
    let name = volume.name;
    Some(match refusal {
        Refusal::ReadOnly => {
            let verb = if is_move { "moved" } else { "copied" };
            let what = match item {
                Some(item) => format!("The item \u{201c}{item}\u{201d}"),
                None => "These items".to_string(),
            };
            format!(
                "{what} can't be {verb} because the volume \u{201c}{name}\u{201d} is read only."
            )
        }
        Refusal::NoSpace => {
            let verb = if is_move { "move" } else { "copy" };
            let what = match item {
                Some(item) => format!("\u{201c}{item}\u{201d}"),
                None => "these items".to_string(),
            };
            let short = match needed.saturating_sub(volume.available) {
                0 => String::new(),
                short => format!(" You need an additional {}.", byte_string(short)),
            };
            format!("There isn't enough space on \u{201c}{name}\u{201d} to {verb} {what}.{short}")
        }
    })
}

/// `NSFileManager` builds an absolute path per item and gives up once one passes `PATH_MAX`.
fn past_path_max(err: &NSError) -> bool {
    let too_long = |err: &NSError| {
        err.domain().to_string() == "NSPOSIXErrorDomain" && err.code() as i32 == libc::ENAMETOOLONG
    };
    err.domain().to_string() == "NSCocoaErrorDomain"
        && err.code() == NSFileWriteInvalidFileNameError
        && err.underlyingErrors().iter().any(|under| too_long(&under))
}

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
            if !same_item(src, dst) {
                let _ = deep::remove(dst);
            }
            deep::transfer(src, dst, is_move).map_err(|err| deep_error(err, src, dst, is_move))
        }
        Err(err) => Err(volume_refusal(&err, src, dst, is_move).unwrap_or_else(|| io_error(err))),
    }
}

fn deep_error(err: io::Error, src: &Path, dest: &Path, is_move: bool) -> io::Error {
    let Some(refused) = err.raw_os_error().and_then(|code| cocoa_error(code, dest)) else {
        return err;
    };
    volume_refusal(&refused, src, dest, is_move).unwrap_or_else(|| io_error(refused))
}

fn cocoa_error(code: i32, path: &Path) -> Option<Retained<NSError>> {
    let cocoa = match code {
        libc::EACCES | libc::EPERM => NSFileWriteNoPermissionError,
        libc::EROFS => NSFileWriteVolumeReadOnlyError,
        libc::ENOSPC => NSFileWriteOutOfSpaceError,
        libc::ENAMETOOLONG => NSFileWriteInvalidFileNameError,
        libc::ENOENT => NSFileNoSuchFileError,
        libc::EEXIST => NSFileWriteFileExistsError,
        _ => return None,
    };
    let named = NSString::from_str(&path.to_string_lossy());
    let named: &AnyObject = &named;
    let info = NSDictionary::from_slices(&[unsafe { NSFilePathErrorKey }], &[named]);
    Some(unsafe { NSError::errorWithDomain_code_userInfo(NSCocoaErrorDomain, cocoa, Some(&info)) })
}

fn volume_refusal(err: &NSError, src: &Path, dest: &Path, is_move: bool) -> Option<io::Error> {
    let message = refusal_message(
        refusal_of(err)?,
        Some(&name_of(src)),
        dest,
        is_move,
        walk_size(src),
    )?;
    Some(io::Error::other(message))
}

fn remove_path(fm: &NSFileManager, path: &Path) -> io::Result<()> {
    fm.removeItemAtURL_error(&url(path)).map_err(io_error)
}

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

/// Finder asks before it clears a lock; `Op::Trash` and `Op::DeleteImmediately` clear it themselves.
pub fn locked(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.st_flags() & libc::UF_IMMUTABLE != 0)
}

/// A locked child refuses a delete of the folder it sits in, but not a trash, which is one rename.
fn unlock(path: &Path, deep: bool) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    let flags = meta.st_flags();
    // `chflags` follows a link, so the lock a link carries itself is left where it is.
    if flags & libc::UF_IMMUTABLE != 0 && !meta.is_symlink() {
        let name = crate::c_path(path)?;
        if unsafe { libc::chflags(name.as_ptr(), flags & !libc::UF_IMMUTABLE) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    if deep && meta.is_dir() {
        for entry in std::fs::read_dir(path)? {
            unlock(&entry?.path(), true)?;
        }
    }
    Ok(())
}

/// Past `PATH_MAX` `lstat` fails, which would read a name that is in use as free.
fn identity(path: &Path) -> Option<(u64, u64)> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => Some((meta.dev(), meta.ino())),
        Err(err) if err.raw_os_error() == Some(libc::ENAMETOOLONG) => deep::identity(path).ok(),
        Err(_) => None,
    }
}

/// A broken symlink still holds its name, which `Path::exists` misses because it follows links.
fn taken(path: &Path) -> bool {
    identity(path).is_some()
}

/// A case-only rename on a case-insensitive volume finds the item itself at the destination.
fn same_item(one: &Path, other: &Path) -> bool {
    matches!((identity(one), identity(other)), (Some(one), Some(other)) if one == other)
}

/// No extension for dot-files or a trailing dot.
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

/// The first of `stem`, `stem 2`, `stem 3`, … not taken in `dir`.
fn first_free(dir: &Path, stem: &str, ext: Option<&str>) -> PathBuf {
    (1..)
        .map(|n| match n {
            1 => dir.join(with_ext(stem, ext)),
            n => dir.join(with_ext(&format!("{stem} {n}"), ext)),
        })
        .find(|path| !taken(path))
        .expect("an endless range")
}

fn dir_of(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new(""))
}

/// `path` itself, or numbered like Keep Both once its name is taken.
fn free_path(path: &Path) -> PathBuf {
    let name = name_of(path);
    let (stem, ext) = split_name(&name);
    first_free(dir_of(path), stem, ext)
}

fn duplicate_path(path: &Path) -> PathBuf {
    let name = name_of(path);
    let (stem, ext) = split_name(&name);
    first_free(dir_of(path), &format!("{stem} copy"), ext)
}

fn alias_path(path: &Path) -> PathBuf {
    first_free(dir_of(path), &format!("{} alias", name_of(path)), None)
}

fn archive_path(items: &[PathBuf]) -> Option<PathBuf> {
    let stem = match items {
        [] => return None,
        [only] => name_of(only),
        _ => Cow::Borrowed("Archive"),
    };
    Some(first_free(dir_of(&items[0]), &stem, Some("zip")))
}

fn staging_path(dest: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = dest.parent().unwrap_or_else(|| Path::new("."));
    dir.join(format!(
        ".finder-fs-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn exchange(staged: &Path, dest: &Path) -> io::Result<()> {
    let (from, to) = (crate::c_path(staged)?, crate::c_path(dest)?);
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

/// Finder carries on past its "the item is locked" question, so a lock is cleared and retried.
fn past_the_lock<T>(
    path: &Path,
    deep: bool,
    refused: io::Error,
    again: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    if refused.kind() != io::ErrorKind::PermissionDenied || unlock(path, deep).is_err() {
        return Err(refused);
    }
    again()
}

fn trash_one(fm: &NSFileManager, path: &Path) -> io::Result<PathBuf> {
    match trash_at(fm, path) {
        Err(refused) => past_the_lock(path, false, refused, || trash_at(fm, path)),
        landed => landed,
    }
}

fn delete_one(fm: &NSFileManager, path: &Path) -> io::Result<()> {
    match remove_path(fm, path) {
        Err(refused) => past_the_lock(path, true, refused, || remove_path(fm, path)),
        gone => gone,
    }
}

/// A Replace swaps the old item out to a staging name, so it goes to the Trash under its own.
fn trash_as(fm: &NSFileManager, staged: &Path, dest: &Path) -> io::Result<PathBuf> {
    let holder = staging_path(dest);
    std::fs::create_dir(&holder)?;
    let named = holder.join(dest.file_name().unwrap_or_default());
    let trashed = std::fs::rename(staged, &named).and_then(|()| {
        trash_one(fm, &named).inspect_err(|_| {
            let _ = std::fs::rename(&named, staged);
        })
    });
    let _ = std::fs::remove_dir(&holder);
    trashed
}

#[derive(Default)]
struct Placed {
    dest: Option<PathBuf>,
    landed: Vec<Restore>,
    displaced: Vec<Restore>,
}

impl Placed {
    fn at(src: &Path, dest: PathBuf) -> Placed {
        Placed {
            landed: vec![Restore {
                item: dest.clone(),
                to: src.to_path_buf(),
            }],
            dest: Some(dest),
            displaced: Vec::new(),
        }
    }
}

fn real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

fn replace_at(fm: &NSFileManager, src: &Path, dest: PathBuf) -> io::Result<Placed> {
    let staged = staging_path(&dest);
    if let Err(err) = transfer(fm, src, &staged, true) {
        let _ = remove_path(fm, &staged);
        return Err(err);
    }
    if let Err(err) = exchange(&staged, &dest) {
        // A moved item's only copy is the staged one; it goes back where it came from.
        let _ = transfer(fm, &staged, src, true);
        return Err(err);
    }
    let displaced = match trash_as(fm, &staged, &dest) {
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
        displaced,
        ..Placed::at(src, dest)
    })
}

/// Moves an item within its volume.
fn place_one(
    fm: &NSFileManager,
    src: &Path,
    dest: PathBuf,
    conflict: Conflict,
) -> io::Result<Placed> {
    if !taken(&dest) {
        transfer(fm, src, &dest, true)?;
        return Ok(Placed::at(src, dest));
    }
    if same_item(src, &dest) {
        return Ok(Placed::at(src, dest));
    }
    match conflict {
        Conflict::Skip => Ok(Placed::default()),
        Conflict::KeepBoth => {
            let dest = free_path(&dest);
            transfer(fm, src, &dest, true)?;
            Ok(Placed::at(src, dest))
        }
        Conflict::Replace => replace_at(fm, src, dest),
        Conflict::Merge if real_dir(src) && real_dir(&dest) => {
            let (mut landed, mut displaced) = (Vec::new(), Vec::new());
            for entry in std::fs::read_dir(src)? {
                let entry = entry?;
                let child_dest = dest.join(entry.file_name());
                let placed = place_one(fm, &entry.path(), child_dest, Conflict::Merge)?;
                landed.extend(placed.landed);
                displaced.extend(placed.displaced);
            }
            std::fs::remove_dir(src)?;
            Ok(Placed {
                dest: Some(dest),
                landed,
                displaced,
            })
        }
        Conflict::Merge => replace_at(fm, src, dest),
    }
}

fn refuse_into_itself(item: &Path, into: &Path, is_move: bool) -> io::Result<()> {
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !real(into).starts_with(real(item)) {
        return Ok(());
    }
    let verb = if is_move { "moved" } else { "copied" };
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "The item \u{201c}{}\u{201d} can't be {verb} into itself.",
            name_of(item)
        ),
    ))
}

fn undo_of_transfer(is_move: bool, home: Vec<Restore>, displaced: Vec<Restore>) -> Option<Op> {
    let landed = (!is_move && !home.is_empty())
        .then(|| Op::Trash(home.iter().map(|placed| placed.item.clone()).collect()));
    let mut put_back = if is_move { home } else { Vec::new() };
    put_back.extend(displaced);
    let restored = (!put_back.is_empty()).then_some(Op::PutBack(put_back));
    match (landed, restored) {
        (Some(landed), Some(restored)) => Some(Op::Batch(vec![landed, restored])),
        (landed, restored) => landed.or(restored),
    }
}

/// Keeps going past an item that refuses, and leaves only what refused to be put back again.
fn put_back(items: Vec<Restore>, transfer: &Transfer) -> Carried {
    for restore in &items {
        let _ = std::fs::create_dir_all(dir_of(&restore.to));
    }
    let (into, beside): (Vec<_>, Vec<_>) = items
        .iter()
        .map(|restore| (restore.item.clone(), restore.to.clone()))
        .partition(|(item, to)| real_dir(item) && real_dir(to) && apart(item, to));
    let merge = by_volume(Kind::Move, into, Conflict::Merge, OnError::Continue);
    let keep = by_volume(Kind::Move, beside, Conflict::KeepBoth, OnError::Continue);
    let mut carried = run_all(merge.into_iter().chain(keep).collect(), transfer);
    let refused = |restore: &Restore| {
        carried
            .failures
            .iter()
            .any(|failure| failure.source.starts_with(&restore.item))
    };
    let left: Vec<Restore> = items.into_iter().filter(refused).collect();
    carried.left = (!left.is_empty()).then_some(Op::PutBack(left));
    carried.done.undo = None;
    carried
}

/// The halves of a move that failed partway share no names but their folders'.
fn apart(item: &Path, into: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(item) else {
        return false;
    };
    entries.flatten().all(|entry| {
        let (here, there) = (entry.path(), into.join(entry.file_name()));
        entry.file_name() == ".DS_Store"
            || !taken(&there)
            || (real_dir(&here) && real_dir(&there) && apart(&here, &there))
    })
}

fn batch(ops: Vec<Op>, conflict: Conflict) -> io::Result<Done> {
    let mut results = Vec::new();
    let (mut left, mut cause) = (Vec::new(), None);
    for op in ops {
        match perform(op.clone(), conflict) {
            Ok(done) => results.extend(done.results),
            Err(error) => match Unfinished::from_error(error) {
                Ok(unfinished) => {
                    results.extend(unfinished.done.results);
                    left.push(unfinished.left);
                    cause.get_or_insert(unfinished.cause);
                }
                Err(error) => {
                    left.push(op);
                    cause.get_or_insert(error);
                }
            },
        }
    }
    let left = match left.len() {
        1 => left.pop().expect("one left"),
        _ => Op::Batch(left),
    };
    let done = Done {
        results,
        undo: None,
    };
    Unfinished::check(done, left, cause)
}

fn trash(items: Vec<PathBuf>) -> io::Result<Done> {
    let fm = NSFileManager::defaultManager();
    let mut home = Vec::with_capacity(items.len());
    let (mut left, mut cause) = (Vec::new(), None);
    for item in items {
        match trash_one(&fm, &item) {
            Ok(trashed) => home.push(Restore {
                item: trashed,
                to: item,
            }),
            Err(error) => {
                cause.get_or_insert(error);
                left.push(item);
            }
        }
    }
    let done = Done {
        results: home.iter().map(|restore| restore.item.clone()).collect(),
        undo: (!home.is_empty()).then_some(Op::PutBack(home)),
    };
    Unfinished::check(done, Op::Trash(left), cause)
}

fn delete(items: Vec<PathBuf>) -> io::Result<Done> {
    let fm = NSFileManager::defaultManager();
    for item in &items {
        delete_one(&fm, item)?;
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
        Placed::at(&path, dest)
    } else {
        place_one(&fm, &path, dest, conflict)?
    };
    let Some(dest) = placed.dest else {
        return Ok(Done::default());
    };
    let back = Op::Rename {
        path: dest.clone(),
        name: name_of(&path).into_owned(),
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

/// Undone by deleting the copies outright, never by a trip to the Trash.
fn duplicate(items: Vec<PathBuf>, on_error: OnError, transfer: &Transfer) -> Carried {
    let pairs = items
        .into_iter()
        .map(|item| {
            let dest = duplicate_path(&item);
            (item, dest)
        })
        .collect();
    let requests = by_volume(Kind::Copy, pairs, Conflict::KeepBoth, on_error);
    let mut carried = run_all(requests, transfer);
    let results = &carried.done.results;
    carried.done.undo = (!results.is_empty()).then(|| Op::DeleteImmediately(results.clone()));
    carried
}

/// `ditto` is what Archive Utility runs; it keeps resource forks and extended attributes.
fn compress(items: Vec<PathBuf>) -> io::Result<Done> {
    let Some(dest) = archive_path(&items) else {
        return Ok(Done::default());
    };
    let mut ditto = std::process::Command::new("/usr/bin/ditto");
    ditto.args(["-c", "-k", "--sequesterRsrc"]);
    let staging = (items.len() > 1).then(|| staging_path(&dest));
    let source = match &staging {
        None => {
            if items[0].is_dir() {
                ditto.arg("--keepParent");
            }
            &items[0]
        }
        Some(staging) => {
            std::fs::create_dir(staging)?;
            let fm = NSFileManager::defaultManager();
            for item in &items {
                let name = item.file_name().unwrap_or_default();
                transfer(&fm, item, &staging.join(name), false)?;
            }
            staging
        }
    };
    let out = ditto.arg(source).arg(&dest).output();
    if let Some(staging) = &staging {
        let _ = std::fs::remove_dir_all(staging);
    }
    let out = out?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&dest);
        return Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(Done {
        results: vec![dest.clone()],
        undo: Some(Op::Trash(vec![dest])),
    })
}

fn make_alias(items: Vec<PathBuf>) -> io::Result<Done> {
    let results = items
        .iter()
        .map(|item| {
            let dest = alias_path(item);
            let bookmark = url(item)
                .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
                    NSURLBookmarkCreationOptions::SuitableForBookmarkFile,
                    None,
                    None,
                )
                .map_err(io_error)?;
            NSURL::writeBookmarkData_toURL_options_error(
                &bookmark,
                &url(&dest),
                NSURLBookmarkFileCreationOptions::default(),
            )
            .map_err(io_error)?;
            Ok(dest)
        })
        .collect::<io::Result<Vec<_>>>()?;
    let undo = (!results.is_empty()).then(|| Op::Trash(results.clone()));
    Ok(Done { results, undo })
}

fn new_item(into: PathBuf, name: String, is_dir: bool) -> io::Result<Done> {
    check_name(&name)?;
    let fm = NSFileManager::defaultManager();
    let dest = free_path(&into.join(&name));
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
    use crate::purge;
    use objc2_foundation::NSURLBookmarkResolutionOptions;
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

    fn displaced(op: &Op) -> Vec<PathBuf> {
        match op {
            Op::PutBack(items) => items.iter().map(|item| item.item.clone()).collect(),
            Op::Batch(ops) => ops.iter().flat_map(displaced).collect(),
            _ => Vec::new(),
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
        )
        .unwrap();
        assert_eq!(done.results, vec![dst.join("a.txt")]);
        assert!(!file.exists(), "move must remove the source");

        let undo = done.undo.expect("a move undoes back to its source folder");
        let back = perform(undo, Conflict::Replace).unwrap();
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
        if !crate::trash_reachable() {
            return;
        }
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
        )
        .unwrap();
        assert!(
            done.undo.is_none(),
            "undoing a skip would trash the source it never touched"
        );
        if let Some(undo) = done.undo {
            perform(undo, Conflict::Replace).unwrap();
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
        if !crate::trash_reachable() {
            return;
        }
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
        )
        .unwrap();
        assert_eq!(done.results, vec![into.join("a.txt")]);
        assert_eq!(fs::read_to_string(into.join("a.txt")).unwrap(), "new");

        let undo = done.undo.expect("a replace can be undone");
        let back = perform(undo, Conflict::Replace).unwrap();
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
        )
        .unwrap();
        let renamed = dir.join("new.txt");
        assert_eq!(done.results, vec![renamed.clone()]);
        assert!(renamed.exists() && !file.exists());

        let undo = done.undo.unwrap();
        let back = perform(undo, Conflict::Replace).unwrap();
        assert_eq!(back.results, vec![file.clone()]);
        assert!(file.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rename_onto_a_taken_name_resolves_like_a_move() {
        if !crate::trash_reachable() {
            return;
        }
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
        )
        .unwrap();
        assert_eq!(done.results, vec![taken.clone()]);
        assert_eq!(fs::read_to_string(&taken).unwrap(), "new");
        assert!(!source.exists());

        let undo = done.undo.expect("a rename that replaced can be undone");
        perform(undo, Conflict::Replace).unwrap();
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
            let err = perform(op, Conflict::KeepBoth).expect_err(wording);
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
        )
        .unwrap();
        assert_eq!(done.results.len(), 2);

        let undo = done.undo.expect("a move from two folders is undoable");
        perform(undo, Conflict::Replace).unwrap();
        assert_eq!(fs::read_to_string(one.join("a.txt")).unwrap(), "a");
        assert_eq!(fs::read_to_string(two.join("b.txt")).unwrap(), "b");
        assert_eq!(fs::read_dir(&into).unwrap().count(), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn put_back_restores_the_name_the_trash_took() {
        if !crate::trash_reachable() {
            return;
        }
        let dir = temp_dir("put-back");
        let file = dir.join("collide.txt");
        touch(&file, "first");
        let first = perform(Op::Trash(vec![file.clone()]), Conflict::Replace).unwrap();

        touch(&file, "second");
        let second = perform(Op::Trash(vec![file.clone()]), Conflict::Replace).unwrap();
        assert_ne!(
            second.results[0].file_name(),
            file.file_name(),
            "the Trash renames what collides in it"
        );

        let undo = second.undo.expect("trash undoes with a put back");
        let back = perform(undo, Conflict::Replace).unwrap();
        assert_eq!(back.results, vec![file.clone()]);
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "second",
            "put back restores the name the item had"
        );

        purge(&first.results);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_never_overwrites_what_arrived_since() {
        let dir = temp_dir("undo-arrived");
        let (from, into) = (dir.join("from"), dir.join("into"));
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&into).unwrap();
        touch(&from.join("a.txt"), "moved");
        let done = perform(
            Op::Move {
                items: vec![from.join("a.txt")],
                into,
            },
            Conflict::KeepBoth,
        )
        .unwrap();
        touch(&from.join("a.txt"), "arrived since");

        perform(done.undo.unwrap(), Conflict::Replace).unwrap();
        assert_eq!(
            fs::read_to_string(from.join("a.txt")).unwrap(),
            "arrived since",
            "undo overwrote a newer file"
        );
        assert_eq!(fs::read_to_string(from.join("a 2.txt")).unwrap(), "moved");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_replace_that_fails_keeps_the_moved_item() {
        let dir = temp_dir("replace-fails-move");
        let (from, into) = (dir.join("from"), dir.join("into"));
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&into).unwrap();
        touch(&from.join("a.txt"), "new");
        touch(&into.join("a.txt"), "old");
        let locked = into.join("a.txt");
        assert!(
            std::process::Command::new("chflags")
                .arg("uchg")
                .arg(&locked)
                .status()
                .unwrap()
                .success()
        );

        let _ = perform(
            Op::Move {
                items: vec![from.join("a.txt")],
                into: into.clone(),
            },
            Conflict::Replace,
        );
        let kept = |path: &Path| fs::read_to_string(path).ok();
        let (left, landed) = (kept(&from.join("a.txt")), kept(&locked));
        let _ = unlock(&locked, true);
        assert!(
            left.as_deref() == Some("new") || landed.as_deref() == Some("new"),
            "the moved item is gone: from={left:?} into={landed:?}"
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    fn deep_path(root: &Path, levels: usize) -> PathBuf {
        (1..=levels).fold(root.to_path_buf(), |path, i| path.join(format!("d{i}")))
    }

    #[test]
    fn a_name_taken_past_path_max_is_kept() {
        let dir = temp_dir("deep-taken");
        let tree = dir.join("Deep");
        deep_tree(&tree, 300);
        deep_sh(&tree, 300, "echo old > x.txt");
        touch(&dir.join("x.txt"), "new");

        perform(
            Op::Move {
                items: vec![dir.join("x.txt")],
                into: deep_path(&tree, 300),
            },
            Conflict::KeepBoth,
        )
        .unwrap();
        assert_eq!(
            deep_sh(&tree, 300, "cat x.txt"),
            "old",
            "the item already there was overwritten"
        );
        assert_eq!(deep_sh(&tree, 300, "cat 'x 2.txt'"), "new");

        remove_deep(&dir);
    }

    #[test]
    fn a_case_only_rename_past_path_max_keeps_the_item() {
        let dir = temp_dir("deep-case");
        let tree = dir.join("Deep");
        deep_tree(&tree, 300);

        perform(
            Op::Rename {
                path: deep_path(&tree, 300).join("leaf.txt"),
                name: "LEAF.txt".into(),
            },
            Conflict::KeepBoth,
        )
        .unwrap();
        assert_eq!(
            deep_sh(&tree, 300, "cat LEAF.txt"),
            "leaf",
            "the renamed item is gone"
        );

        remove_deep(&dir);
    }

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

    /// `remove_dir_all` holds a descriptor per level, which a test lowering the process's limit
    /// alongside takes away; `rm` walks a tree past `PATH_MAX` on a handful.
    fn remove_deep(dir: &Path) {
        let ok = std::process::Command::new("rm")
            .args(["-rf".as_ref(), dir.as_os_str()])
            .status()
            .unwrap();
        assert!(ok.success(), "could not remove the fixture");
    }

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
        )
        .unwrap();
        assert_eq!(deep_sh(&dst.join("Moved"), 300, "cat leaf.txt"), "leaf");
        drop(few);

        remove_deep(&dir);
    }

    #[test]
    fn a_deep_copy_that_fails_keeps_what_landed_for_its_undo() {
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

        let error = perform(
            Op::Copy {
                items: vec![tree.clone()],
                into: dst.clone(),
            },
            Conflict::KeepBoth,
        )
        .expect_err("a directory that cannot be read cannot be copied");
        let unfinished = Unfinished::from_error(error).expect("the error says what landed");
        assert_eq!(unfinished.done.results, vec![dst.join("Deep")]);
        assert!(
            matches!(&unfinished.done.undo, Some(Op::Trash(items)) if items == &[dst.join("Deep")]),
            "what landed is undone whole: {:?}",
            unfinished.done.undo
        );
        deep_sh(&tree, 249, "chmod 700 d250");
        assert_eq!(
            deep_sh(&tree, 300, "cat leaf.txt"),
            "leaf",
            "the source is still whole"
        );

        remove_deep(&dir);
    }

    #[test]
    fn a_deep_refusal_is_worded_like_a_shallow_one() {
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("skipping: root walks into a directory whatever its mode says");
            return;
        }
        let dir = temp_dir("deep-wording");
        let dst = dir.join("dst");
        fs::create_dir_all(&dst).unwrap();
        let tree = dir.join("Deep");
        deep_tree(&tree, 300);
        deep_sh(&tree, 249, "chmod 000 d250");

        let err = perform(
            Op::Copy {
                items: vec![tree.clone()],
                into: dst.clone(),
            },
            Conflict::KeepBoth,
        )
        .expect_err("a directory that cannot be read cannot be copied");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        let message = err.to_string();
        assert!(message.contains("permission"), "{message}");
        assert!(!message.contains("os error"), "{message}");

        deep_sh(&tree, 249, "chmod 700 d250");
        remove_deep(&dir);
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

        let full = refusal_message(Refusal::NoSpace, Some("m.txt"), &dir, false, u64::MAX)
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
        if !crate::trash_reachable() {
            return;
        }
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

        let done = perform(Op::Trash(vec![file.clone()]), Conflict::Replace).unwrap();
        assert!(!taken(&file), "the locked item left its folder");
        let undo = done.undo.expect("a trash undoes with a put back");
        let back = perform(undo, Conflict::Replace).unwrap();
        assert_eq!(back.results, vec![file.clone()]);

        let folder = dir.join("Folder");
        fs::create_dir_all(&folder).unwrap();
        touch(&folder.join("child.txt"), "child");
        lock(&folder.join("child.txt"));
        perform(
            Op::DeleteImmediately(vec![folder.clone()]),
            Conflict::Replace,
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

        let done = perform(Op::Duplicate(vec![file.clone()]), Conflict::Replace).unwrap();
        assert_eq!(done.results, vec![dir.join("doc copy.txt")]);

        let again = perform(Op::Duplicate(vec![file.clone()]), Conflict::Replace).unwrap();
        assert_eq!(again.results, vec![dir.join("doc copy 2.txt")]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn undoing_a_duplicate_deletes_the_copy_without_a_trip_to_the_trash() {
        let dir = temp_dir("duplicate-undo");
        let file = dir.join("ngx-doc.txt");
        touch(&file, "content");
        let copy = dir.join("ngx-doc copy.txt");

        let done = perform(Op::Duplicate(vec![file.clone()]), Conflict::Replace).unwrap();
        assert_eq!(done.results, vec![copy.clone()]);
        let undo = done.undo.expect("a duplicate can be undone");
        let back = perform(undo, Conflict::Replace).unwrap();

        assert!(!copy.exists(), "the copy is gone");
        assert_eq!(back.results, vec![copy], "nothing landed in the Trash");
        assert!(back.undo.is_none(), "there is no Trash record to put back");
        assert!(file.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    fn member_groups() -> Vec<u32> {
        let mut groups = vec![0; 64];
        let count = unsafe { libc::getgroups(groups.len() as i32, groups.as_mut_ptr()) };
        groups.truncate(count.max(0) as usize);
        groups
    }

    #[test]
    fn a_paste_exactly_keeps_the_group_a_plain_copy_takes_from_its_folder() {
        let dir = temp_dir("exactly");
        let (from, exact, plain) = (dir.join("from"), dir.join("exact"), dir.join("plain"));
        for folder in [&from, &exact, &plain] {
            fs::create_dir_all(folder).unwrap();
        }
        let file = from.join("doc.txt");
        touch(&file, "content");
        let fresh = fs::metadata(&plain).unwrap().gid();
        let other = member_groups()
            .into_iter()
            .find(|gid| *gid != fresh)
            .expect("the test user belongs to a second group");
        std::os::unix::fs::lchown(&file, None, Some(other)).unwrap();

        let kept = perform(
            Op::CopyExactly {
                items: vec![file.clone()],
                into: exact.clone(),
            },
            Conflict::KeepBoth,
        )
        .unwrap();
        let taken = perform(
            Op::Copy {
                items: vec![file.clone()],
                into: plain.clone(),
            },
            Conflict::KeepBoth,
        )
        .unwrap();

        assert_eq!(fs::metadata(&kept.results[0]).unwrap().gid(), other);
        assert_eq!(fs::metadata(&taken.results[0]).unwrap().gid(), fresh);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_alias_is_a_bookmark_file_beside_its_target() {
        let dir = temp_dir("alias");
        let file = dir.join("doc.txt");
        touch(&file, "content");

        let done = perform(Op::MakeAlias(vec![file.clone()]), Conflict::Replace).unwrap();
        assert_eq!(done.results, vec![dir.join("doc.txt alias")]);
        let alias = &done.results[0];
        assert!(fs::symlink_metadata(alias).unwrap().is_file());
        let resolved = NSURL::URLByResolvingAliasFileAtURL_options_error(
            &url(alias),
            NSURLBookmarkResolutionOptions::WithoutUI,
        )
        .unwrap();
        assert_eq!(path_from(&resolved), file.canonicalize().unwrap());

        let again = perform(Op::MakeAlias(vec![file]), Conflict::Replace).unwrap();
        assert_eq!(again.results, vec![dir.join("doc.txt alias 2")]);

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
        )
        .unwrap();
        assert_eq!(again.results, vec![dir.join("untitled folder 2")]);

        let file = perform(
            Op::NewFile {
                into: dir.clone(),
                name: "untitled.txt".into(),
            },
            Conflict::Replace,
        )
        .unwrap();
        assert!(dir.join("untitled.txt").is_file());
        assert_eq!(fs::metadata(&file.results[0]).unwrap().len(), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trash_and_put_back() {
        if !crate::trash_reachable() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("finder-fs-ops-trash-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("throwaway.txt");
        touch(&file, "gone soon");

        let done = perform(Op::Trash(vec![file.clone()]), Conflict::Replace).unwrap();
        assert!(!file.exists(), "the item left its folder");
        assert!(done.results[0].to_string_lossy().contains(".Trash"));

        let undo = done.undo.expect("trash undoes with a move back");
        let back = perform(undo, Conflict::Replace).unwrap();
        assert!(file.exists(), "put back restores the original path");
        assert_eq!(back.results, vec![file.clone()]);

        let _ = fs::remove_file(&file);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compresses_a_file_the_way_finder_names_it() {
        let dir = temp_dir("compress-file");
        let file = dir.join("sample.txt");
        std::fs::write(&file, b"x").unwrap();
        let done = perform(Op::Compress(vec![file]), Conflict::KeepBoth).unwrap();
        let zip = dir.join("sample.txt.zip");
        assert_eq!(done.results, vec![zip.clone()]);
        assert!(zip.is_file());
        let listing = std::process::Command::new("/usr/bin/unzip")
            .args(["-Z1"])
            .arg(&zip)
            .output()
            .unwrap();
        let names = String::from_utf8_lossy(&listing.stdout);
        assert!(names.lines().any(|line| line == "sample.txt"), "{names}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compresses_a_selection_into_one_archive() {
        let dir = temp_dir("compress-many");
        let file = dir.join("a.txt");
        let folder = dir.join("b");
        std::fs::write(&file, b"x").unwrap();
        std::fs::create_dir(&folder).unwrap();
        let done = perform(Op::Compress(vec![file, folder]), Conflict::KeepBoth).unwrap();
        assert_eq!(done.results, vec![dir.join("Archive.zip")]);
        let listing = std::process::Command::new("/usr/bin/unzip")
            .args(["-Z1"])
            .arg(dir.join("Archive.zip"))
            .output()
            .unwrap();
        let names = String::from_utf8_lossy(&listing.stdout);
        assert!(names.lines().any(|line| line == "a.txt"), "{names}");
        assert!(names.lines().any(|line| line.starts_with("b/")), "{names}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn undoing_a_merge_leaves_what_was_already_there() {
        if !crate::trash_reachable() {
            return;
        }
        for is_move in [false, true] {
            let dir = temp_dir(if is_move {
                "merge-undo-move"
            } else {
                "merge-undo-copy"
            });
            let src = dir.join("src").join("Folder");
            let dst_parent = dir.join("dest");
            let dst = dst_parent.join("Folder");
            fs::create_dir_all(src.join("nested")).unwrap();
            fs::create_dir_all(dst.join("nested")).unwrap();
            touch(&src.join("keep.txt"), "from src");
            touch(&src.join("nested").join("child.txt"), "child");
            touch(&dst.join("keep.txt"), "from dst");
            touch(&dst.join("only-in-dst.txt"), "untouched");

            let items = vec![src.clone()];
            let into = dst_parent.clone();
            let op = match is_move {
                true => Op::Move { items, into },
                false => Op::Copy { items, into },
            };
            let done = perform(op, Conflict::Merge).unwrap();
            let back = perform(done.undo.expect("a merge undoes"), Conflict::KeepBoth).unwrap();

            let read = |path: PathBuf| fs::read_to_string(path).ok();
            assert_eq!(
                read(dst.join("only-in-dst.txt")).as_deref(),
                Some("untouched")
            );
            assert_eq!(read(dst.join("keep.txt")).as_deref(), Some("from dst"));
            assert!(!dst.join("nested").join("child.txt").exists());
            if is_move {
                assert_eq!(read(src.join("keep.txt")).as_deref(), Some("from src"));
                assert_eq!(
                    read(src.join("nested").join("child.txt")).as_deref(),
                    Some("child")
                );
            }
            purge(
                &back
                    .results
                    .into_iter()
                    .filter(|p| !p.starts_with(&dir))
                    .collect::<Vec<_>>(),
            );
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_failed_compress_is_an_error() {
        let dir = temp_dir("compress-fails");
        let folder = dir.join("Unreadable");
        fs::create_dir_all(&folder).unwrap();
        let secret = folder.join("secret.txt");
        touch(&secret, "secret");
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).unwrap();

        let done = perform(Op::Compress(vec![folder.clone()]), Conflict::KeepBoth);
        assert!(done.is_err(), "ditto failed, so the archive is not done");
        assert!(
            !dir.join("Unreadable.zip").exists(),
            "no partial archive is left"
        );

        fs::set_permissions(&secret, fs::Permissions::from_mode(0o644)).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_replaced_item_goes_to_the_trash_under_its_own_name() {
        if !crate::trash_reachable() {
            return;
        }
        let dir = temp_dir("replaced-name");
        let name = format!("replaced-{}.txt", std::process::id());
        fs::create_dir_all(dir.join("a")).unwrap();
        fs::create_dir_all(dir.join("b")).unwrap();
        touch(&dir.join("a").join(&name), "new");
        touch(&dir.join("b").join(&name), "old");

        let done = perform(
            Op::Copy {
                items: vec![dir.join("a").join(&name)],
                into: dir.join("b"),
            },
            Conflict::Replace,
        )
        .unwrap();
        let trashed = displaced(&done.undo.unwrap());
        assert_eq!(trashed.len(), 1);
        assert_eq!(trashed[0].file_name().unwrap().to_string_lossy(), name);
        assert_eq!(fs::read_to_string(&trashed[0]).unwrap(), "old");

        purge(&trashed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_never_follows_a_link_into_its_target() {
        if !crate::trash_reachable() {
            return;
        }
        let dir = temp_dir("merge-link");
        let elsewhere = dir.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        touch(&elsewhere.join("mine.txt"), "mine");
        let src = dir.join("src").join("Folder");
        fs::create_dir_all(&src).unwrap();
        touch(&src.join("keep.txt"), "from src");
        fs::create_dir_all(dir.join("dest")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.join("dest").join("Folder")).unwrap();

        let done = perform(
            Op::Copy {
                items: vec![src.clone()],
                into: dir.join("dest"),
            },
            Conflict::Merge,
        )
        .unwrap();
        assert!(
            !elsewhere.join("keep.txt").exists(),
            "the link's target is untouched"
        );
        assert!(
            !fs::symlink_metadata(dir.join("dest").join("Folder"))
                .unwrap()
                .is_symlink()
        );

        purge(&displaced(&done.undo.unwrap()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trashing_a_locked_folder_keeps_the_locks_inside() {
        if !crate::trash_reachable() {
            return;
        }
        let dir = temp_dir("locked-folder");
        let folder = dir.join("LockedFolder");
        fs::create_dir_all(&folder).unwrap();
        let child = folder.join("child.txt");
        touch(&child, "child");
        for path in [&child, &folder] {
            assert!(
                std::process::Command::new("/usr/bin/chflags")
                    .arg("uchg")
                    .arg(path)
                    .status()
                    .unwrap()
                    .success()
            );
        }

        let done = perform(Op::Trash(vec![folder.clone()]), Conflict::Replace).unwrap();
        perform(done.undo.unwrap(), Conflict::KeepBoth).unwrap();
        assert!(locked(&child), "the child's own lock survived the trip");

        let _ = std::process::Command::new("/usr/bin/chflags")
            .args(["-R", "nouchg"])
            .arg(&dir)
            .status();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_put_back_keeps_going_past_an_item_that_refuses() {
        if !crate::trash_reachable() {
            return;
        }
        let dir = temp_dir("put-back-partial");
        let kept = dir.join("kept.txt");
        touch(&kept, "kept");
        let trashed = perform(Op::Trash(vec![kept.clone()]), Conflict::Replace).unwrap();
        let gone = Restore {
            item: dir.join("gone.txt"),
            to: dir.join("gone.txt"),
        };
        let home = Restore {
            item: trashed.results[0].clone(),
            to: kept.clone(),
        };
        let back = perform(Op::PutBack(vec![gone, home]), Conflict::KeepBoth);
        assert!(back.is_err(), "the item that refused is still reported");
        assert_eq!(
            fs::read_to_string(&kept).ok().as_deref(),
            Some("kept"),
            "the item after the one that refused still went back"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unfinished_put_back_names_only_what_refused() {
        if !crate::trash_reachable() {
            return;
        }
        let dir = temp_dir("put-back-left");
        let kept = dir.join("kept.txt");
        touch(&kept, "kept");
        let trashed = perform(Op::Trash(vec![kept.clone()]), Conflict::Replace).unwrap();
        let gone = Restore {
            item: dir.join("gone.txt"),
            to: dir.join("gone.txt"),
        };
        let home = Restore {
            item: trashed.results[0].clone(),
            to: kept.clone(),
        };
        let error = perform(Op::PutBack(vec![gone, home]), Conflict::KeepBoth).unwrap_err();
        let unfinished = Unfinished::from_error(error).expect("the error says what is left");
        assert_eq!(unfinished.done.results, vec![kept.clone()]);
        let Op::PutBack(left) = unfinished.left else {
            panic!("a put back leaves a put back");
        };
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].to, dir.join("gone.txt"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_put_back_beside_what_its_move_left_merges_into_it() {
        let dir = temp_dir("put-back-merge");
        let (left, landed) = (dir.join("Folder"), dir.join("dest").join("Folder"));
        for folder in [left.join("inner"), landed.join("inner")] {
            fs::create_dir_all(folder).unwrap();
        }
        touch(&left.join("inner").join("kept.txt"), "kept");
        touch(&landed.join("moved.txt"), "moved");
        touch(&landed.join("inner").join("also.txt"), "also");
        let restore = Restore {
            item: landed.clone(),
            to: left.clone(),
        };
        perform(Op::PutBack(vec![restore]), Conflict::KeepBoth).unwrap();
        for (name, text) in [
            ("moved.txt", "moved"),
            ("inner/also.txt", "also"),
            ("inner/kept.txt", "kept"),
        ] {
            let read = fs::read_to_string(left.join(name)).ok();
            assert_eq!(read.as_deref(), Some(text), "{name}");
        }
        assert!(!landed.exists(), "the carried half stayed");
        assert!(!dir.join("Folder 2").exists(), "the halves stayed apart");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_put_back_over_a_namesake_keeps_both() {
        let dir = temp_dir("put-back-both");
        let (there, back) = (dir.join("Folder"), dir.join("dest").join("Folder"));
        for folder in [&there, &back] {
            fs::create_dir_all(folder).unwrap();
        }
        touch(&there.join("same.txt"), "there");
        touch(&back.join("same.txt"), "back");
        let restore = Restore {
            item: back,
            to: there.clone(),
        };
        perform(Op::PutBack(vec![restore]), Conflict::KeepBoth).unwrap();
        assert_eq!(fs::read_to_string(there.join("same.txt")).unwrap(), "there");
        let kept = fs::read_to_string(dir.join("Folder 2").join("same.txt")).unwrap();
        assert_eq!(kept, "back");
        let _ = fs::remove_dir_all(&dir);
    }
}
