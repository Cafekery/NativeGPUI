use std::borrow::Cow;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd as _;
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::fs::{MetadataExt as _, lchown};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;

use objc2_foundation::NSFileManager;
use parking_lot::Mutex;

use super::{Gate, Job, Kind, Slot, Task, What};
use crate::ops::{
    Refusal, deep, drop_slot, exchange, land, name_of, replace_slot, staging_path, taken,
    trash_slot,
};

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
        let Ok(task) = queue.lock().recv() else {
            return;
        };
        run(job, task, slot);
    }
}

fn run(job: &Job, task: Task, slot: &Slot) {
    if !task.parent.wait(job.shared) || job.halted() {
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

fn make_dir(job: &Job, task: &Task, gate: &Gate) -> io::Result<()> {
    if let Err(error) = deep::create_dir(&task.dest, 0o700) {
        gate.settle(false, u32::MAX);
        return Err(error);
    }
    job.made(task);
    gate.settle(true, task.parent.gid());
    Ok(())
}

fn copy_file(job: &Job, task: &Task, slot: &Slot) -> io::Result<()> {
    if task.clone && !task.over {
        let next = || job.next_free(task);
        let (at, cloned) = land(task.dest.clone(), next, |at| clone(&task.source, at))?;
        if cloned {
            if job.kind == Kind::CopyExactly {
                keep_owner(&task.source, &at);
            }
            job.bytes_done(task.size);
            job.landed(task, &at);
            return Ok(());
        }
    }
    let temp = match task.over {
        true => replace_slot(&task.dest)?,
        false => staging_path(&task.dest),
    };
    *slot.file.lock() = Some((task.source.clone(), task.size));
    job.claim(task);
    let copied = copy_into(job, task, &temp, slot);
    *slot.file.lock() = None;
    let landed = copied.and_then(|copied| Ok((copied, commit(job, task, &temp)?)));
    let (copied, at) = match landed {
        Ok(landed) => landed,
        Err(error) => {
            match task.over {
                true => drop_slot(&temp),
                false => {
                    let _ = deep::remove_file(&temp);
                }
            }
            job.unclaim(task);
            return Err(error);
        }
    };
    let stamp = copied.stamp;
    copied.settle(&at);
    job.landed(task, &at);
    job.release(task, &at, stamp);
    Ok(())
}

/// What the source was when its copy was made; a move deletes only a source that still is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
}

impl Stamp {
    fn of(meta: &fs::Metadata) -> Stamp {
        Stamp {
            dev: meta.dev(),
            ino: meta.ino(),
            len: meta.len(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
        }
    }

    fn at(meta: &deep::Stat) -> Stamp {
        Stamp {
            dev: meta.dev(),
            ino: meta.ino(),
            len: meta.len(),
            mtime: meta.mtime(),
        }
    }

    pub(super) fn holds(self, path: &Path) -> bool {
        deep::lstat(path).is_ok_and(|meta| Stamp::at(&meta) == self)
    }

    pub(super) fn len(self) -> u64 {
        self.len
    }
}

struct Copied {
    source: File,
    out: File,
    relock: Option<u32>,
    acl: bool,
    stamp: Stamp,
}

impl Copied {
    /// After the rename: a locked file cannot be renamed, nor one whose access list denies deleting it.
    fn settle(self, at: &Path) {
        if self.acl {
            let (from, into) = (self.source.as_raw_fd(), self.out.as_raw_fd());
            unsafe { libc::fcopyfile(from, into, std::ptr::null_mut(), libc::COPYFILE_ACL) };
        }
        if let Some(flags) = self.relock {
            let out = self.out.as_raw_fd();
            let _ = deep::either(
                || set_flags(at, flags),
                || crate::check(unsafe { libc::fchflags(out, flags) }),
            );
        }
    }
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
        Some(libc::ENOTSUP | libc::EXDEV | libc::EPERM | libc::ENAMETOOLONG) => Ok(false),
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

fn copy_into(job: &Job, task: &Task, temp: &Path, slot: &Slot) -> io::Result<Copied> {
    let source = deep::open(&task.source, libc::O_RDONLY | libc::O_NOFOLLOW, 0)?;
    let before = source.metadata()?;
    let out = deep::open(temp, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600)?;
    let kept = attributes(job, &source);
    let carry = libc::COPYFILE_DATA | (kept & !libc::COPYFILE_ACL);
    let bytes = State::new()?
        .call(job, slot, |state| unsafe {
            libc::fcopyfile(source.as_raw_fd(), out.as_raw_fd(), state, carry)
        })
        .map_err(|error| match job.halted() {
            true => io::Error::from_raw_os_error(libc::ECANCELED),
            false => error,
        })?;
    let after = source.metadata()?;
    if (after.len(), after.mtime(), after.mtime_nsec())
        != (before.len(), before.mtime(), before.mtime_nsec())
    {
        let why = "it changed while it was being copied";
        return Err(because(job, &task.source, io::ErrorKind::InvalidData, why));
    }
    let written = out.metadata()?;
    if written.len() != before.len() || bytes != before.len() {
        let why = "not all of it could be written";
        return Err(because(job, &task.source, io::ErrorKind::InvalidData, why));
    }
    let mut copied = Copied {
        stamp: Stamp::of(&before),
        acl: kept & libc::COPYFILE_ACL != 0,
        relock: None,
        source,
        out,
    };
    if lean(job) {
        set_times(&copied.out, &before);
        // Not the staging file's flags: FAT and ExFAT hide any dot-named file.
        let locks = before.st_flags() & LOCKS;
        copied.relock = (locks != 0).then_some(locks);
        return Ok(copied);
    }
    keep_birth(&copied.out, &before);
    if job.kind == Kind::Copy {
        regroup(&copied.out, task.parent.gid());
    }
    if job.is_move() && job.volume.network {
        copied.out.sync_all()?;
    }
    let flags = written.st_flags();
    if flags & LOCKS != 0 {
        crate::check(unsafe { libc::fchflags(copied.out.as_raw_fd(), flags & !LOCKS) })?;
        copied.relock = Some(flags);
    }
    Ok(copied)
}

fn commit(job: &Job, task: &Task, temp: &Path) -> io::Result<PathBuf> {
    if task.over {
        return replace(job, temp, &task.dest).map(|()| task.dest.clone());
    }
    let next = || job.next_free(task);
    land(task.dest.clone(), next, |at| rename_new(temp, at)).map(|(at, ())| at)
}

fn replace(job: &Job, slot: &Path, dest: &Path) -> io::Result<()> {
    exchange(slot, dest)?;
    match trash_slot(&NSFileManager::defaultManager(), slot) {
        Ok(item) => job.displaced(item, dest),
        Err(_) => drop_slot(slot),
    }
    Ok(())
}

fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    match deep::rename(from, to, libc::RENAME_EXCL) {
        Err(error) if matches!(error.raw_os_error(), Some(libc::ENOTSUP | libc::EINVAL)) => {}
        renamed => return renamed,
    }
    if taken(to) {
        return Err(io::Error::from_raw_os_error(libc::EEXIST));
    }
    deep::rename(from, to, 0)
}

fn copy_link(job: &Job, task: &Task) -> io::Result<()> {
    let stamp = Stamp::at(&deep::lstat(&task.source)?);
    let at = match task.over {
        false => {
            let next = || job.next_free(task);
            land(task.dest.clone(), next, |at| {
                make_link(job, &task.source, at)
            })?
            .0
        }
        true => {
            let slot = replace_slot(&task.dest)?;
            let placed =
                make_link(job, &task.source, &slot).and_then(|()| replace(job, &slot, &task.dest));
            if let Err(error) = placed {
                drop_slot(&slot);
                return Err(error);
            }
            task.dest.clone()
        }
    };
    job.landed(task, &at);
    job.release(task, &at, stamp);
    Ok(())
}

pub(super) fn settle_dir(job: &Job, source: &Path, dest: &Path) -> io::Result<()> {
    let open = |path: &Path| {
        deep::open(
            path,
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            0,
        )
    };
    let (from, into) = (open(source)?, open(dest)?);
    let carry = attributes(job, &from);
    if carry != 0 {
        State::new()?.call(job, &Slot::default(), |state| unsafe {
            libc::fcopyfile(from.as_raw_fd(), into.as_raw_fd(), state, carry)
        })?;
    }
    if lean(job) {
        set_times(&into, &from.metadata()?);
        return Ok(());
    }
    keep_birth(&into, &from.metadata()?);
    if job.kind == Kind::Copy {
        let parent = dest.parent().and_then(|parent| deep::lstat(parent).ok());
        regroup(&into, parent.map_or(u32::MAX, |meta| meta.gid()));
    }
    Ok(())
}

fn make_link(job: &Job, source: &Path, dest: &Path) -> io::Result<()> {
    let (from, to) = (crate::c_path(source)?, crate::c_path(dest)?);
    let flags = ALL | libc::COPYFILE_NOFOLLOW | libc::COPYFILE_EXCL;
    let copied = State::new()?.call(job, &Slot::default(), |state| unsafe {
        libc::copyfile(from.as_ptr(), to.as_ptr(), state, flags)
    });
    match copied {
        Err(error) if error.raw_os_error() == Some(libc::ENAMETOOLONG) => {
            deep::copy_link(source, dest)
        }
        copied => copied.map(|_| ()),
    }
}

fn regroup(file: &File, gid: u32) {
    if gid != u32::MAX {
        unsafe { libc::fchown(file.as_raw_fd(), u32::MAX, gid) };
    }
}

fn keep_owner(source: &Path, dest: &Path) {
    let Ok(meta) = fs::symlink_metadata(source) else {
        return;
    };
    if lchown(dest, Some(meta.uid()), Some(meta.gid())).is_err() {
        let _ = lchown(dest, None, Some(meta.gid()));
    }
}

fn set_flags(path: &Path, flags: u32) -> io::Result<()> {
    let path = crate::c_path(path)?;
    crate::check(unsafe { libc::chflags(path.as_ptr(), flags) })
}

fn set_times(file: &File, times: &fs::Metadata) {
    let at = |tv_sec, tv_nsec| libc::timespec { tv_sec, tv_nsec };
    let attrs = libc::ATTR_CMN_CRTIME | libc::ATTR_CMN_MODTIME | libc::ATTR_CMN_ACCTIME;
    let mut values = [
        at(times.st_birthtime(), times.st_birthtime_nsec()),
        at(times.mtime(), times.mtime_nsec()),
        at(times.atime(), times.atime_nsec()),
    ];
    apply_times(file, attrs, &mut values);
}

/// `copyfile` carries the modification date but leaves the creation date at the copy's own.
fn keep_birth(file: &File, times: &fs::Metadata) {
    let mut values = [libc::timespec {
        tv_sec: times.st_birthtime(),
        tv_nsec: times.st_birthtime_nsec(),
    }];
    apply_times(file, libc::ATTR_CMN_CRTIME, &mut values);
}

fn apply_times(file: &File, attrs: libc::attrgroup_t, values: &mut [libc::timespec]) {
    let mut wanted = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: attrs,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    unsafe {
        libc::fsetattrlist(
            file.as_raw_fd(),
            (&raw mut wanted).cast(),
            values.as_mut_ptr().cast(),
            std::mem::size_of_val(values),
            0,
        )
    };
}

struct Status<'s, 'a> {
    job: &'s Job<'a>,
    slot: &'s Slot,
    copied: u64,
}

struct State(libc::copyfile_state_t);

impl State {
    fn new() -> io::Result<State> {
        let state = unsafe { libc::copyfile_state_alloc() };
        match state.is_null() {
            true => Err(io::Error::last_os_error()),
            false => Ok(State(state)),
        }
    }

    fn call(
        &self,
        job: &Job,
        slot: &Slot,
        copy: impl FnOnce(libc::copyfile_state_t) -> c_int,
    ) -> io::Result<u64> {
        let mut status = Status {
            job,
            slot,
            copied: 0,
        };
        let context: *mut Status = &mut status;
        unsafe {
            libc::copyfile_state_set(
                self.0,
                libc::COPYFILE_STATE_STATUS_CB as u32,
                status_callback as *const c_void,
            );
            libc::copyfile_state_set(
                self.0,
                libc::COPYFILE_STATE_STATUS_CTX as u32,
                context.cast_const().cast(),
            );
        }
        match copy(self.0) {
            0 => Ok(status.copied),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        unsafe { libc::copyfile_state_free(self.0) };
    }
}

/// Runs on the copying thread for every chunk, so a pause holds and a cancel lands here.
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
            let at = (&raw mut copied).cast();
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
        // CONTINUE would make copyfile retry a failed write forever on a full disk.
        (libc::COPYFILE_COPY_DATA, libc::COPYFILE_ERR) => libc::COPYFILE_QUIT,
        (libc::COPYFILE_COPY_XATTR, libc::COPYFILE_ERR) => libc::COPYFILE_SKIP,
        (libc::COPYFILE_COPY_XATTR, libc::COPYFILE_START) if !status.job.volume.xattrs => {
            let mut name: *const c_char = std::ptr::null();
            let at = (&raw mut name).cast();
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

pub(super) fn worded(error: io::Error, source: &Path, dest: &Path, job: &Job) -> io::Error {
    let Some(code) = error.raw_os_error() else {
        return error;
    };
    let refusal = match code {
        libc::ENOSPC | libc::EDQUOT => job.refusal(Refusal::NoSpace, dest),
        libc::EROFS => job.refusal(Refusal::ReadOnly, dest),
        _ => None,
    };
    if let Some(message) = refusal {
        return io::Error::new(error.kind(), message);
    }
    let why: Cow<str> = match code {
        libc::EFBIG => "it's too large for the volume's format".into(),
        libc::EACCES | libc::EPERM => "you don't have permission".into(),
        libc::ENOENT => "it can't be found".into(),
        libc::ENAMETOOLONG => "its name is too long".into(),
        code => reason(code).into(),
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

pub(super) fn kept(source: &Path, why: &str) -> io::Error {
    let item = name_of(source);
    io::Error::other(format!(
        "\u{201c}{item}\u{201d} was copied, but the original was kept because {why}."
    ))
}

pub(super) fn not_removed(error: io::Error, source: &Path) -> io::Error {
    let item = name_of(source);
    let why: Cow<str> = match error.raw_os_error() {
        Some(libc::EPERM) => "it's locked".into(),
        Some(libc::EACCES) => "you don't have permission".into(),
        Some(code) => reason(code).into(),
        None => error.to_string().into(),
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
