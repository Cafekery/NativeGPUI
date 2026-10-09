use std::ffi::{CStr, OsStr, c_char, c_int};
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use objc2_foundation::{
    NSArray, NSNumber, NSString, NSURL, NSURLVolumeAvailableCapacityForImportantUsageKey,
};

const SYNC_VOLUME_FULLSYNC: c_int = 0x01;
const SYNC_VOLUME_WAIT: c_int = 0x02;

unsafe extern "C" {
    fn sync_volume_np(path: *const c_char, flags: c_int) -> c_int;
}

/// What a destination volume can take, read once per transfer.
#[derive(Clone, Debug)]
pub(super) struct Volume {
    pub dev: u64,
    pub mount: PathBuf,
    /// APFS or Mac OS Extended, where Finder's own `.DS_Store` files belong.
    pub apple: bool,
    /// Without native extended attributes every attribute written lands in a `._` file.
    pub xattrs: bool,
    pub clones: bool,
    pub network: bool,
    pub read_only: bool,
    pub block: u64,
    pub max_file: u64,
}

impl Volume {
    pub fn of(dir: &Path) -> io::Result<Volume> {
        let path = crate::c_path(dir)?;
        let dev = std::fs::metadata(dir)?.dev() as u64;
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        crate::check(unsafe { libc::statfs(path.as_ptr(), &mut stat) })?;
        // SAFETY: the kernel NUL-terminates both names.
        let kind = unsafe { CStr::from_ptr(stat.f_fstypename.as_ptr()) };
        let mount = unsafe { CStr::from_ptr(stat.f_mntonname.as_ptr()) };
        let interfaces = interfaces(&path);
        let bits = unsafe { libc::pathconf(path.as_ptr(), libc::_PC_FILESIZEBITS) };
        Ok(Volume {
            dev,
            mount: PathBuf::from(OsStr::from_bytes(mount.to_bytes())),
            apple: matches!(kind.to_bytes(), b"apfs" | b"hfs"),
            xattrs: interfaces & libc::VOL_CAP_INT_EXTENDED_ATTR != 0,
            clones: interfaces & libc::VOL_CAP_INT_CLONE != 0,
            network: stat.f_flags & libc::MNT_LOCAL as u32 == 0,
            read_only: stat.f_flags & libc::MNT_RDONLY as u32 != 0,
            block: u64::from(stat.f_bsize).max(1),
            max_file: match bits {
                2..=63 => (1u64 << (bits - 1)) - 1,
                _ => u64::MAX,
            },
        })
    }

    /// What `len` bytes take up once the volume rounds them to its allocation block.
    pub fn allocated(&self, len: u64) -> u64 {
        len.div_ceil(self.block) * self.block
    }

    /// Counts purgeable space as free, as Finder does.
    pub fn available(&self) -> u64 {
        let key = unsafe { NSURLVolumeAvailableCapacityForImportantUsageKey };
        let url = NSURL::fileURLWithPath(&NSString::from_str(&self.mount.to_string_lossy()));
        let important = url
            .resourceValuesForKeys_error(&NSArray::from_slice(&[key]))
            .ok()
            .and_then(|values| values.objectForKey(key))
            .and_then(|value| value.downcast::<NSNumber>().ok())
            .map(|number| number.longLongValue().max(0) as u64)
            .filter(|&free| free > 0);
        important.unwrap_or_else(|| self.statfs_available())
    }

    fn statfs_available(&self) -> u64 {
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        let Ok(path) = crate::c_path(&self.mount) else {
            return 0;
        };
        match unsafe { libc::statfs(path.as_ptr(), &mut stat) } {
            0 => stat.f_bavail.saturating_mul(u64::from(stat.f_bsize)),
            _ => 0,
        }
    }

    /// A network volume is never synced whole: `sync_volume_np` on an SMB mount can hang for minutes.
    pub fn flush(&self) -> io::Result<()> {
        if self.network {
            return Ok(());
        }
        let path = crate::c_path(&self.mount)?;
        crate::check(unsafe {
            sync_volume_np(path.as_ptr(), SYNC_VOLUME_FULLSYNC | SYNC_VOLUME_WAIT)
        })
    }
}

pub(super) fn native_xattrs(path: &Path) -> bool {
    crate::c_path(path).is_ok_and(|path| interfaces(&path) & libc::VOL_CAP_INT_EXTENDED_ATTR != 0)
}

fn interfaces(path: &CStr) -> u32 {
    #[repr(C)]
    struct Reply {
        length: u32,
        capabilities: libc::vol_capabilities_attr_t,
    }
    let mut wanted = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_CAPABILITIES,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut reply: Reply = unsafe { std::mem::zeroed() };
    let got = unsafe {
        libc::getattrlist(
            path.as_ptr(),
            (&mut wanted as *mut libc::attrlist).cast(),
            (&mut reply as *mut Reply).cast(),
            std::mem::size_of::<Reply>(),
            0,
        )
    };
    let at = libc::VOL_CAPABILITIES_INTERFACES;
    match got {
        0 => reply.capabilities.capabilities[at] & reply.capabilities.valid[at],
        _ => 0,
    }
}
