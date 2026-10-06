use std::ffi::CStr;
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use crate::xattr;

const RESOURCE_FORK: &CStr = c"com.apple.ResourceFork";
const ICON_FILE: &str = "Icon\r";
const VOLUME_ICON: &str = ".VolumeIcon.icns";
const ICNS: &[u8] = b"icns";
const CUSTOM_ICON_ID: i16 = -16455;
const SF_RESTRICTED: u32 = 0x0008_0000;

/// Finder's rule: neither locked nor system-restricted, and writable by this user.
pub fn can_change(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.st_flags() & (libc::UF_IMMUTABLE | libc::SF_IMMUTABLE | SF_RESTRICTED) != 0 {
        return false;
    }
    crate::c_path(path).is_ok_and(|name| unsafe { libc::access(name.as_ptr(), libc::W_OK) } == 0)
}

/// The icns a custom icon is kept as: a file's own resource fork, a folder's `Icon\r`, a volume's `.VolumeIcon.icns`.
pub fn stored(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_dir() {
        return in_fork(path);
    }
    let mounted = path
        .parent()
        .is_none_or(|parent| std::fs::metadata(parent).is_ok_and(|up| up.dev() != meta.dev()));
    if mounted {
        if let Ok(icns) = std::fs::read(path.join(VOLUME_ICON)) {
            return Some(icns);
        }
    }
    in_fork(&path.join(ICON_FILE))
}

fn in_fork(path: &Path) -> Option<Vec<u8>> {
    let fork = xattr::get(&crate::c_path(path).ok()?, RESOURCE_FORK)?;
    icns_in(&fork).map(<[u8]>::to_vec)
}

fn icns_in(fork: &[u8]) -> Option<&[u8]> {
    let word =
        |at: usize| Some(u32::from_be_bytes(fork.get(at..at + 4)?.try_into().ok()?) as usize);
    let half = |at: usize| Some(u16::from_be_bytes(fork.get(at..at + 2)?.try_into().ok()?));
    let (data, map) = (word(0)?, word(4)?);
    let types = map + usize::from(half(map + 24)?);
    // Counts are stored less one, so an emptied map says 0xFFFF.
    let kinds = usize::from(half(types)?.wrapping_add(1));
    (0..kinds).find_map(|kind| {
        let entry = types + 2 + kind * 8;
        if fork.get(entry..entry + 4)? != ICNS {
            return None;
        }
        let count = usize::from(half(entry + 4)?.wrapping_add(1));
        let list = types + usize::from(half(entry + 6)?);
        (0..count).find_map(|at| {
            let reference = list + at * 12;
            if half(reference)? as i16 != CUSTOM_ICON_ID {
                return None;
            }
            let start = data + (word(reference + 4)? & 0x00FF_FFFF);
            fork.get(start + 4..start + 4 + word(start)?)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fork(resources: &[(&[u8; 4], i16, &[u8])]) -> Vec<u8> {
        let mut data = Vec::new();
        let mut offsets = Vec::new();
        for (_, _, body) in resources {
            offsets.push(data.len() as u32);
            data.extend_from_slice(&(body.len() as u32).to_be_bytes());
            data.extend_from_slice(body);
        }
        let types_at = 28u16;
        let mut types = ((resources.len() as u16).wrapping_sub(1))
            .to_be_bytes()
            .to_vec();
        let mut references = Vec::new();
        let first_reference = 2 + 8 * resources.len();
        for (index, (kind, id, _)) in resources.iter().enumerate() {
            types.extend_from_slice(*kind);
            types.extend_from_slice(&0u16.to_be_bytes());
            types.extend_from_slice(&((first_reference + index * 12) as u16).to_be_bytes());
            references.extend_from_slice(&id.to_be_bytes());
            references.extend_from_slice(&0xFFFFu16.to_be_bytes());
            references.extend_from_slice(&offsets[index].to_be_bytes());
            references.extend_from_slice(&0u32.to_be_bytes());
        }
        let mut map = vec![0u8; 24];
        map.extend_from_slice(&types_at.to_be_bytes());
        map.extend_from_slice(&0u16.to_be_bytes());
        map.extend_from_slice(&types);
        map.extend_from_slice(&references);
        let data_at = 256u32;
        let map_at = data_at + data.len() as u32;
        let mut fork = Vec::new();
        for word in [data_at, map_at, data.len() as u32, map.len() as u32] {
            fork.extend_from_slice(&word.to_be_bytes());
        }
        fork.resize(data_at as usize, 0);
        fork.extend_from_slice(&data);
        fork.extend_from_slice(&map);
        fork
    }

    #[test]
    fn finds_the_custom_icon_among_other_resources() {
        let built = fork(&[
            (b"STR ", 128, b"text"),
            (b"icns", 128, b"not the custom one"),
            (b"icns", CUSTOM_ICON_ID, b"icns-body"),
        ]);
        assert_eq!(icns_in(&built), Some(&b"icns-body"[..]));
    }

    #[test]
    fn an_emptied_fork_holds_no_icon() {
        assert_eq!(icns_in(&fork(&[])), None);
        assert_eq!(icns_in(&fork(&[(b"STR ", 128, b"text")])), None);
        assert_eq!(icns_in(&[0u8; 3]), None);
    }

    #[test]
    fn a_locked_file_cannot_change_its_icon() {
        let file = std::env::temp_dir().join(format!("finder-fs-icon-{}", std::process::id()));
        std::fs::write(&file, b"pad").unwrap();
        assert!(can_change(&file));
        let name = crate::c_path(&file).unwrap();
        assert_eq!(
            unsafe { libc::chflags(name.as_ptr(), libc::UF_IMMUTABLE) },
            0
        );
        assert!(!can_change(&file));
        assert_eq!(unsafe { libc::chflags(name.as_ptr(), 0) }, 0);
        assert!(can_change(&file));
        assert_eq!(stored(&file), None);
        std::fs::remove_file(&file).unwrap();
    }
}
