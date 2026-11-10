use std::ffi::CStr;
use std::io;
use std::path::Path;

use objc2::rc::Retained;
use objc2_foundation::{
    NSData, NSDictionary, NSJSONReadingOptions, NSJSONSerialization, NSJSONWritingOptions,
    NSString, ns_string,
};

use crate::list::HAS_CUSTOM_ICON;
use crate::xattr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decoration {
    Symbol(String),
    Emoji(String),
}

const DECORATION: &CStr = c"com.apple.icon.folder#S";

pub fn read(path: &Path) -> Option<Decoration> {
    let bytes = xattr::get(&crate::c_path(path).ok()?, DECORATION)?;
    let object = NSJSONSerialization::JSONObjectWithData_options_error(
        &NSData::with_bytes(&bytes),
        NSJSONReadingOptions::empty(),
    )
    .ok()?
    .downcast::<NSDictionary>()
    .ok()?;
    let text = |key: &NSString| {
        let value = object.objectForKey(key)?;
        Some(value.downcast::<NSString>().ok()?.to_string())
    };
    text(ns_string!("sym"))
        .map(Decoration::Symbol)
        .or_else(|| text(ns_string!("emoji")).map(Decoration::Emoji))
}

fn json(decoration: &Decoration) -> io::Result<Retained<NSData>> {
    let (key, value) = match decoration {
        Decoration::Symbol(name) => (ns_string!("sym"), name),
        Decoration::Emoji(text) => (ns_string!("emoji"), text),
    };
    let object = NSDictionary::from_slices(&[key], &[&*NSString::from_str(value)]);
    unsafe {
        NSJSONSerialization::dataWithJSONObject_options_error(
            &object,
            NSJSONWritingOptions::empty(),
        )
    }
    .map_err(|error| io::Error::other(error.localizedDescription().to_string()))
}

pub fn write(path: &Path, decoration: Option<&Decoration>) -> io::Result<()> {
    let named = crate::c_path(path)?;
    match decoration {
        Some(decoration) => xattr::set(&named, DECORATION, &json(decoration)?.to_vec())?,
        None => xattr::remove(&named, DECORATION)?,
    }
    crate::flags::set(path, HAS_CUSTOM_ICON, decoration.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::FINDER_INFO;

    #[test]
    fn clearing_keeps_the_other_finder_flags() {
        let dir =
            std::env::temp_dir().join(format!("finder-fs-folder-icon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = crate::c_path(&dir).unwrap();
        let mut labelled = [0u8; 32];
        labelled[9] = 0x0c;
        xattr::set(&path, FINDER_INFO, &labelled).unwrap();

        let dog = Decoration::Symbol("dog.fill".into());
        write(&dir, Some(&dog)).unwrap();
        assert_eq!(read(&dir), Some(dog));
        assert_eq!(xattr::get(&path, FINDER_INFO).unwrap()[8..10], [0x04, 0x0c]);

        write(&dir, None).unwrap();
        assert_eq!(read(&dir), None);
        assert_eq!(xattr::get(&path, FINDER_INFO).unwrap(), labelled);

        xattr::set(&path, FINDER_INFO, &[0u8; 32]).unwrap();
        write(&dir, Some(&Decoration::Emoji("🐶".into()))).unwrap();
        write(&dir, None).unwrap();
        assert_eq!(xattr::get(&path, FINDER_INFO), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
