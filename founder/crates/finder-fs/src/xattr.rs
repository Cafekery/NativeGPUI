use std::ffi::CStr;
use std::io;

use crate::check;

pub(crate) fn get(path: &CStr, name: &CStr) -> Option<Vec<u8>> {
    let size =
        unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
    let mut bytes = vec![0u8; usize::try_from(size).ok().filter(|&size| size > 0)?];
    let read = unsafe {
        libc::getxattr(
            path.as_ptr(),
            name.as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            0,
            0,
        )
    };
    bytes.truncate(usize::try_from(read).ok()?);
    Some(bytes)
}

pub(crate) fn set(path: &CStr, name: &CStr, bytes: &[u8]) -> io::Result<()> {
    check(unsafe {
        libc::setxattr(
            path.as_ptr(),
            name.as_ptr(),
            bytes.as_ptr().cast(),
            bytes.len(),
            0,
            0,
        )
    })
}

pub(crate) fn remove(path: &CStr, name: &CStr) -> io::Result<()> {
    match check(unsafe { libc::removexattr(path.as_ptr(), name.as_ptr(), 0) }) {
        Err(error) if error.raw_os_error() != Some(libc::ENOATTR) => Err(error),
        _ => Ok(()),
    }
}
