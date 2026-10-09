use std::ffi::{CStr, c_char, c_int, c_void};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd as _;
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;

use objc2_foundation::NSFileManager;
use parking_lot::Mutex;

use super::{Job, Kind, Slot, Task, What};
use crate::ops::{Conflict, deep, exchange, free_path, name_of, staging_path, taken, trash_as};

const LOCKS: u32 = libc::UF_IMMUTABLE | libc::UF_APPEND;
const ALL: u32 = libc::COPYFILE_METADATA | libc::COPYFILE_DATA;

/// What a copy carries to a volume without native attributes; anything more only adds a `._` file.
const KEPT: [&[u8]; 4] = [
    b"com.apple.ResourceFork",
    b"com.apple.FinderInfo",
    b"com.apple.metadata:_kMDItemUserTags",
    b"com.apple.metadata:kMDItemFinderComment",
];

pub(super) fn work(job: &Job, queue: &Mutex<Receiver<Task>>, slot: &Slot) {
    loop {
        let next = queue.lock().recv();
        let Ok(task) = next else {
            return;
        };
        run(job, task, slot);
    }
}

fn run(job: &Job, task: Task, slot: &Slot) {
    let ready = task.parent.wait(job.shared);
    if !ready || job.halted() {
        if let What::Dir(gate) = &task.what {
            gate.settle(false, u32::MAX);
        }
        return job.forget(&task, 0);
    }
    if !matches!(task.what, What::File) || task.clone {
        job.shared.wait_while_paused();
    }
    let done = match &task.what {
        What::Dir(gate) => make_dir(job, &task, gate),
        What::File => copy_file(job, &task, slot),
        What::Link => copy_link(job, &task),
        What::Deep => deep::transfer(&task.source, &task.dest, job.is_move())
            .map(|()| job.landed(&task, &task.dest)),
    };
    let partial = slot.done.swap(0, Ordering::Relaxed);
    match done {
        Ok(()) => job.item_done(),
        Err(error) => {
            if !job.halted() || error.raw_os_error() != Some(libc::ECANCELED) {
                job.fail(&task.source, &task.dest, error);
            }
            job.forget(&task, partial);
        }
    }
}

fn make_dir(job: &Job, task: &Task, gate: &super::Gate) -> io::Result<()> {
    if let Err(error) = fs::DirBuilder::new().mode(0o700).create(&task.dest) {
        gate.settle(false, u32::MAX);
        return Err(error);
    }
    job.made(task);
    gate.settle(true, task.parent.gid());
    Ok(())
}

fn copy_file(job: &Job, task: &Task, slot: &Slot) -> io::Result<()> {
    if task.clone && !task.over && clone(&task.source, &task.dest)? {
        if job.kind == Kind::CopyExactly {
            keep_owner(&task.source, &task.dest);
        }
        job.bytes_done(task.size);
        job.landed(task, &task.dest);
        return Ok(());
    }
    let temp = staging_path(&task.dest);
    *slot.file.lock() = Some((task.source.clone(), task.size));
    job.claim(task);
    let copied = copy_into(job, task, &temp, slot);
    *slot.file.lock() = None;
    let landed = copied.and_then(|relock| Ok((relock, commit(job, task, &temp)?)));
    let (relock, at) = match landed {
        Ok(landed) => landed,
        Err(error) => {
            let _ = fs::remove_file(&temp);
            job.unclaim(task);
            return Err(error);
        }
    };
    slot.done.store(0, Ordering::Relaxed);
    let Some(at) = at else {
        return Ok(());
    };
    if let Some(flags) = relock {
        let _ = set_flags(&at, flags);
    }
    job.landed(task, &at);
    job.release(task, &at);
    Ok(())
}

/// Atomic in the kernel, so a clone goes straight to its name.
fn clone(source: &Path, dest: &Path) -> io::Result<bool> {
    let (from, to) = (crate::c_path(source)?, crate::c_path(dest)?);
    let flags = libc::COPYFILE_CLONE_FORCE | libc::COPYFILE_NOFOLLOW;
    if unsafe { libc::copyfile(from.as_ptr(), to.as_ptr(), std::ptr::null_mut(), flags) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENOTSUP | libc::EXDEV | libc::EPERM) => Ok(false),
        _ => Err(error),
    }
}

/// FAT and ExFAT keep no owner, mode or ACL, and every metadata call is a round trip to them.
fn lean(job: &Job) -> bool {
    !job.volume.xattrs && !job.volume.network
}

fn attributes(job: &Job, source: &File) -> u32 {
    match lean(job) {
        false => libc::COPYFILE_METADATA,
        true if has_kept(source) => libc::COPYFILE_XATTR,
        true => 0,
    }
}

