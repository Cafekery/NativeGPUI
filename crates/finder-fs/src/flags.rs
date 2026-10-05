use std::ffi::CStr;
use std::io;
use std::path::Path;

use crate::xattr;

pub(crate) const FINDER_INFO: &CStr = c"com.apple.FinderInfo";

pub const IS_STATIONERY: u16 = 0x0800;

pub fn of(path: &Path) -> u16 {
    let info = crate::c_path(path)
        .ok()
        .and_then(|path| xattr::get(&path, FINDER_INFO))
        .unwrap_or_default();
    match info.get(8..10) {
        Some(&[high, low]) => u16::from_be_bytes([high, low]),
        _ => 0,
    }
}

pub fn set(path: &Path, flag: u16, on: bool) -> io::Result<()> {
    let path = crate::c_path(path)?;
    let mut info = xattr::get(&path, FINDER_INFO).unwrap_or_default();
    info.resize(32, 0);
    let flags = u16::from_be_bytes([info[8], info[9]]);
    let flags = match on {
        true => flags | flag,
        false => flags & !flag,
    };
    info[8..10].copy_from_slice(&flags.to_be_bytes());
    match info.iter().all(|byte| *byte == 0) {
        true => xattr::remove(&path, FINDER_INFO),
        false => xattr::set(&path, FINDER_INFO, &info),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stationery_round_trips_beside_other_flags() {
        let file = std::env::temp_dir().join(format!("finder-fs-flags-{}", std::process::id()));
        std::fs::write(&file, b"pad").unwrap();
        assert_eq!(of(&file), 0);
        set(&file, IS_STATIONERY, true).unwrap();
        assert_eq!(of(&file), IS_STATIONERY);
        set(&file, 0x000c, true).unwrap();
        set(&file, IS_STATIONERY, false).unwrap();
        assert_eq!(of(&file), 0x000c);
        set(&file, 0x000c, false).unwrap();
        let path = crate::c_path(&file).unwrap();
        assert_eq!(xattr::get(&path, FINDER_INFO), None);
        std::fs::remove_file(&file).unwrap();
    }
}
