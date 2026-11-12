use std::ffi::{CStr, c_char, c_int};
use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use crate::volumes;

const SYNC_VOLUME_FULLSYNC: c_int = 0x01;
const SYNC_VOLUME_WAIT: c_int = 0x02;

unsafe extern "C" {
    fn sync_volume_np(path: *const c_char, flags: c_int) -> c_int;
}

pub(super) struct Volume {
    pub dev: u64,
    pub mount: PathBuf,
    pub apple: bool,
    pub xattrs: bool,
    pub clones: bool,
    pub network: bool,
    pub read_only: bool,
    pub block: u64,
    pub max_file: u64,
}

impl Volume {
    pub fn of(dir: &Path) -> io::Result<Volume> {
        // No mount point lies past `PATH_MAX`: the nearest ancestor that resolves is on the volume.
        let found = dir
            .ancestors()
            .map(|at| (at, std::fs::metadata(at)))
            .find(|(_, meta)| {
                !matches!(meta, Err(error) if error.raw_os_error() == Some(libc::ENAMETOOLONG))
            });
        let Some((dir, meta)) = found else {
            return Err(io::Error::from_raw_os_error(libc::ENAMETOOLONG));
        };
        let dev = meta?.dev();
        let path = crate::c_path(dir)?;
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        crate::check(unsafe { libc::statfs(path.as_ptr(), &mut stat) })?;
        let kind = volumes::statfs_str(&stat.f_fstypename);
        let interfaces = interfaces(&path);
        let bits = unsafe { libc::pathconf(path.as_ptr(), libc::_PC_FILESIZEBITS) };
        Ok(Volume {
            dev,
            mount: volumes::mount_path(&stat),
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

    pub fn allocated(&self, len: u64) -> u64 {
        len.div_ceil(self.block) * self.block
    }

    pub fn available(&self) -> u64 {
        volumes::available(&self.mount)
    }

    /// Never a network volume: `sync_volume_np` on an SMB mount can hang for minutes.
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
            (&raw mut wanted).cast(),
            (&raw mut reply).cast(),
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