fn has_kept(source: &File) -> bool {
    let mut names = [0u8; 1024];
    let fd = source.as_raw_fd();
    let len = unsafe { libc::flistxattr(fd, names.as_mut_ptr().cast(), names.len(), 0) };
    let Ok(len) = usize::try_from(len) else {
        return true;
    };
    names[..len]
        .split(|&byte| byte == 0)
        .any(|name| KEPT.contains(&name))
}

/// Copies into `temp` and checks that every byte arrived; returns the lock flags to set once
/// the copy has its name, since a locked file cannot be renamed.
fn copy_into(job: &Job, task: &Task, temp: &Path, slot: &Slot) -> io::Result<Option<u32>> {
    let source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&task.source)?;
    let before = source.metadata()?;
    let out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(temp)?;
    let mut status = Status {
        job,
        slot,
        filter: !job.volume.xattrs,
        copied: 0,
    };
    let carry = libc::COPYFILE_DATA | attributes(job, &source);
    let state = State::new(&mut status)?;
    let copied = unsafe { libc::fcopyfile(source.as_raw_fd(), out.as_raw_fd(), state.0, carry) };
    let error = io::Error::last_os_error();
    drop(state);
    if copied != 0 {
        return Err(match job.halted() {
            true => io::Error::from_raw_os_error(libc::ECANCELED),
            false => error,
        });
    }
    let after = source.metadata()?;
    if (after.len(), after.mtime(), after.mtime_nsec())
        != (before.len(), before.mtime(), before.mtime_nsec())
    {
        let why = "it changed while it was being copied";
        return Err(because(job, &task.source, io::ErrorKind::InvalidData, why));
    }
    let written = out.metadata()?;
    if written.len() != before.len() || status.copied != before.len() {
        let why = "not all of it could be written";
        return Err(because(job, &task.source, io::ErrorKind::InvalidData, why));
    }
    if lean(job) {
        set_times(&out, &before);
        // Not the staging file's flags: FAT and ExFAT hide any dot-named file.
        let locks = before.st_flags() & LOCKS;
        return Ok((locks != 0).then_some(locks));
    }
    if job.kind == Kind::Copy {
        regroup(&out, task.parent.gid());
    }
    if job.is_move() && job.volume.network {
        out.sync_all()?;
    }
    let flags = written.st_flags();
    if flags & LOCKS == 0 {
        return Ok(None);
    }
    crate::check(unsafe { libc::fchflags(out.as_raw_fd(), flags & !LOCKS) })?;
    Ok(Some(flags))
}

/// Gives `temp` its name, or for a replace swaps it with what was there and trashes that.
fn commit(job: &Job, task: &Task, temp: &Path) -> io::Result<Option<PathBuf>> {
    if task.over {
        return replace(job, temp, &task.dest).map(|()| Some(task.dest.clone()));
    }
    match rename_new(temp, &task.dest) {
        Ok(()) => Ok(Some(task.dest.clone())),
        Err(error) if error.raw_os_error() != Some(libc::EEXIST) => Err(error),
        Err(_) => match job.conflict {
            Conflict::Skip => {
                let _ = deep::remove(temp);
                Ok(None)
            }
            Conflict::KeepBoth => {
                let free = free_path(&task.dest);
                rename_new(temp, &free).map(|()| Some(free))
            }
            Conflict::Replace | Conflict::Merge => {
                replace(job, temp, &task.dest).map(|()| Some(task.dest.clone()))
            }
        },
    }
}

fn replace(job: &Job, temp: &Path, dest: &Path) -> io::Result<()> {
    exchange(temp, dest)?;
    match trash_as(&NSFileManager::defaultManager(), temp, dest) {
        Ok(item) => job.displaced(item, dest),
        Err(_) => {
            let _ = deep::remove(temp);
        }
    }
    Ok(())
}

/// Never over a name taken since the walk; where the volume cannot refuse that itself, it is
/// checked just before.
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    let (a, b) = (crate::c_path(from)?, crate::c_path(to)?);
    if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_EXCL) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if !matches!(error.raw_os_error(), Some(libc::ENOTSUP | libc::EINVAL)) {
        return Err(error);
    }
    if taken(to) {
        return Err(io::Error::from_raw_os_error(libc::EEXIST));
    }
    fs::rename(from, to)
}

