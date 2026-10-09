use std::ffi::{CStr, OsStr, c_void};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::ptr::null;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSArray, NSDictionary, NSMutableDictionary, NSNumber, NSString, NSURL, NSURLResourceKey,
    NSURLVolumeAvailableCapacityForImportantUsageKey, NSURLVolumeAvailableCapacityKey,
    NSURLVolumeIsEjectableKey, NSURLVolumeIsLocalKey, NSURLVolumeIsRemovableKey,
    NSURLVolumeLocalizedFormatDescriptionKey, NSURLVolumeNameKey, NSURLVolumeTotalCapacityKey,
};

type Values = NSDictionary<NSURLResourceKey, AnyObject>;

#[derive(Clone, Debug)]
pub struct Volume {
    pub name: String,
    pub path: PathBuf,
    pub ejectable: bool,
    pub network: bool,
    pub capacity: u64,
    pub available: u64,
    pub format: String,
    pub server: Option<String>,
}

fn volume_at(url: &NSURL) -> Option<Volume> {
    let keys = unsafe {
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
    };
    let values = url.resourceValuesForKeys_error(&keys).ok()?;
    let path = url
        .path()
        .map(|p| PathBuf::from(p.to_string()))
        .unwrap_or_default();
    let available = available_at(&values, &path);
    Some(Volume {
        name: string_value(&values, unsafe { NSURLVolumeNameKey }).unwrap_or_default(),
        path,
        ejectable: bool_value(&values, unsafe { NSURLVolumeIsEjectableKey })
            || bool_value(&values, unsafe { NSURLVolumeIsRemovableKey }),
        network: !bool_value(&values, unsafe { NSURLVolumeIsLocalKey }),
        capacity: u64_value(&values, unsafe { NSURLVolumeTotalCapacityKey }).unwrap_or(0),
        available,
        format: string_value(&values, unsafe { NSURLVolumeLocalizedFormatDescriptionKey })
            .unwrap_or_default(),
        server: None,
    })
}

fn string_value(values: &Values, key: &NSString) -> Option<String> {
    values
        .objectForKey(key)?
        .downcast::<NSString>()
        .ok()
        .map(|s| s.to_string())
}

fn bool_value(values: &Values, key: &NSString) -> bool {
    values
        .objectForKey(key)
        .and_then(|v| v.downcast::<NSNumber>().ok())
        .is_some_and(|n| n.boolValue())
}

fn u64_value(values: &Values, key: &NSString) -> Option<u64> {
    values
        .objectForKey(key)
        .and_then(|v| v.downcast::<NSNumber>().ok())
        .map(|n| n.longLongValue().max(0) as u64)
}

/// On exFAT and FAT32 both keys read 0 rather than being absent.
fn available_at(values: &Values, path: &Path) -> u64 {
    let free = |key: &NSString| u64_value(values, key).filter(|&free| free > 0);
    free(unsafe { NSURLVolumeAvailableCapacityForImportantUsageKey })
        .or_else(|| free(unsafe { NSURLVolumeAvailableCapacityKey }))
        .unwrap_or_else(|| statfs_available(path))
}

/// Counts purgeable space as free, as Finder does.
pub(crate) fn available(path: &Path) -> u64 {
    let keys = unsafe {
        NSArray::from_slice(&[
            NSURLVolumeAvailableCapacityForImportantUsageKey,
            NSURLVolumeAvailableCapacityKey,
        ])
    };
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    match url.resourceValuesForKeys_error(&keys) {
        Ok(values) => available_at(&values, path),
        Err(_) => statfs_available(path),
    }
}

fn statfs(path: &Path) -> Option<libc::statfs> {
    let path = crate::c_path(path).ok()?;
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statfs(path.as_ptr(), &mut stat) } == 0).then_some(stat)
}

fn statfs_available(path: &Path) -> u64 {
    statfs(path).map_or(0, |stat| stat.f_bavail.saturating_mul(stat.f_bsize as u64))
}

pub(crate) fn is_local(path: &Path) -> bool {
    statfs(path).is_some_and(|stat| stat.f_flags & libc::MNT_LOCAL as u32 != 0)
}

pub fn ignores_ownership(path: &Path) -> bool {
    statfs(path).is_some_and(|stat| stat.f_flags & libc::MNT_IGNORE_OWNERSHIP as u32 != 0)
}

pub(crate) fn statfs_str(chars: &[libc::c_char]) -> &CStr {
    // SAFETY: the kernel NUL-terminates every name in a statfs record.
    unsafe { CStr::from_ptr(chars.as_ptr()) }
}

