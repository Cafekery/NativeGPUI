use std::ffi::{CStr, CString, OsString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Component, Path};
use std::ptr;

use crate::check;

const WHILE_FILLING: libc::mode_t = 0o700;
const DIR_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;

/// What `lstat` says of an item, read for a path of any length.
#[derive(Clone, Copy)]
pub(super) struct Stat(libc::stat);

impl Stat {
    pub(super) fn is_dir(&self) -> bool {
        is_dir(&self.0)
    }

    pub(super) fn is_file(&self) -> bool {
        self.0.st_mode & libc::S_IFMT == libc::S_IFREG
    }

    pub(super) fn is_symlink(&self) -> bool {
        self.0.st_mode & libc::S_IFMT == libc::S_IFLNK
    }

    pub(super) fn len(&self) -> u64 {
        self.0.st_size as u64
    }

    pub(super) fn dev(&self) -> u64 {
        self.0.st_dev as u64
    }

    pub(super) fn ino(&self) -> u64 {
        self.0.st_ino
    }

    pub(super) fn gid(&self) -> u32 {
        self.0.st_gid
    }

    pub(super) fn mtime(&self) -> (i64, i64) {
        (self.0.st_mtime, self.0.st_mtime_nsec)
    }
}

/// The path's own call, or past `PATH_MAX`, where that fails, the same call through descriptors.
pub(super) fn either<T>(
    short: impl FnOnce() -> io::Result<T>,
    long: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    match short() {
        Err(err) if err.raw_os_error() == Some(libc::ENAMETOOLONG) => long(),
        done => done,
    }
}

fn split(path: &Path) -> io::Result<(&Path, CString)> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path has no file name"))?;
    Ok((
        path.parent().unwrap_or(Path::new("/")),
        CString::new(name.as_bytes())?,
    ))
}

fn owned(fd: libc::c_int) -> io::Result<OwnedFd> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn open_dir_at(dir: &OwnedFd, name: &CStr) -> io::Result<OwnedFd> {
    owned(unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), DIR_FLAGS) })
}

/// Half of `PATH_MAX` opens in one call and the rest a part at a time: the limit counts the path
/// with its links expanded, and `/var` alone is one.
fn open_dir(path: &Path) -> io::Result<OwnedFd> {
    let fits = path
        .ancestors()
        .find(|part| part.as_os_str().len() < libc::PATH_MAX as usize / 2)
        .filter(|part| !part.as_os_str().is_empty());
    let start = match fits {
        Some(part) => crate::c_path(part)?,
        None => c".".to_owned(),
    };
    let (mut dir, rest) = match owned(unsafe { libc::open(start.as_ptr(), DIR_FLAGS) }) {
        Ok(dir) => (
            dir,
            fits.map_or(path, |part| path.strip_prefix(part).unwrap_or(path)),
        ),
        Err(err) if err.raw_os_error() == Some(libc::ENAMETOOLONG) => {
            let start = if path.is_absolute() { c"/" } else { c"." };
            (
                owned(unsafe { libc::open(start.as_ptr(), DIR_FLAGS) })?,
                path,
            )
        }
        Err(err) => return Err(err),
    };
    for part in rest.components() {
        dir = match part {
            Component::Normal(name) => open_dir_at(&dir, &CString::new(name.as_bytes())?)?,
            Component::ParentDir => open_dir_at(&dir, c"..")?,
            Component::RootDir | Component::CurDir | Component::Prefix(_) => dir,
        };
    }
    Ok(dir)
}

fn at(path: &Path) -> io::Result<(OwnedFd, CString)> {
    let (dir, name) = split(path)?;
    Ok((open_dir(dir)?, name))
}

fn stat_at(dir: &OwnedFd, name: &CStr) -> io::Result<libc::stat> {
    let mut meta = unsafe { std::mem::zeroed::<libc::stat>() };
    check(unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            name.as_ptr(),
            &mut meta,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    })?;
    Ok(meta)
}

fn is_dir(meta: &libc::stat) -> bool {
    meta.st_mode & libc::S_IFMT == libc::S_IFDIR
}