fn copy_link(job: &Job, task: &Task) -> io::Result<()> {
    match task.over {
        false => make_link(job, &task.source, &task.dest)?,
        true => {
            let temp = staging_path(&task.dest);
            let placed =
                make_link(job, &task.source, &temp).and_then(|()| replace(job, &temp, &task.dest));
            if let Err(error) = placed {
                let _ = fs::remove_file(&temp);
                return Err(error);
            }
        }
    }
    job.landed(task, &task.dest);
    job.release(task, &task.dest);
    Ok(())
}

/// Folders take their metadata last, once nothing more is written into them.
pub(super) fn settle_dir(job: &Job, source: &Path, dest: &Path) -> io::Result<()> {
    let open = |path: &Path| {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
    };
    let (from, into) = (open(source)?, open(dest)?);
    let carry = attributes(job, &from);
    if carry != 0 {
        let mut status = Status {
            job,
            slot: &Slot::default(),
            filter: !job.volume.xattrs,
            copied: 0,
        };
        let state = State::new(&mut status)?;
        crate::check(unsafe {
            libc::fcopyfile(from.as_raw_fd(), into.as_raw_fd(), state.0, carry)
        })?;
    }
    if lean(job) {
        set_times(&into, &from.metadata()?);
    } else if job.kind == Kind::Copy {
        let parent = dest.parent().and_then(|parent| fs::metadata(parent).ok());
        regroup(&into, parent.map_or(u32::MAX, |meta| meta.gid()));
    }
    Ok(())
}

/// `copyfile` makes the link itself and carries its attributes through the same filter.
fn make_link(job: &Job, source: &Path, dest: &Path) -> io::Result<()> {
    let (from, to) = (crate::c_path(source)?, crate::c_path(dest)?);
    let mut status = Status {
        job,
        slot: &Slot::default(),
        filter: !job.volume.xattrs,
        copied: 0,
    };
    let state = State::new(&mut status)?;
    let flags = ALL | libc::COPYFILE_NOFOLLOW | libc::COPYFILE_EXCL;
    crate::check(unsafe { libc::copyfile(from.as_ptr(), to.as_ptr(), state.0, flags) })
}

/// A plain copy belongs to the group of the folder it lands in, as a new file does.
fn regroup(file: &File, gid: u32) {
    if gid != u32::MAX {
        unsafe { libc::fchown(file.as_raw_fd(), u32::MAX, gid) };
    }
}

/// Only root may give an item away; any user may pass on a group they belong to.
fn keep_owner(source: &Path, dest: &Path) {
    let Ok(meta) = fs::symlink_metadata(source) else {
        return;
    };
    if std::os::unix::fs::lchown(dest, Some(meta.uid()), Some(meta.gid())).is_err() {
        let _ = std::os::unix::fs::lchown(dest, None, Some(meta.gid()));
    }
}

fn set_flags(path: &Path, flags: u32) -> io::Result<()> {
    let path = crate::c_path(path)?;
    crate::check(unsafe { libc::chflags(path.as_ptr(), flags) })
}

fn set_times(file: &File, times: &fs::Metadata) {
    let mut wanted = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: libc::ATTR_CMN_CRTIME | libc::ATTR_CMN_MODTIME | libc::ATTR_CMN_ACCTIME,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let at = |tv_sec, tv_nsec| libc::timespec { tv_sec, tv_nsec };
    let mut values = [
        at(times.st_birthtime(), times.st_birthtime_nsec()),
        at(times.mtime(), times.mtime_nsec()),
        at(times.atime(), times.atime_nsec()),
    ];
    unsafe {
        libc::fsetattrlist(
            file.as_raw_fd(),
            (&mut wanted as *mut libc::attrlist).cast(),
            values.as_mut_ptr().cast(),
            std::mem::size_of_val(&values),
            0,
        )
    };
}

struct Status<'s, 'j, 'a> {
    job: &'j Job<'a>,
    slot: &'s Slot,
    filter: bool,
    copied: u64,
}

struct State(libc::copyfile_state_t);

impl State {
    fn new(status: &mut Status) -> io::Result<State> {
        let state = unsafe { libc::copyfile_state_alloc() };
        if state.is_null() {
            return Err(io::Error::last_os_error());
        }
        let callback: extern "C" fn(
            c_int,
            c_int,
            libc::copyfile_state_t,
            *const c_char,
            *const c_char,
            *mut c_void,
        ) -> c_int = status_callback;
        let context: *mut Status = status;
        unsafe {
            libc::copyfile_state_set(
                state,
                libc::COPYFILE_STATE_STATUS_CB as u32,
                callback as *const c_void,
            );
            libc::copyfile_state_set(
                state,
                libc::COPYFILE_STATE_STATUS_CTX as u32,
                context.cast_const().cast(),
            );
        }
        Ok(State(state))
    }
}

