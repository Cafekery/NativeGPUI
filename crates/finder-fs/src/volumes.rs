use std::ffi::{CString, c_void};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::ptr::null;

use objc2::rc::Retained;
use objc2_foundation::{
    NSArray, NSDictionary, NSFileManager, NSMutableDictionary, NSNumber, NSString, NSURL,
    NSURLResourceKey, NSURLVolumeAvailableCapacityForImportantUsageKey,
    NSURLVolumeAvailableCapacityKey, NSURLVolumeIsEjectableKey, NSURLVolumeIsLocalKey,
    NSURLVolumeIsRemovableKey, NSURLVolumeLocalizedFormatDescriptionKey, NSURLVolumeNameKey,
    NSURLVolumeTotalCapacityKey, NSVolumeEnumerationOptions,
};

#[derive(Clone, Debug)]
pub struct Volume {
    pub name: String,
    pub path: PathBuf,
    pub ejectable: bool,
    pub network: bool,
    pub capacity: u64,
    pub available: u64,
    /// The file system as Finder names it, e.g. `APFS (Encrypted)`.
    pub format: String,
}

fn keys() -> Retained<NSArray<NSURLResourceKey>> {
    unsafe {
        NSArray::from_slice(&[
            NSURLVolumeNameKey,
            NSURLVolumeIsEjectableKey,
            NSURLVolumeIsRemovableKey,
            NSURLVolumeIsLocalKey,
            NSURLVolumeTotalCapacityKey,
            NSURLVolumeAvailableCapacityForImportantUsageKey,
            NSURLVolumeAvailableCapacityKey,
            NSURLVolumeLocalizedFormatDescriptionKey,
        ])
    }
}

fn string_value(
    values: &NSDictionary<NSURLResourceKey, objc2::runtime::AnyObject>,
    key: &NSString,
) -> Option<String> {
    values
        .objectForKey(key)?
        .downcast::<NSString>()
        .ok()
        .map(|s| s.to_string())
}

fn bool_value(
    values: &NSDictionary<NSURLResourceKey, objc2::runtime::AnyObject>,
    key: &NSString,
) -> bool {
    values
        .objectForKey(key)
        .and_then(|v| v.downcast::<NSNumber>().ok())
        .is_some_and(|n| n.boolValue())
}

fn u64_value(
    values: &NSDictionary<NSURLResourceKey, objc2::runtime::AnyObject>,
    key: &NSString,
) -> Option<u64> {
    values
        .objectForKey(key)
        .and_then(|v| v.downcast::<NSNumber>().ok())
        .map(|n| n.longLongValue().max(0) as u64)
}

/// `…ForImportantUsage` reads 0 on exFAT and FAT32 rather than being absent, so a zero from
/// either key means "this file system does not say" and `statfs` answers instead.
fn available_at(
    values: &NSDictionary<NSURLResourceKey, objc2::runtime::AnyObject>,
    path: &Path,
) -> u64 {
    let free = |key: &NSString| u64_value(values, key).filter(|&free| free > 0);
    free(unsafe { NSURLVolumeAvailableCapacityForImportantUsageKey })
        .or_else(|| free(unsafe { NSURLVolumeAvailableCapacityKey }))
        .unwrap_or_else(|| statfs_available(path))
}

fn statfs_available(path: &Path) -> u64 {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return 0;
    };
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `stat` is a valid out-pointer for the call.
    if unsafe { libc::statfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return 0;
    }
    // SAFETY: statfs returned 0, so the struct is initialised.
    let stat = unsafe { stat.assume_init() };
    stat.f_bavail.saturating_mul(stat.f_bsize as u64)
}

fn volume_at(url: &NSURL, keys: &NSArray<NSURLResourceKey>) -> Option<Volume> {
    let values = url.resourceValuesForKeys_error(keys).ok()?;
    let name_key = unsafe { NSURLVolumeNameKey };
    let path = url
        .path()
        .map(|p| PathBuf::from(p.to_string()))
        .unwrap_or_default();
    let available = available_at(&values, &path);
    Some(Volume {
        name: string_value(&values, name_key).unwrap_or_default(),
        path,
        ejectable: bool_value(&values, unsafe { NSURLVolumeIsEjectableKey })
            || bool_value(&values, unsafe { NSURLVolumeIsRemovableKey }),
        network: !bool_value(&values, unsafe { NSURLVolumeIsLocalKey }),
        capacity: u64_value(&values, unsafe { NSURLVolumeTotalCapacityKey }).unwrap_or(0),
        available,
        format: string_value(&values, unsafe { NSURLVolumeLocalizedFormatDescriptionKey })
            .unwrap_or_default(),
    })
}

