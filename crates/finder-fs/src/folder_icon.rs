use std::ffi::{CStr, CString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use objc2::rc::Retained;
use objc2_foundation::{
    NSData, NSDictionary, NSJSONReadingOptions, NSJSONSerialization, NSJSONWritingOptions, NSString,
};

/// What Customize Folder… draws on a folder's icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decoration {
    Symbol(String),
    Emoji(String),
}

const DECORATION: &CStr = c"com.apple.icon.folder#S";
const FINDER_INFO: &CStr = c"com.apple.FinderInfo";
// FinderInfo's big-endian `finderFlags` sit at byte 8; `kHasCustomIcon` is its 0x0400 bit.
const HAS_CUSTOM_ICON: u8 = 0x04;

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)
}

fn get(path: &CStr, name: &CStr) -> Option<Vec<u8>> {
    let size =
        unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
    let mut bytes = vec![0u8; usize::try_from(size).ok()?];
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

fn set(path: &CStr, name: &CStr, bytes: &[u8]) -> io::Result<()> {
    let done = unsafe {
        libc::setxattr(
            path.as_ptr(),
            name.as_ptr(),
            bytes.as_ptr().cast(),
            bytes.len(),
            0,
            0,
        )
    };
    match done {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

fn remove(path: &CStr, name: &CStr) -> io::Result<()> {
    if unsafe { libc::removexattr(path.as_ptr(), name.as_ptr(), 0) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENOATTR) => Ok(()),
        _ => Err(error),
    }
}

pub fn read(path: &Path) -> Option<Decoration> {
    let bytes = get(&c_path(path).ok()?, DECORATION)?;
    let object = NSJSONSerialization::JSONObjectWithData_options_error(
        &NSData::with_bytes(&bytes),
        NSJSONReadingOptions::empty(),
    )
    .ok()?
    .downcast::<NSDictionary>()
    .ok()?;
    let text = |key: &str| -> Option<String> {
        let value = object.objectForKey(&NSString::from_str(key))?;
        Some(value.downcast::<NSString>().ok()?.to_string())
    };
    text("sym")
        .map(Decoration::Symbol)
        .or_else(|| text("emoji").map(Decoration::Emoji))
}

fn json(decoration: &Decoration) -> io::Result<Retained<NSData>> {
    let (key, value) = match decoration {
        Decoration::Symbol(name) => ("sym", name),
        Decoration::Emoji(text) => ("emoji", text),
    };
    let object =
        NSDictionary::from_slices(&[&*NSString::from_str(key)], &[&*NSString::from_str(value)]);
    unsafe {
        NSJSONSerialization::dataWithJSONObject_options_error(
            &object,
            NSJSONWritingOptions::empty(),
        )
    }
    .map_err(|error| io::Error::other(error.localizedDescription().to_string()))
}

/// Sets or, with `None`, clears the decoration and FinderInfo's custom-icon flag that makes icons show it.
pub fn write(path: &Path, decoration: Option<&Decoration>) -> io::Result<()> {
    let path = c_path(path)?;
    match decoration {
        Some(decoration) => set(&path, DECORATION, &json(decoration)?.to_vec())?,
        None => remove(&path, DECORATION)?,
    }
    let mut info = get(&path, FINDER_INFO).unwrap_or_default();
    info.resize(32, 0);
    match decoration {
        Some(_) => info[8] |= HAS_CUSTOM_ICON,
        None => info[8] &= !HAS_CUSTOM_ICON,
    }
    match info.iter().all(|byte| *byte == 0) {
        true => remove(&path, FINDER_INFO),
        false => set(&path, FINDER_INFO, &info),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_keeps_the_other_finder_flags() {
        let dir =
            std::env::temp_dir().join(format!("finder-fs-folder-icon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = c_path(&dir).unwrap();
        let mut labelled = [0u8; 32];
        labelled[9] = 0x0c;
        set(&path, FINDER_INFO, &labelled).unwrap();

        let dog = Decoration::Symbol("dog.fill".into());
        write(&dir, Some(&dog)).unwrap();
        assert_eq!(read(&dir), Some(dog));
        assert_eq!(get(&path, FINDER_INFO).unwrap()[8..10], [0x04, 0x0c]);

        write(&dir, None).unwrap();
        assert_eq!(read(&dir), None);
        assert_eq!(get(&path, FINDER_INFO).unwrap(), labelled);

        set(&path, FINDER_INFO, &[0u8; 32]).unwrap();
        write(&dir, Some(&Decoration::Emoji("🐶".into()))).unwrap();
        write(&dir, None).unwrap();
        assert_eq!(get(&path, FINDER_INFO), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
