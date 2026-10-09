use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Component, Path};
use std::ptr;

use crate::check;

const WHILE_FILLING: libc::mode_t = 0o700;
const DIR_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;

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

fn open_dir(path: &Path) -> io::Result<OwnedFd> {
    let start = if path.is_absolute() { c"/" } else { c"." };
    let mut dir = owned(unsafe { libc::open(start.as_ptr(), DIR_FLAGS) })?;
    for part in path.components() {
        dir = match part {
            Component::Normal(name) => open_dir_at(&dir, &CString::new(name.as_bytes())?)?,
            Component::ParentDir => open_dir_at(&dir, c"..")?,
            Component::RootDir | Component::CurDir | Component::Prefix(_) => dir,
        };
    }
    Ok(dir)
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

fn copy_link(
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

fn make_dir(dir: &OwnedFd, name: &CStr) -> io::Result<()> {
    check(unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), WHILE_FILLING) })
}

fn unlink_at(dir: &OwnedFd, name: &CStr, flags: libc::c_int) -> io::Result<()> {
    check(unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), flags) })
}

fn copy_file(sdir: &OwnedFd, sname: &CStr, ddir: &OwnedFd, dname: &CStr) -> io::Result<()> {
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
        libc::S_IFLNK => copy_link(sdir, sname, ddir, dname, meta.st_size),
        _ => copy_file(sdir, sname, ddir, dname),
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
        make_dir(&into, &name)?;
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
    make_dir(ddir, dname)?;
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
    let (dir, name) = split(path)?;
    stat_at(&open_dir(dir)?, &name)
}

pub(super) fn identity(path: &Path) -> io::Result<(u64, u64)> {
    stat(path).map(|meta| (meta.st_dev as u64, meta.st_ino))
}

pub(super) fn remove(path: &Path) -> io::Result<()> {
    let (dir, name) = split(path)?;
    remove_at(&open_dir(dir)?, &name)
}

pub(super) fn transfer(src: &Path, dst: &Path, is_move: bool) -> io::Result<()> {
    let (sdir, sname) = split(src)?;
    let (ddir, dname) = split(dst)?;
    let (sdir, ddir) = (open_dir(sdir)?, open_dir(ddir)?);
    if is_move
        && unsafe {
            libc::renameat(
                sdir.as_raw_fd(),
                sname.as_ptr(),
                ddir.as_raw_fd(),
                dname.as_ptr(),
            )
        } == 0
    {
        return Ok(());
    }
    if let Err(err) = copy_at(&sdir, &sname, &ddir, &dname) {
        // EEXIST comes from the first create: whatever holds the name was there before.
        if err.raw_os_error() != Some(libc::EEXIST) {
            let _ = remove_at(&ddir, &dname);
        }
        return Err(err);
    }
    if is_move {
        remove_at(&sdir, &sname)?;
    }
    Ok(())
}