pub(crate) fn mount_path(stats: &libc::statfs) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(statfs_str(&stats.f_mntonname).to_bytes()))
}

pub(crate) fn mount_point(path: &Path) -> Option<PathBuf> {
    statfs(path).map(|stats| mount_path(&stats))
}

/// Reads the kernel's cached mount table, so a share that stopped answering can't stall it.
pub(crate) fn mount_table() -> Vec<libc::statfs> {
    let mut stats: Vec<libc::statfs> = Vec::new();
    // SAFETY: a null buffer asks only for the count; the second call passes a buffer of that size.
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count > 0 {
        stats.reserve_exact(count as usize);
        let bytes = (count as usize * std::mem::size_of::<libc::statfs>()) as libc::c_int;
        let got = unsafe { libc::getfsstat(stats.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
        unsafe { stats.set_len(got.max(0) as usize) };
    }
    stats
}

pub fn mounted() -> Vec<Volume> {
    let mut volumes: Vec<Volume> = mount_table()
        .iter()
        .filter(|stats| stats.f_flags & libc::MNT_DONTBROWSE as u32 == 0)
        .map(volume_from)
        .collect();
    if let Some(boot) = volumes.iter().position(|v| v.path == Path::new("/")) {
        volumes.swap(0, boot);
    }
    volumes
}

/// The volume `path` is on, one hidden from browsing too, such as a disk image attached that way.
pub(crate) fn holding(path: &Path) -> Option<Volume> {
    mount_table()
        .iter()
        .filter(|stats| path.starts_with(mount_path(stats)))
        .max_by_key(|stats| mount_path(stats).as_os_str().len())
        .map(volume_from)
}

fn volume_from(stats: &libc::statfs) -> Volume {
    let path = mount_path(stats);
    let block = u64::from(stats.f_bsize);
    let local = stats.f_flags & libc::MNT_LOCAL as u32 != 0;
    let volume = Volume {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path,
        ejectable: !local,
        network: !local,
        capacity: stats.f_blocks * block,
        available: stats.f_bavail * block,
        format: statfs_str(&stats.f_fstypename)
            .to_string_lossy()
            .into_owned(),
        server: if local {
            None
        } else {
            host_of(&statfs_str(&stats.f_mntfromname).to_string_lossy())
        },
    };
    if !local {
        return volume;
    }
    // Asking is an XPC round trip that waits on any share that stopped answering.
    let url = NSURL::fileURLWithPath(&NSString::from_str(&volume.path.to_string_lossy()));
    volume_at(&url).unwrap_or(volume)
}

fn host_of(source: &str) -> Option<String> {
    let rest = source.split_once("//").map_or(source, |(_, rest)| rest);
    let host = rest.split('/').next()?.rsplit('@').next()?;
    let host = host.split(':').next()?;
    (!host.is_empty()).then(|| host.to_owned())
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
    let home = crate::home()?;
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

/// Blocks until the share is up; the error is NetFS's status code.
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
    // SAFETY: NetFS hands back a +1 CFArray of paths, or null.
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
    fn a_network_share_is_named_by_its_mount_point_listed_by_its_host_and_never_asked() {
        // SAFETY: statfs is plain data; all-zero is a valid, if empty, record.
        let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
        for (at, byte) in b"/Volumes/Storage".iter().enumerate() {
            stats.f_mntonname[at] = *byte as libc::c_char;
        }
        for (at, byte) in b"//jace@100.109.9.62/Storage".iter().enumerate() {
            stats.f_mntfromname[at] = *byte as libc::c_char;
        }
        stats.f_bsize = 4096;
        stats.f_blocks = 10;
        stats.f_bavail = 4;
        let volume = volume_from(&stats);
        assert_eq!(volume.name, "Storage");
        assert_eq!(volume.server.as_deref(), Some("100.109.9.62"));
        assert!(volume.network && volume.ejectable);
        assert_eq!((volume.capacity, volume.available), (40960, 16384));
    }

    #[test]
    fn a_mount_source_names_its_host_without_user_share_or_port() {
        for source in [
            "//100.109.9.62/Storage",
            "//jace@100.109.9.62/Storage",
            "smb://jace@100.109.9.62:445/Storage",
            "100.109.9.62:/export",
        ] {
            assert_eq!(host_of(source).as_deref(), Some("100.109.9.62"), "{source}");
        }
        assert_eq!(host_of("//"), None);
    }
}