impl Drop for State {
    fn drop(&mut self) {
        unsafe { libc::copyfile_state_free(self.0) };
    }
}

/// Runs on the copying thread for every chunk written, which is where a pause holds and a
/// cancel lands.
extern "C" fn status_callback(
    what: c_int,
    stage: c_int,
    state: libc::copyfile_state_t,
    _: *const c_char,
    _: *const c_char,
    context: *mut c_void,
) -> c_int {
    // SAFETY: the context is the `Status` that outlives the `copyfile` call it was set for.
    let status = unsafe { &mut *context.cast::<Status>() };
    match (what, stage) {
        (libc::COPYFILE_COPY_DATA, libc::COPYFILE_PROGRESS) => {
            let mut copied: libc::off_t = 0;
            let at = (&mut copied as *mut libc::off_t).cast();
            unsafe { libc::copyfile_state_get(state, libc::COPYFILE_STATE_COPIED as u32, at) };
            let copied = copied.max(0) as u64;
            status.job.bytes_done(copied.saturating_sub(status.copied));
            status.copied = copied;
            status.slot.done.store(copied, Ordering::Relaxed);
            status.job.shared.wait_while_paused();
            match status.job.halted() {
                true => libc::COPYFILE_QUIT,
                false => libc::COPYFILE_CONTINUE,
            }
        }
        // Answering a failed write with CONTINUE makes copyfile try it again, forever on a full
        // disk; an attribute that will not take is left out rather than costing the file.
        (libc::COPYFILE_COPY_DATA, libc::COPYFILE_ERR) => libc::COPYFILE_QUIT,
        (libc::COPYFILE_COPY_XATTR, libc::COPYFILE_ERR) => libc::COPYFILE_SKIP,
        (libc::COPYFILE_COPY_XATTR, libc::COPYFILE_START) if status.filter => {
            let mut name: *const c_char = std::ptr::null();
            let at = (&mut name as *mut *const c_char).cast();
            unsafe { libc::copyfile_state_get(state, libc::COPYFILE_STATE_XATTRNAME as u32, at) };
            // SAFETY: copyfile hands over a NUL-terminated name for as long as the callback runs.
            let kept =
                !name.is_null() && KEPT.contains(&unsafe { CStr::from_ptr(name) }.to_bytes());
            match kept {
                true => libc::COPYFILE_CONTINUE,
                false => libc::COPYFILE_SKIP,
            }
        }
        _ => libc::COPYFILE_CONTINUE,
    }
}

/// Worded the way Finder words it, naming the item rather than the call that failed; an error
/// that is already a sentence passes through.
pub(super) fn worded(error: io::Error, source: &Path, dest: &Path, job: &Job) -> io::Error {
    let Some(code) = error.raw_os_error() else {
        return error;
    };
    let item = name_of(source);
    let refusal = match code {
        libc::ENOSPC | libc::EDQUOT => job.no_space_message(&item, dest),
        libc::EROFS => job.read_only_message(&item, dest),
        _ => None,
    };
    if let Some(message) = refusal {
        return io::Error::new(error.kind(), message);
    }
    let why = match code {
        libc::EFBIG => "it's too large for the volume's format".to_string(),
        libc::EACCES | libc::EPERM => "you don't have permission".to_string(),
        libc::ENOENT => "it can't be found".to_string(),
        libc::ENAMETOOLONG => "its name is too long".to_string(),
        code => reason(code),
    };
    because(job, source, error.kind(), &why)
}

pub(super) fn because(job: &Job, source: &Path, kind: io::ErrorKind, why: &str) -> io::Error {
    let verb = if job.is_move() { "moved" } else { "copied" };
    let item = name_of(source);
    io::Error::new(
        kind,
        format!("The item \u{201c}{item}\u{201d} can't be {verb} because {why}."),
    )
}

pub(super) fn not_removed(error: io::Error, source: &Path) -> io::Error {
    let item = name_of(source);
    let why = match error.raw_os_error() {
        Some(libc::EPERM) => "it's locked".to_string(),
        Some(libc::EACCES) => "you don't have permission".to_string(),
        Some(code) => reason(code),
        None => error.to_string(),
    };
    io::Error::new(
        error.kind(),
        format!(
            "\u{201c}{item}\u{201d} was copied, but the original can't be removed because {why}."
        ),
    )
}

fn reason(code: i32) -> String {
    // SAFETY: strerror returns a static, NUL-terminated message.
    let text = unsafe { CStr::from_ptr(libc::strerror(code)) };
    let text = text.to_string_lossy();
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}