fn names_in(dir: &OwnedFd) -> io::Result<Vec<CString>> {
    let listing = dir.try_clone()?;
    let handle = unsafe { libc::fdopendir(listing.into_raw_fd()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut names = Vec::new();
    let failed = loop {
        unsafe { *libc::__error() = 0 };
        let entry = unsafe { libc::readdir(handle) };
        if entry.is_null() {
            let err = io::Error::last_os_error();
            break (err.raw_os_error() != Some(0)).then_some(err);
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name != c"." && name != c".." {
            names.push(name.to_owned());
        }
    };
    unsafe { libc::closedir(handle) };
    failed.map_or(Ok(names), Err)
}

fn copy_state(from: &OwnedFd, into: &OwnedFd, with_data: bool) -> io::Result<()> {
    let mut flags = libc::COPYFILE_STAT | libc::COPYFILE_XATTR | libc::COPYFILE_ACL;
    if with_data {
        flags |= libc::COPYFILE_DATA;
    }
    check(unsafe {
        libc::fcopyfile(
            from.as_raw_fd(),
            into.as_raw_fd(),
            ptr::null_mut(),
            flags as libc::copyfile_flags_t,
        )
    })
}

fn link_at(
    sdir: &OwnedFd,
    sname: &CStr,
    ddir: &OwnedFd,
    dname: &CStr,
    size: i64,
) -> io::Result<()> {
    let mut target = vec![0u8; size.max(1024) as usize + 1];
    let read = unsafe {
        libc::readlinkat(
            sdir.as_raw_fd(),
            sname.as_ptr(),
            target.as_mut_ptr().cast(),
            target.len(),
        )
    };
    if read < 0 {
        return Err(io::Error::last_os_error());
    }
    target.truncate(read as usize);
    let target = CString::new(target)?;
    check(unsafe { libc::symlinkat(target.as_ptr(), ddir.as_raw_fd(), dname.as_ptr()) })
}

fn make_dir_at(dir: &OwnedFd, name: &CStr) -> io::Result<()> {
    check(unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), WHILE_FILLING) })
}

fn unlink_at(dir: &OwnedFd, name: &CStr, flags: libc::c_int) -> io::Result<()> {
    check(unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), flags) })
}

fn file_at(sdir: &OwnedFd, sname: &CStr, ddir: &OwnedFd, dname: &CStr) -> io::Result<()> {
    let from = owned(unsafe {
        libc::openat(
            sdir.as_raw_fd(),
            sname.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC,
        )
    })?;
    let into = owned(unsafe {
        libc::openat(
            ddir.as_raw_fd(),
            dname.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
            libc::c_uint::from(WHILE_FILLING),
        )
    })?;
    copy_state(&from, &into, true)
}

fn copy_leaf(
    meta: &libc::stat,
    sdir: &OwnedFd,
    sname: &CStr,
    ddir: &OwnedFd,
    dname: &CStr,
) -> io::Result<()> {
    match meta.st_mode & libc::S_IFMT {
        libc::S_IFLNK => link_at(sdir, sname, ddir, dname, meta.st_size),
        _ => file_at(sdir, sname, ddir, dname),
    }
}

/// Let go on the way down and found again through `..`, or a deep tree runs out of descriptors.
fn fill_dir(mut from: OwnedFd, mut into: OwnedFd) -> io::Result<(OwnedFd, OwnedFd)> {
    for name in names_in(&from)? {
        let meta = stat_at(&from, &name)?;
        if !is_dir(&meta) {
            copy_leaf(&meta, &from, &name, &into, &name)?;
            continue;
        }
        make_dir_at(&into, &name)?;
        let child = (open_dir_at(&from, &name)?, open_dir_at(&into, &name)?);
        drop(from);
        drop(into);
        let (child_from, child_into) = fill_dir(child.0, child.1)?;
        from = open_dir_at(&child_from, c"..")?;
        into = open_dir_at(&child_into, c"..")?;
        copy_state(&child_from, &child_into, false)?;
    }
    Ok((from, into))
}

fn copy_at(sdir: &OwnedFd, sname: &CStr, ddir: &OwnedFd, dname: &CStr) -> io::Result<()> {
    let meta = stat_at(sdir, sname)?;
    if !is_dir(&meta) {
        return copy_leaf(&meta, sdir, sname, ddir, dname);
    }
    make_dir_at(ddir, dname)?;
    let (from, into) = fill_dir(open_dir_at(sdir, sname)?, open_dir_at(ddir, dname)?)?;
    copy_state(&from, &into, false)
}

fn empty_dir(mut dir: OwnedFd) -> io::Result<OwnedFd> {
    for name in names_in(&dir)? {
        if !is_dir(&stat_at(&dir, &name)?) {
            unlink_at(&dir, &name, 0)?;
            continue;
        }
        let inside = open_dir_at(&dir, &name)?;
        drop(dir);
        let inside = empty_dir(inside)?;
        dir = open_dir_at(&inside, c"..")?;
        drop(inside);
        unlink_at(&dir, &name, libc::AT_REMOVEDIR)?;
    }
    Ok(dir)
}