pub fn mounted() -> Vec<Volume> {
    let fm = NSFileManager::defaultManager();
    let keys = keys();
    let Some(urls) = fm.mountedVolumeURLsIncludingResourceValuesForKeys_options(
        Some(&keys),
        NSVolumeEnumerationOptions::SkipHiddenVolumes,
    ) else {
        return Vec::new();
    };
    let mut volumes: Vec<Volume> = urls
        .to_vec()
        .iter()
        .filter_map(|url| volume_at(url, &keys))
        .collect();
    if let Some(boot) = volumes
        .iter()
        .position(|v| v.path.as_path() == std::path::Path::new("/"))
    {
        volumes.swap(0, boot);
    }
    volumes
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standard {
    Home,
    Desktop,
    Documents,
    Downloads,
    Applications,
    Movies,
    Music,
    Pictures,
    Trash,
}

pub fn standard(which: Standard) -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(match which {
        Standard::Home => home,
        Standard::Desktop => home.join("Desktop"),
        Standard::Documents => home.join("Documents"),
        Standard::Downloads => home.join("Downloads"),
        Standard::Applications => PathBuf::from("/Applications"),
        Standard::Movies => home.join("Movies"),
        Standard::Music => home.join("Music"),
        Standard::Pictures => home.join("Pictures"),
        Standard::Trash => home.join(".Trash"),
    })
}

#[link(name = "NetFS", kind = "framework")]
extern "C" {
    fn NetFSMountURLSync(
        url: *const c_void,
        mountpath: *const c_void,
        user: *const c_void,
        passwd: *const c_void,
        open_options: *const c_void,
        mount_options: *const c_void,
        mountpoints: *mut *mut c_void,
    ) -> i32;
}

/// Mounts a server as Connect to Server does, the system asking for any login itself. It blocks
/// until the share is up, so it belongs off the main thread; the error is NetFS's status code.
pub fn mount(address: &str) -> Result<PathBuf, i32> {
    let url = NSURL::URLWithString(&NSString::from_str(address)).ok_or(libc::EINVAL)?;
    let open = NSMutableDictionary::<NSString, NSString>::new();
    open.insert(
        &*NSString::from_str("UIOption"),
        &*NSString::from_str("AllowUI"),
    );
    let mut points: *mut c_void = std::ptr::null_mut();
    let status = unsafe {
        NetFSMountURLSync(
            Retained::as_ptr(&url).cast(),
            null(),
            null(),
            null(),
            Retained::as_ptr(&open).cast(),
            null(),
            &mut points,
        )
    };
    if status != 0 {
        return Err(status);
    }
    // A `+1` CFArray of the mount points' paths.
    let points: Option<Retained<NSArray<NSString>>> = unsafe { Retained::from_raw(points.cast()) };
    points
        .and_then(|points| points.firstObject())
        .map(|point| PathBuf::from(point.to_string()))
        .ok_or(libc::ENOENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_volume_is_first_and_sized() {
        let volumes = mounted();
        assert!(!volumes.is_empty());
        assert_eq!(volumes[0].path, PathBuf::from("/"));
        assert!(volumes[0].capacity > 0);
    }

    #[test]
    fn free_space_falls_back_to_the_file_system() {
        let boot = mounted().remove(0);
        assert!(boot.available > 0);
        assert!(statfs_available(&boot.path) > 0);
        assert_eq!(statfs_available(Path::new("/no/such/volume")), 0);
    }

    #[test]
    fn standard_folders_are_under_home() {
        let home = standard(Standard::Home).unwrap();
        assert_eq!(standard(Standard::Desktop).unwrap(), home.join("Desktop"));
    }
}
