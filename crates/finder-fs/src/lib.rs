pub mod comment;
pub mod complete;
pub mod custom_icon;
pub mod ds_store;
pub mod entry;
pub mod flags;
pub mod folder_icon;
pub mod icloud;
pub mod kind;
pub mod list;
pub mod meta;
pub mod name;
pub mod ops;
pub mod permissions;
pub mod privacy;
pub mod put_back;
pub mod queue;
pub mod servers;
pub mod sidebar;
pub mod sort;
pub mod tags;
pub mod viewer;
pub mod volumes;
pub mod watch;
pub mod words;
mod xattr;

pub use entry::{Entry, EntryKind};

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSData, NSDictionary, NSPropertyListMutabilityOptions, NSPropertyListSerialization, NSString,
};

/// Not background: background QoS spends more CPU than utility on the same walk.
pub(crate) fn utility_qos() {
    unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0) };
}

pub(crate) fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub(crate) fn c_path(path: &Path) -> io::Result<CString> {
    Ok(CString::new(path.as_os_str().as_bytes())?)
}

pub(crate) fn check(status: libc::c_int) -> io::Result<()> {
    match status {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

#[cfg(test)]
pub(crate) fn trash_reachable() -> bool {
    let trash = volumes::standard(volumes::Standard::Trash);
    let reachable = trash.is_some_and(|trash| std::fs::read_dir(trash).is_ok());
    if !reachable {
        eprintln!("skipping: the Trash is not readable here");
    }
    reachable
}

#[cfg(test)]
pub(crate) fn purge(paths: &[PathBuf]) {
    for path in paths {
        let removed = std::fs::symlink_metadata(path).and_then(|meta| {
            if meta.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            }
        });
        removed.unwrap_or_else(|err| panic!("{} was left behind: {err}", path.display()));
    }
}

pub(crate) fn plist(bytes: &[u8]) -> Option<Retained<AnyObject>> {
    unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &NSData::with_bytes(bytes),
            NSPropertyListMutabilityOptions::Immutable,
            std::ptr::null_mut(),
        )
    }
    .ok()
}

pub(crate) fn plist_dict(bytes: &[u8]) -> Option<Retained<NSDictionary<NSString, AnyObject>>> {
    let dict = plist(bytes)?.downcast::<NSDictionary>().ok()?;
    // SAFETY: property-list dictionary keys are always strings.
    Some(unsafe { Retained::cast_unchecked(dict) })
}