fn remove_at(dir: &OwnedFd, name: &CStr) -> io::Result<()> {
    if !is_dir(&stat_at(dir, name)?) {
        return unlink_at(dir, name, 0);
    }
    drop(empty_dir(open_dir_at(dir, name)?)?);
    unlink_at(dir, name, libc::AT_REMOVEDIR)
}

pub(super) fn stat(path: &Path) -> io::Result<libc::stat> {
    let (dir, name) = at(path)?;
    stat_at(&dir, &name)
}

pub(super) fn identity(path: &Path) -> io::Result<(u64, u64)> {
    stat(path).map(|meta| (meta.st_dev as u64, meta.st_ino))
}

pub(super) fn lstat(path: &Path) -> io::Result<Stat> {
    either(
        || {
            let name = crate::c_path(path)?;
            let mut meta = unsafe { std::mem::zeroed::<libc::stat>() };
            check(unsafe { libc::lstat(name.as_ptr(), &mut meta) })?;
            Ok(Stat(meta))
        },
        || stat(path).map(Stat),
    )
}

pub(super) fn open(path: &Path, flags: libc::c_int, mode: libc::mode_t) -> io::Result<File> {
    let (flags, mode) = (flags | libc::O_CLOEXEC, libc::c_uint::from(mode));
    let opened = either(
        || {
            let name = crate::c_path(path)?;
            loop {
                match owned(unsafe { libc::open(name.as_ptr(), flags, mode) }) {
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                    opened => return opened,
                }
            }
        },
        || {
            let (dir, name) = at(path)?;
            owned(unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags, mode) })
        },
    )?;
    Ok(File::from(opened))
}

pub(super) fn create_dir(path: &Path, mode: libc::mode_t) -> io::Result<()> {
    either(
        || {
            std::fs::DirBuilder::new()
                .mode(u32::from(mode))
                .create(path)
        },
        || {
            let (dir, name) = at(path)?;
            check(unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), mode) })
        },
    )
}

pub(super) fn rename(from: &Path, to: &Path, flags: libc::c_uint) -> io::Result<()> {
    either(
        || {
            let (from, to) = (crate::c_path(from)?, crate::c_path(to)?);
            check(unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), flags) })
        },
        || {
            let ((from_dir, from_name), (to_dir, to_name)) = (at(from)?, at(to)?);
            check(unsafe {
                libc::renameatx_np(
                    from_dir.as_raw_fd(),
                    from_name.as_ptr(),
                    to_dir.as_raw_fd(),
                    to_name.as_ptr(),
                    flags,
                )
            })
        },
    )
}

pub(super) fn remove_file(path: &Path) -> io::Result<()> {
    either(
        || std::fs::remove_file(path),
        || {
            let (dir, name) = at(path)?;
            unlink_at(&dir, &name, 0)
        },
    )
}

pub(super) fn remove_dir(path: &Path) -> io::Result<()> {
    either(
        || std::fs::remove_dir(path),
        || {
            let (dir, name) = at(path)?;
            unlink_at(&dir, &name, libc::AT_REMOVEDIR)
        },
    )
}

/// Each name in the folder with what `lstat` says of it, read whole so a deep walk holds one
/// directory descriptor at a time.
pub(super) fn list(path: &Path) -> io::Result<Vec<(OsString, io::Result<Stat>)>> {
    let dir = open_dir(path)?;
    let names = names_in(&dir)?;
    Ok(names
        .into_iter()
        .map(|name| {
            let meta = stat_at(&dir, &name).map(Stat);
            (OsString::from_vec(name.into_bytes()), meta)
        })
        .collect())
}

/// A link, never what it points at.
pub(super) fn copy_link(source: &Path, dest: &Path) -> io::Result<()> {
    let ((sdir, sname), (ddir, dname)) = (at(source)?, at(dest)?);
    let meta = stat_at(&sdir, &sname)?;
    link_at(&sdir, &sname, &ddir, &dname, meta.st_size)
}

pub(super) fn remove(path: &Path) -> io::Result<()> {
    let (dir, name) = at(path)?;
    remove_at(&dir, &name)
}

pub(super) fn copy(src: &Path, dst: &Path) -> io::Result<()> {
    let ((sdir, sname), (ddir, dname)) = (at(src)?, at(dst)?);
    if let Err(err) = copy_at(&sdir, &sname, &ddir, &dname) {
        // EEXIST comes from the first create: whatever holds the name was there before.
        if err.raw_os_error() != Some(libc::EEXIST) {
            let _ = remove_at(&ddir, &dname);
        }
        return Err(err);
    }
    Ok(())
}
